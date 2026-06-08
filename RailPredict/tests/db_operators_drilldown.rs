//! Database integration tests for the per-operator drill-down queries in
//! `src/db/operators.rs` (`operator_detail`, `operator_daily_series`,
//! `operator_delay_distribution`, `operator_routes`).
//!
//! These now read from `journeys` (the per-service `toc` from the Darwin schedule message),
//! so each test seeds `journeys` (+ `operators` for the friendly name, + `prediction_outcomes`
//! for MAE). Each test gets a freshly migrated, isolated Postgres database via
//! `#[sqlx::test(migrations = "../migrations")]`.
//!
//! # Running locally
//! ```bash
//! cd RailPredict
//! DATABASE_URL=postgresql://railpredict:railpredict@localhost:5432/railpredict \
//!   cargo test --test db_operators_drilldown
//! ```

use railpredict::db::operators::{
    operator_daily_series, operator_delay_distribution, operator_detail, operator_routes,
};

/// Seed an operator reference row.
async fn seed_operator(
    pool: &sqlx::PgPool,
    toc: &str,
    name: &str,
    brand_color: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO operators (toc, name, brand_color) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING",
    )
    .bind(toc)
    .bind(name)
    .bind(brand_color)
    .execute(pool)
    .await?;
    Ok(())
}

/// Seed one finalised journey for an operator. `arrival_delay` drives the on-time / avg / band
/// metrics; `prediction = (predicted, final)` adds a matching `prediction_outcomes` row for MAE.
/// `age_secs` ages `finalised_at` so rows can be spread across the window / across days.
#[allow(clippy::too_many_arguments)]
async fn seed_journey(
    pool: &sqlx::PgPool,
    rid: &str,
    uid: &str,
    toc: &str,
    origin_tpl: &str,
    dest_tpl: &str,
    arrival_delay: i32,
    age_secs: i64,
    prediction: Option<(i32, i32)>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO journeys \
            (rid, uid, ssd, weekday, departure_hour, toc, origin_tpl, destination_tpl, \
             scheduled_departure, arrival_delay_mins, finalised_at) \
         VALUES ($1,$2, CURRENT_DATE, 0, 9, $3, $4, $5, NOW(), $6, \
                 NOW() - ($7::INT * INTERVAL '1 second'))",
    )
    .bind(rid)
    .bind(uid)
    .bind(toc)
    .bind(origin_tpl)
    .bind(dest_tpl)
    .bind(arrival_delay)
    .bind(age_secs as i32)
    .execute(pool)
    .await?;
    if let Some((pred, fin)) = prediction {
        sqlx::query(
            "INSERT INTO prediction_outcomes \
                (rid, uid, origin_crs, scheduled_departure, predicted_delay_mins, \
                 final_delay_mins, finalised_at) \
             VALUES ($1,$2,'XXX', NOW(), $3, $4, NOW())",
        )
        .bind(rid)
        .bind(uid)
        .bind(pred)
        .bind(fin)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// `operator_detail` aggregates on-time %, avg delay, MAE, and sample count; `name` is
/// COALESCEd from `operators`; out-of-window / out-of-operator rows are excluded.
#[sqlx::test(migrations = "../migrations")]
async fn detail_aggregates_and_coalesce_name(pool: sqlx::PgPool) -> sqlx::Result<()> {
    seed_operator(&pool, "GW", "Great Western", "#0a493e").await?;
    // Arrival delays 0,0,10,20 → on-time (<=5) = 2/4 = 50%, avg = 7.5.
    // Predictions on three rows → |pred-final| = 2, 5, 10 → MAE = 17/3.
    seed_journey(&pool, "100000000000001", "C00001", "GW", "PAD", "RDG", 0, 10, Some((2, 0))).await?;
    seed_journey(&pool, "100000000000002", "C00001", "GW", "PAD", "RDG", 0, 20, Some((5, 0))).await?;
    seed_journey(&pool, "100000000000003", "C00001", "GW", "PAD", "RDG", 10, 30, Some((20, 10))).await?;
    seed_journey(&pool, "100000000000004", "C00001", "GW", "PAD", "RDG", 20, 40, None).await?;

    let detail = operator_detail(&pool, "GW", 24)
        .await?
        .expect("GW should have in-window rows");
    assert_eq!(detail.toc.trim(), "GW");
    assert_eq!(detail.name, "Great Western");
    assert_eq!(detail.brand_color, "#0a493e");
    assert_eq!(detail.sample_count, 4);
    assert!((detail.on_time_pct.unwrap() - 50.0).abs() < 1e-6);
    assert!((detail.avg_delay_mins.unwrap() - 7.5).abs() < 1e-6);
    assert!((detail.mae_mins.unwrap() - (17.0 / 3.0)).abs() < 1e-6);

    assert!(operator_detail(&pool, "NO", 24).await?.is_none());
    Ok(())
}

/// With no `operators` row, `name` falls back to the TOC code and `brand_color` to grey.
#[sqlx::test(migrations = "../migrations")]
async fn detail_name_falls_back_to_toc(pool: sqlx::PgPool) -> sqlx::Result<()> {
    seed_journey(&pool, "100000000000001", "C00099", "XX", "EUS", "MAN", 3, 5, None).await?;
    let detail = operator_detail(&pool, "XX", 24).await?.expect("XX row");
    assert_eq!(detail.name.trim(), "XX");
    assert_eq!(detail.brand_color, "#9aa7b4");
    assert!(detail.mae_mins.is_none(), "no predictions → MAE is NULL");
    Ok(())
}

/// `operator_daily_series` groups by Europe/London calendar day, oldest → newest.
#[sqlx::test(migrations = "../migrations")]
async fn daily_series_groups_by_day(pool: sqlx::PgPool) -> sqlx::Result<()> {
    seed_operator(&pool, "GW", "Great Western", "#0a493e").await?;
    seed_journey(&pool, "100000000000001", "C00001", "GW", "PAD", "RDG", 0, 60, None).await?;
    seed_journey(&pool, "100000000000002", "C00001", "GW", "PAD", "RDG", 4, 120, None).await?;
    let two_days = 2 * 24 * 3600;
    seed_journey(&pool, "100000000000003", "C00001", "GW", "PAD", "RDG", 0, two_days + 60, None).await?;
    seed_journey(&pool, "100000000000004", "C00001", "GW", "PAD", "RDG", 12, two_days + 120, None).await?;

    let series = operator_daily_series(&pool, "GW", 168).await?;
    assert!(series.len() >= 2, "expected >=2 days, got {}", series.len());
    for w in series.windows(2) {
        assert!(w[0].day <= w[1].day, "series must be ascending by day");
    }
    let total: i64 = series.iter().map(|p| p.sample_count).sum();
    assert_eq!(total, 4);
    Ok(())
}

/// `operator_delay_distribution` buckets the arrival delay into fixed bands.
#[sqlx::test(migrations = "../migrations")]
async fn distribution_bands_split_correctly(pool: sqlx::PgPool) -> sqlx::Result<()> {
    seed_operator(&pool, "GW", "Great Western", "#0a493e").await?;
    // <=0: two (-1, 0). (0,5]: three (1, 5, 3). (5,15]: one (6).
    seed_journey(&pool, "100000000000001", "C00001", "GW", "PAD", "RDG", -1, 10, None).await?;
    seed_journey(&pool, "100000000000002", "C00001", "GW", "PAD", "RDG", 0, 20, None).await?;
    seed_journey(&pool, "100000000000003", "C00001", "GW", "PAD", "RDG", 1, 30, None).await?;
    seed_journey(&pool, "100000000000004", "C00001", "GW", "PAD", "RDG", 5, 40, None).await?;
    seed_journey(&pool, "100000000000005", "C00001", "GW", "PAD", "RDG", 3, 50, None).await?;
    seed_journey(&pool, "100000000000006", "C00001", "GW", "PAD", "RDG", 6, 60, None).await?;

    let buckets = operator_delay_distribution(&pool, "GW", 24).await?;
    let bounds: Vec<i32> = buckets.iter().map(|b| b.lower_bound_mins).collect();
    assert_eq!(bounds, vec![0, 1, 6], "ascending non-empty band lower bounds");
    let count_for = |lb: i32| -> i64 {
        buckets.iter().find(|b| b.lower_bound_mins == lb).map(|b| b.sample_count).unwrap_or(0)
    };
    assert_eq!(count_for(0), 2, "on-time band (<=0)");
    assert_eq!(count_for(1), 3, "(0,5] band");
    assert_eq!(count_for(6), 1, "(5,15] band");
    Ok(())
}

/// `operator_routes` aggregates per `(origin_tpl, destination_tpl)`, worst avg delay first.
#[sqlx::test(migrations = "../migrations")]
async fn routes_ordered_by_avg_delay(pool: sqlx::PgPool) -> sqlx::Result<()> {
    seed_operator(&pool, "GW", "Great Western", "#0a493e").await?;
    // PAD→RDG avg ~1; PAD→BRI avg ~25.
    seed_journey(&pool, "100000000000001", "C00001", "GW", "PAD", "RDG", 0, 10, None).await?;
    seed_journey(&pool, "100000000000002", "C00001", "GW", "PAD", "RDG", 2, 20, None).await?;
    seed_journey(&pool, "100000000000003", "C00002", "GW", "PAD", "BRI", 20, 30, None).await?;
    seed_journey(&pool, "100000000000004", "C00002", "GW", "PAD", "BRI", 30, 40, None).await?;

    let routes = operator_routes(&pool, "GW", 24, 10).await?;
    assert_eq!(routes.len(), 2);
    assert_eq!(routes[0].origin_crs.trim(), "PAD");
    assert_eq!(routes[0].destination_crs.trim(), "BRI");
    assert!(routes[0].avg_delay_mins.unwrap() > routes[1].avg_delay_mins.unwrap());
    assert_eq!(routes[1].destination_crs.trim(), "RDG");

    let top1 = operator_routes(&pool, "GW", 24, 1).await?;
    assert_eq!(top1.len(), 1);
    assert_eq!(top1[0].destination_crs.trim(), "BRI");
    Ok(())
}
