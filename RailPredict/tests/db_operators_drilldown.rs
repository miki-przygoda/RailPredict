//! Database integration tests for the per-operator drill-down queries in
//! `src/db/operators.rs` (`operator_detail`, `operator_daily_series`,
//! `operator_delay_distribution`, `operator_routes`).
//!
//! Each test gets a freshly migrated, isolated Postgres database via
//! `#[sqlx::test(migrations = "../migrations")]`.
//!
//! # FK chain
//!
//! `services` has NOT NULL FKs `origin_crs` / `destination_crs` →
//! `stations(crs)`, so every test seeds in this order:
//!   1. `stations` (only the NOT NULL columns `crs`, `name`),
//!   2. `operators`,
//!   3. `services` (which references the stations),
//!   4. `delay_history` (no FK to services; joined at query time by `uid`).
//!
//! `delay_history` has a unique index on
//! `(uid, weekday, origin_crs, departure_hour, recorded_at)`, so seed rows use
//! distinct tuples (we vary `recorded_at` per row) to avoid collisions.
//!
//! # Running locally
//!
//! ```bash
//! cd RailPredict
//! DATABASE_URL=postgresql://railpredict:railpredict@localhost:5432/railpredict \
//!   cargo test --test db_operators_drilldown
//! ```

use railpredict::db::operators::{
    operator_daily_series, operator_delay_distribution, operator_detail, operator_routes,
};

/// Seed a station (only NOT NULL columns: `crs`, `name`).
async fn seed_station(pool: &sqlx::PgPool, crs: &str, name: &str) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO stations (crs, name) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(crs)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(())
}

/// Seed an operator reference row.
async fn seed_operator(
    pool: &sqlx::PgPool,
    toc: &str,
    name: &str,
    brand_color: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO operators (toc, name, brand_color) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(toc)
    .bind(name)
    .bind(brand_color)
    .execute(pool)
    .await?;
    Ok(())
}

/// Seed a service (recurring O–D identity for an operator).
async fn seed_service(
    pool: &sqlx::PgPool,
    uid: &str,
    origin: &str,
    destination: &str,
    toc: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO services (uid, origin_crs, destination_crs, toc) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (uid) DO NOTHING",
    )
    .bind(uid)
    .bind(origin)
    .bind(destination)
    .bind(toc)
    .execute(pool)
    .await?;
    Ok(())
}

/// Seed one delay observation. `recorded_at` is computed as `NOW()` minus
/// `age_secs` seconds, keeping rows inside a short window while still giving
/// each row a distinct timestamp (the unique index includes `recorded_at`).
#[allow(clippy::too_many_arguments)]
async fn seed_delay(
    pool: &sqlx::PgPool,
    uid: &str,
    weekday: i16,
    origin: &str,
    delay_mins: i32,
    departure_hour: i16,
    predicted: Option<i32>,
    age_secs: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO delay_history \
            (uid, weekday, origin_crs, delay_mins, departure_hour, predicted_delay_mins, recorded_at) \
         VALUES ($1, $2, $3, $4, $5, $6, NOW() - ($7::INT * INTERVAL '1 second'))",
    )
    .bind(uid)
    .bind(weekday)
    .bind(origin)
    .bind(delay_mins)
    .bind(departure_hour)
    .bind(predicted)
    .bind(age_secs as i32)
    .execute(pool)
    .await?;
    Ok(())
}

/// `operator_detail` aggregates on-time %, avg delay, MAE, and sample count;
/// `name` is COALESCEd from `operators`, and out-of-window / out-of-operator
/// rows are excluded.
#[sqlx::test(migrations = "../migrations")]
async fn detail_aggregates_and_coalesce_name(pool: sqlx::PgPool) -> sqlx::Result<()> {
    seed_station(&pool, "PAD", "London Paddington").await?;
    seed_station(&pool, "RDG", "Reading").await?;
    seed_operator(&pool, "GW", "Great Western", "#0a493e").await?;
    seed_service(&pool, "C00001", "PAD", "RDG", "GW").await?;

    // In-window GW rows: delays 0, 0, 10, 20  → on-time = 2/4 = 50%, avg = 7.5.
    // predicted captured on three rows → |pred-actual| = 2, 5, 10 → MAE = 17/3.
    seed_delay(&pool, "C00001", 0, "PAD", 0, 8, Some(2), 10).await?;
    seed_delay(&pool, "C00001", 0, "PAD", 0, 8, Some(5), 20).await?;
    seed_delay(&pool, "C00001", 0, "PAD", 10, 9, Some(20), 30).await?;
    seed_delay(&pool, "C00001", 0, "PAD", 20, 9, None, 40).await?;

    let detail = operator_detail(&pool, "GW", 24)
        .await?
        .expect("GW should have in-window rows");

    assert_eq!(detail.toc, "GW");
    assert_eq!(detail.name, "Great Western");
    assert_eq!(detail.brand_color, "#0a493e");
    assert_eq!(detail.sample_count, 4);
    assert!((detail.on_time_pct.unwrap() - 50.0).abs() < 1e-6);
    assert!((detail.avg_delay_mins.unwrap() - 7.5).abs() < 1e-6);
    // MAE over the three rows with a prediction: (2 + 5 + 10) / 3.
    assert!((detail.mae_mins.unwrap() - (17.0 / 3.0)).abs() < 1e-6);

    // An operator with no rows returns None.
    assert!(operator_detail(&pool, "NOPE", 24).await?.is_none());

    Ok(())
}

/// When no `operators` row exists, `name` falls back to the TOC code and
/// `brand_color` to the neutral grey sentinel.
#[sqlx::test(migrations = "../migrations")]
async fn detail_name_falls_back_to_toc(pool: sqlx::PgPool) -> sqlx::Result<()> {
    seed_station(&pool, "EUS", "London Euston").await?;
    seed_station(&pool, "MAN", "Manchester Piccadilly").await?;
    // No operators row for "XX".
    seed_service(&pool, "C00099", "EUS", "MAN", "XX").await?;
    seed_delay(&pool, "C00099", 1, "EUS", 3, 7, None, 5).await?;

    let detail = operator_detail(&pool, "XX", 24)
        .await?
        .expect("XX should have a row");
    assert_eq!(detail.name, "XX");
    assert_eq!(detail.brand_color, "#9aa7b4");
    assert!(detail.mae_mins.is_none(), "no predictions → MAE is NULL");

    Ok(())
}

/// `operator_daily_series` groups observations by Europe/London calendar day
/// and returns points oldest → newest.
#[sqlx::test(migrations = "../migrations")]
async fn daily_series_groups_by_day(pool: sqlx::PgPool) -> sqlx::Result<()> {
    seed_station(&pool, "PAD", "London Paddington").await?;
    seed_station(&pool, "RDG", "Reading").await?;
    seed_operator(&pool, "GW", "Great Western", "#0a493e").await?;
    seed_service(&pool, "C00001", "PAD", "RDG", "GW").await?;

    // Two rows ~today (small ages) and two rows ~2 days ago. With a 168h window
    // all four are in-window but split across (at least) two distinct days.
    seed_delay(&pool, "C00001", 0, "PAD", 0, 8, None, 60).await?;
    seed_delay(&pool, "C00001", 0, "PAD", 4, 8, None, 120).await?;
    let two_days = 2 * 24 * 3600;
    seed_delay(&pool, "C00001", 5, "PAD", 0, 9, None, two_days + 60).await?;
    seed_delay(&pool, "C00001", 5, "PAD", 12, 9, None, two_days + 120).await?;

    let series = operator_daily_series(&pool, "GW", 168).await?;
    assert!(
        series.len() >= 2,
        "expected at least two distinct days, got {}",
        series.len()
    );
    // Oldest → newest ordering.
    for w in series.windows(2) {
        assert!(w[0].day <= w[1].day, "series must be ascending by day");
    }
    // Total samples across all days equals the four in-window rows.
    let total: i64 = series.iter().map(|p| p.sample_count).sum();
    assert_eq!(total, 4);

    Ok(())
}

/// `operator_delay_distribution` buckets delays into fixed bands and returns one
/// row per non-empty band, ascending by `lower_bound_mins`. Seed delays straddle
/// the (0,5] and (5,15] bands plus the on-time band.
#[sqlx::test(migrations = "../migrations")]
async fn distribution_bands_split_correctly(pool: sqlx::PgPool) -> sqlx::Result<()> {
    seed_station(&pool, "PAD", "London Paddington").await?;
    seed_station(&pool, "RDG", "Reading").await?;
    seed_operator(&pool, "GW", "Great Western", "#0a493e").await?;
    seed_service(&pool, "C00001", "PAD", "RDG", "GW").await?;

    // on-time (<=0): two rows. (0,5]: three rows. (5,15]: one row.
    seed_delay(&pool, "C00001", 0, "PAD", -1, 8, None, 10).await?;
    seed_delay(&pool, "C00001", 0, "PAD", 0, 8, None, 20).await?;
    seed_delay(&pool, "C00001", 0, "PAD", 1, 8, None, 30).await?;
    seed_delay(&pool, "C00001", 0, "PAD", 5, 8, None, 40).await?;
    seed_delay(&pool, "C00001", 0, "PAD", 3, 8, None, 50).await?;
    seed_delay(&pool, "C00001", 0, "PAD", 6, 8, None, 60).await?;

    let buckets = operator_delay_distribution(&pool, "GW", 24).await?;
    // Three non-empty bands: lower bounds 0, 1, 6.
    let bounds: Vec<i32> = buckets.iter().map(|b| b.lower_bound_mins).collect();
    assert_eq!(bounds, vec![0, 1, 6], "ascending non-empty band lower bounds");

    let count_for = |lb: i32| -> i64 {
        buckets
            .iter()
            .find(|b| b.lower_bound_mins == lb)
            .map(|b| b.sample_count)
            .unwrap_or(0)
    };
    assert_eq!(count_for(0), 2, "on-time band (<=0)");
    assert_eq!(count_for(1), 3, "(0,5] band");
    assert_eq!(count_for(6), 1, "(5,15] band");

    Ok(())
}

/// `operator_routes` aggregates per `(origin, destination)` pair and orders by
/// average delay, worst first, respecting the limit.
#[sqlx::test(migrations = "../migrations")]
async fn routes_ordered_by_avg_delay(pool: sqlx::PgPool) -> sqlx::Result<()> {
    seed_station(&pool, "PAD", "London Paddington").await?;
    seed_station(&pool, "RDG", "Reading").await?;
    seed_station(&pool, "BRI", "Bristol Temple Meads").await?;
    seed_operator(&pool, "GW", "Great Western", "#0a493e").await?;
    // Two routes for the same operator.
    seed_service(&pool, "C00001", "PAD", "RDG", "GW").await?; // low delay
    seed_service(&pool, "C00002", "PAD", "BRI", "GW").await?; // high delay

    // PAD→RDG: avg ~1.
    seed_delay(&pool, "C00001", 0, "PAD", 0, 8, None, 10).await?;
    seed_delay(&pool, "C00001", 0, "PAD", 2, 8, None, 20).await?;
    // PAD→BRI: avg ~25.
    seed_delay(&pool, "C00002", 0, "PAD", 20, 9, None, 30).await?;
    seed_delay(&pool, "C00002", 0, "PAD", 30, 9, None, 40).await?;

    let routes = operator_routes(&pool, "GW", 24, 10).await?;
    assert_eq!(routes.len(), 2);
    // Worst (highest avg delay) first.
    assert_eq!(routes[0].origin_crs, "PAD");
    assert_eq!(routes[0].destination_crs, "BRI");
    assert!(routes[0].avg_delay_mins.unwrap() > routes[1].avg_delay_mins.unwrap());
    assert_eq!(routes[1].destination_crs, "RDG");

    // Limit is respected.
    let top1 = operator_routes(&pool, "GW", 24, 1).await?;
    assert_eq!(top1.len(), 1);
    assert_eq!(top1[0].destination_crs, "BRI");

    Ok(())
}
