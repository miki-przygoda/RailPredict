//! Database integration tests for `src/db/stations.rs` — the per-station /
//! per-route explorer queries that back the `/stations/:crs` page.
//!
//! Each test gets a freshly migrated, isolated Postgres database via
//! `#[sqlx::test(migrations = "../migrations")]`, so they run concurrently with
//! no shared state.
//!
//! # Seeding rules
//!
//! - `delay_history` has NO FK to `services`/`stations`, so the summary and
//!   heatmap tests insert directly into it. Rows must differ in at least one of
//!   `(uid, weekday, origin_crs, departure_hour, recorded_at)` to dodge the
//!   `dh_unique_observation_idx` unique index.
//! - `station_busiest_services` JOINs `services`, which FK-references
//!   `stations(crs)`. That test seeds in dependency order: `stations` → `services`
//!   → `delay_history`. The `stations` table only requires `crs` + `name`
//!   (all other columns have defaults).
//!
//! Seed queries use the runtime `sqlx::query` (not `sqlx::query!`) so no
//! compile-time DB connection / `.sqlx/` snapshot is needed.

use chrono::{Duration, Utc};

use railpredict::db::stations::{
    station_busiest_services, station_heatmap, station_summary,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Insert one delay_history row. `mins_ago` keeps rows inside the rolling window
/// and (combined with the other columns) keeps the unique index happy.
#[allow(clippy::too_many_arguments)]
async fn insert_delay(
    pool: &sqlx::PgPool,
    uid: &str,
    weekday: i16,
    origin_crs: &str,
    departure_hour: i16,
    delay_mins: i32,
    predicted: Option<i32>,
    mins_ago: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO delay_history
            (uid, weekday, origin_crs, departure_hour, delay_mins, predicted_delay_mins, recorded_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(uid)
    .bind(weekday)
    .bind(origin_crs)
    .bind(departure_hour)
    .bind(delay_mins)
    .bind(predicted)
    .bind(Utc::now() - Duration::minutes(mins_ago))
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_station(pool: &sqlx::PgPool, crs: &str, name: &str) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO stations (crs, name) VALUES ($1, $2) ON CONFLICT (crs) DO NOTHING")
        .bind(crs)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

async fn insert_service(
    pool: &sqlx::PgPool,
    uid: &str,
    origin_crs: &str,
    destination_crs: &str,
    toc: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO services (uid, origin_crs, destination_crs, toc) VALUES ($1, $2, $3, $4)",
    )
    .bind(uid)
    .bind(origin_crs)
    .bind(destination_crs)
    .bind(toc)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// station_summary
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../migrations")]
async fn summary_aggregates_window(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // 4 departures from LDS: two on time (<=0), two late. Two carry predictions.
    //   delay = 0, 5, 10, -2  -> avg = 3.25 ; on_time = 2/4 = 50%
    //   predicted/actual MAE over rows WITH a prediction:
    //     row delay=5  predicted=2  -> |2-5| = 3
    //     row delay=10 predicted=4  -> |4-10| = 6
    //     MAE = (3 + 6) / 2 = 4.5
    insert_delay(&pool, "C00001", 0, "LDS", 8, 0, None, 10).await?;
    insert_delay(&pool, "C00001", 0, "LDS", 9, 5, Some(2), 11).await?;
    insert_delay(&pool, "C00001", 1, "LDS", 8, 10, Some(4), 12).await?;
    insert_delay(&pool, "C00001", 2, "LDS", 8, -2, None, 13).await?;

    let summary = station_summary(&pool, "LDS", 24)
        .await?
        .expect("aggregate query always returns one row");

    assert_eq!(summary.crs, "LDS");
    assert_eq!(summary.sample_count, 4);
    assert!((summary.on_time_pct.unwrap() - 50.0).abs() < 1e-9, "on_time_pct");
    assert!((summary.avg_delay_mins.unwrap() - 3.25).abs() < 1e-9, "avg_delay_mins");
    assert!((summary.mae_mins.unwrap() - 4.5).abs() < 1e-9, "mae_mins");
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
async fn summary_filters_to_origin_and_window(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // In-window LDS row counts; a different origin (MAN) and an out-of-window
    // LDS row (30h ago) must both be excluded.
    insert_delay(&pool, "C00001", 0, "LDS", 8, 4, None, 10).await?;
    insert_delay(&pool, "C00002", 0, "MAN", 8, 9, None, 10).await?;
    insert_delay(&pool, "C00001", 0, "LDS", 9, 99, None, 60 * 30).await?;
    // Out-of-sanity-range row (>600) is dropped too.
    insert_delay(&pool, "C00001", 1, "LDS", 7, 9000, None, 10).await?;

    let summary = station_summary(&pool, "LDS", 24)
        .await?
        .expect("one row");
    assert_eq!(summary.sample_count, 1, "only the single in-window in-range LDS row");
    assert!((summary.avg_delay_mins.unwrap() - 4.0).abs() < 1e-9);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
async fn summary_empty_yields_zero_count_and_null_aggregates(pool: sqlx::PgPool) -> sqlx::Result<()> {
    let summary = station_summary(&pool, "XYZ", 24)
        .await?
        .expect("pure aggregate returns one row even with no matching data");
    assert_eq!(summary.crs, "XYZ");
    assert_eq!(summary.sample_count, 0, "no data => sample_count 0");
    assert!(summary.on_time_pct.is_none(), "AVG over empty set is NULL");
    assert!(summary.avg_delay_mins.is_none());
    assert!(summary.mae_mins.is_none());
    Ok(())
}

// ---------------------------------------------------------------------------
// station_heatmap
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../migrations")]
async fn heatmap_groups_distinct_weekday_hour_cells(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Three distinct (weekday, hour) cells. The Mon/08 cell gets two rows so we
    // can check averaging and counting within a cell.
    //   (Mon=0, 08): delays 4 and 8 -> avg 6, both late -> on_time 0%
    //   (Tue=1, 09): delay 0        -> on_time 100%
    //   (Wed=2, 17): delay 10
    insert_delay(&pool, "C00001", 0, "LDS", 8, 4, None, 10).await?;
    insert_delay(&pool, "C00002", 0, "LDS", 8, 8, None, 11).await?;
    insert_delay(&pool, "C00001", 1, "LDS", 9, 0, None, 12).await?;
    insert_delay(&pool, "C00001", 2, "LDS", 17, 10, None, 13).await?;

    let cells = station_heatmap(&pool, "LDS", 24).await?;
    assert_eq!(cells.len(), 3, "three distinct (weekday, hour) cells");

    // Ordered by weekday then hour.
    assert_eq!((cells[0].weekday, cells[0].departure_hour), (0, 8));
    assert_eq!((cells[1].weekday, cells[1].departure_hour), (1, 9));
    assert_eq!((cells[2].weekday, cells[2].departure_hour), (2, 17));

    // Mon/08 cell aggregates two rows.
    assert_eq!(cells[0].sample_count, 2);
    assert!((cells[0].avg_delay_mins.unwrap() - 6.0).abs() < 1e-9);
    assert!((cells[0].on_time_pct.unwrap() - 0.0).abs() < 1e-9);

    // Tue/09 cell is fully on time.
    assert_eq!(cells[1].sample_count, 1);
    assert!((cells[1].on_time_pct.unwrap() - 100.0).abs() < 1e-9);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
async fn heatmap_empty_returns_no_cells(pool: sqlx::PgPool) -> sqlx::Result<()> {
    let cells = station_heatmap(&pool, "LDS", 24).await?;
    assert!(cells.is_empty(), "no data => no cells");
    Ok(())
}

// ---------------------------------------------------------------------------
// station_busiest_services
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../migrations")]
async fn busiest_services_ranks_by_count_and_joins_destination_and_toc(
    pool: sqlx::PgPool,
) -> sqlx::Result<()> {
    // FK chain: stations -> services -> delay_history.
    insert_station(&pool, "LDS", "Leeds").await?;
    insert_station(&pool, "MAN", "Manchester Piccadilly").await?;
    insert_station(&pool, "YRK", "York").await?;

    insert_service(&pool, "C00001", "LDS", "MAN", "TP").await?;
    insert_service(&pool, "C00002", "LDS", "YRK", "NT").await?;

    // C00001: 3 observations. C00002: 1 observation. -> C00001 ranks first.
    insert_delay(&pool, "C00001", 0, "LDS", 8, 2, None, 10).await?;
    insert_delay(&pool, "C00001", 0, "LDS", 9, 6, None, 11).await?;
    insert_delay(&pool, "C00001", 1, "LDS", 8, 0, None, 12).await?;
    insert_delay(&pool, "C00002", 0, "LDS", 8, 3, None, 13).await?;

    let rows = station_busiest_services(&pool, "LDS", 24, 10).await?;
    assert_eq!(rows.len(), 2);

    // Ranked by sample_count DESC.
    assert_eq!(rows[0].uid, "C00001");
    assert_eq!(rows[0].sample_count, 3);
    assert_eq!(rows[0].destination_crs.as_deref(), Some("MAN"));
    assert_eq!(rows[0].toc.as_deref(), Some("TP"));
    // avg of 2, 6, 0 = 8/3; on_time = 1/3 (delay=0 row) * 100.
    assert!((rows[0].avg_delay_mins.unwrap() - (8.0 / 3.0)).abs() < 1e-9);
    assert!((rows[0].on_time_pct.unwrap() - (100.0 / 3.0)).abs() < 1e-9);

    assert_eq!(rows[1].uid, "C00002");
    assert_eq!(rows[1].sample_count, 1);
    assert_eq!(rows[1].destination_crs.as_deref(), Some("YRK"));
    assert_eq!(rows[1].toc.as_deref(), Some("NT"));
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
async fn busiest_services_includes_unmapped_uid_via_left_join(pool: sqlx::PgPool) -> sqlx::Result<()> {
    insert_station(&pool, "LDS", "Leeds").await?;
    insert_station(&pool, "MAN", "Manchester Piccadilly").await?;
    insert_service(&pool, "C00001", "LDS", "MAN", "TP").await?;

    // C00099 has NO matching services row -> must still appear with NULL dest/toc.
    insert_delay(&pool, "C00001", 0, "LDS", 8, 2, None, 10).await?;
    insert_delay(&pool, "C00099", 0, "LDS", 8, 4, None, 11).await?;
    insert_delay(&pool, "C00099", 1, "LDS", 9, 5, None, 12).await?;

    let rows = station_busiest_services(&pool, "LDS", 24, 10).await?;
    assert_eq!(rows.len(), 2);

    // C00099 has 2 obs vs C00001's 1, so it ranks first.
    assert_eq!(rows[0].uid, "C00099");
    assert_eq!(rows[0].sample_count, 2);
    assert!(rows[0].destination_crs.is_none(), "unmapped uid => NULL destination");
    assert!(rows[0].toc.is_none(), "unmapped uid => NULL toc");

    assert_eq!(rows[1].uid, "C00001");
    assert_eq!(rows[1].destination_crs.as_deref(), Some("MAN"));
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
async fn busiest_services_respects_limit(pool: sqlx::PgPool) -> sqlx::Result<()> {
    insert_station(&pool, "LDS", "Leeds").await?;
    // Three distinct services, descending observation counts: A=3, B=2, C=1.
    for (uid, n) in [("C0000A", 3), ("C0000B", 2), ("C0000C", 1)] {
        for i in 0..n {
            insert_delay(&pool, uid, (i % 7) as i16, "LDS", (8 + i) as i16, 1, None, 10 + i as i64)
                .await?;
        }
    }

    let rows = station_busiest_services(&pool, "LDS", 24, 2).await?;
    assert_eq!(rows.len(), 2, "LIMIT 2 caps the result set");
    assert_eq!(rows[0].uid, "C0000A");
    assert_eq!(rows[1].uid, "C0000B");
    Ok(())
}
