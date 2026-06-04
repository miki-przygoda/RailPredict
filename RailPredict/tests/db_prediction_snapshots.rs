//! Integration tests for the `prediction_snapshots` read path.
//!
//! Exercises `snapshots_for_rid`, `convergence_for_rid`, and `leadtime_accuracy`
//! against a freshly migrated, isolated Postgres database (one per test, courtesy
//! of `#[sqlx::test]`).
//!
//! # Running locally
//!
//! ```bash
//! cd RailPredict
//! DATABASE_URL=postgresql://railpredict:railpredict@localhost:5432/railpredict \
//!   cargo test --test db_prediction_snapshots
//! ```
//!
//! Seed rows are inserted with the runtime `sqlx::query` API so no compile-time
//! database connection (or `.sqlx/` offline snapshot) is required.

use railpredict::db::predictions::{
    convergence_for_rid, leadtime_accuracy, snapshots_for_rid,
};

/// Insert a finalised outcome row for `rid` with the given scheduled departure
/// (expressed as an SQL interval relative to NOW) and final delay.
async fn seed_outcome(
    pool: &sqlx::PgPool,
    rid: &str,
    uid: &str,
    sched_dep_sql: &str,
    final_delay_mins: i32,
) -> sqlx::Result<()> {
    let sql = format!(
        r#"
        INSERT INTO prediction_outcomes
            (rid, uid, origin_crs, destination_crs, scheduled_departure,
             predicted_delay_mins, final_delay_mins, finalised_at)
        VALUES ($1, $2, 'LDS', 'KGX', {sched_dep_sql}, $3, $3, NOW())
        "#
    );
    sqlx::query(&sql)
        .bind(rid)
        .bind(uid)
        .bind(final_delay_mins)
        .execute(pool)
        .await?;
    Ok(())
}

/// Insert a single snapshot for `rid`, with `snapshotted_at` given as an SQL
/// expression relative to NOW (e.g. "NOW() - INTERVAL '90 minutes'").
async fn seed_snapshot(
    pool: &sqlx::PgPool,
    rid: &str,
    uid: &str,
    predicted_delay_mins: i32,
    snapshotted_at_sql: &str,
) -> sqlx::Result<()> {
    let sql = format!(
        r#"
        INSERT INTO prediction_snapshots
            (rid, uid, predicted_delay_mins, snapshotted_at)
        VALUES ($1, $2, $3, {snapshotted_at_sql})
        "#
    );
    sqlx::query(&sql)
        .bind(rid)
        .bind(uid)
        .bind(predicted_delay_mins)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// (a) snapshots_for_rid returns every snapshot for the RID, oldest first.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn snapshots_for_rid_orders_ascending(pool: sqlx::PgPool) -> sqlx::Result<()> {
    let rid = "202406040000001";
    let uid = "C12345";

    // Insert out of chronological order to prove ORDER BY does the work.
    seed_snapshot(&pool, rid, uid, 5, "NOW() - INTERVAL '30 minutes'").await?;
    seed_snapshot(&pool, rid, uid, 9, "NOW() - INTERVAL '90 minutes'").await?;
    seed_snapshot(&pool, rid, uid, 7, "NOW() - INTERVAL '60 minutes'").await?;
    // A snapshot for a different RID must not bleed in.
    seed_snapshot(&pool, "202406040000099", "C99999", 42, "NOW()").await?;

    let rows = snapshots_for_rid(&pool, rid).await?;
    assert_eq!(rows.len(), 3, "only this RID's snapshots");

    // Oldest first: 90m-ago, 60m-ago, 30m-ago → predicted 9, 7, 5.
    let predicted: Vec<i32> = rows.iter().map(|r| r.predicted_delay_mins).collect();
    assert_eq!(predicted, vec![9, 7, 5], "ascending by snapshotted_at");

    // snapshotted_at strictly increasing.
    for w in rows.windows(2) {
        assert!(w[0].snapshotted_at < w[1].snapshotted_at);
    }
    assert!(rows.iter().all(|r| r.rid.trim() == rid));
    Ok(())
}

// ---------------------------------------------------------------------------
// (b) convergence_for_rid joins snapshots ⋈ finalised outcome, computes
//     abs_error correctly, and orders by lead_time DESC (far → near).
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn convergence_for_rid_joins_and_orders(pool: sqlx::PgPool) -> sqlx::Result<()> {
    let rid = "202406040000002";
    let uid = "C22222";

    // Scheduled departure 10 minutes from now; final actual delay = 8 mins.
    seed_outcome(&pool, rid, uid, "NOW() + INTERVAL '10 minutes'", 8).await?;

    // Three snapshots at decreasing lead time (further → closer to departure).
    // lead_time = scheduled_departure - snapshotted_at.
    //   t-110m → lead ~120m, predicted 2 → abs_err |8-2| = 6
    //   t-50m  → lead ~ 60m, predicted 5 → abs_err |8-5| = 3
    //   t-5m   → lead ~ 15m, predicted 9 → abs_err |8-9| = 1
    seed_snapshot(&pool, rid, uid, 2, "NOW() - INTERVAL '110 minutes'").await?;
    seed_snapshot(&pool, rid, uid, 5, "NOW() - INTERVAL '50 minutes'").await?;
    seed_snapshot(&pool, rid, uid, 9, "NOW() - INTERVAL '5 minutes'").await?;

    let points = convergence_for_rid(&pool, rid).await?;
    assert_eq!(points.len(), 3, "one point per snapshot");

    // Ordered far-from-departure first → lead_time strictly descending.
    for w in points.windows(2) {
        assert!(
            w[0].lead_time_mins > w[1].lead_time_mins,
            "lead_time descending"
        );
    }

    // First point is the earliest snapshot (largest lead, ~120m).
    assert_eq!(points[0].predicted_delay_mins, 2);
    assert_eq!(points[0].final_delay_mins, 8);
    assert_eq!(points[0].abs_error_mins, 6);
    assert!((points[0].lead_time_mins - 120.0).abs() < 1.0);

    // Last point is the latest snapshot (smallest lead, ~15m).
    assert_eq!(points[2].predicted_delay_mins, 9);
    assert_eq!(points[2].abs_error_mins, 1);
    assert!((points[2].lead_time_mins - 15.0).abs() < 1.0);

    // Middle point.
    assert_eq!(points[1].abs_error_mins, 3);

    Ok(())
}

// ---------------------------------------------------------------------------
// (b') convergence excludes RIDs whose outcome is not yet finalised.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn convergence_excludes_unfinalised(pool: sqlx::PgPool) -> sqlx::Result<()> {
    let rid = "202406040000003";
    let uid = "C33333";

    // Outcome row exists but is NOT finalised (no final_delay_mins / finalised_at).
    sqlx::query(
        r#"
        INSERT INTO prediction_outcomes
            (rid, uid, origin_crs, scheduled_departure, predicted_delay_mins)
        VALUES ($1, $2, 'LDS', NOW() + INTERVAL '10 minutes', 4)
        "#,
    )
    .bind(rid)
    .bind(uid)
    .execute(&pool)
    .await?;

    seed_snapshot(&pool, rid, uid, 4, "NOW() - INTERVAL '30 minutes'").await?;

    let points = convergence_for_rid(&pool, rid).await?;
    assert!(points.is_empty(), "no convergence until finalised");
    Ok(())
}

// ---------------------------------------------------------------------------
// (c) leadtime_accuracy buckets snapshots into the correct band and computes
//     mean abs error per band.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn leadtime_accuracy_buckets_and_means(pool: sqlx::PgPool) -> sqlx::Result<()> {
    let rid = "202406040000004";
    let uid = "C44444";

    // Final actual delay = 10.
    seed_outcome(&pool, rid, uid, "NOW() + INTERVAL '10 minutes'", 10).await?;

    // Band [0,15): lead ~12m  (snapshot at t-2m), predicted 4 → abs_err 6
    seed_snapshot(&pool, rid, uid, 4, "NOW() - INTERVAL '2 minutes'").await?;
    // Band [30,60): lead ~50m (snapshot at t-40m), predicted 7 → abs_err 3
    seed_snapshot(&pool, rid, uid, 7, "NOW() - INTERVAL '40 minutes'").await?;
    // Band [120,∞): lead ~130m (snapshot at t-120m), two snapshots → mae averaged.
    //   predicted 0 → abs_err 10 ; predicted 6 → abs_err 4 ; mean = 7.0
    seed_snapshot(&pool, rid, uid, 0, "NOW() - INTERVAL '120 minutes'").await?;
    seed_snapshot(&pool, rid, uid, 6, "NOW() - INTERVAL '121 minutes'").await?;

    let buckets = leadtime_accuracy(&pool, 24).await?;

    // Bands present: 0, 30, 120 — ascending.
    let edges: Vec<i32> = buckets.iter().map(|b| b.lower_bound_mins).collect();
    assert_eq!(edges, vec![0, 30, 120], "ascending populated band edges");

    let band = |edge: i32| buckets.iter().find(|b| b.lower_bound_mins == edge).unwrap();

    let b0 = band(0);
    assert_eq!(b0.sample_count, 1);
    assert!((b0.mae_mins.unwrap() - 6.0).abs() < 1e-9);

    let b30 = band(30);
    assert_eq!(b30.sample_count, 1);
    assert!((b30.mae_mins.unwrap() - 3.0).abs() < 1e-9);

    let b120 = band(120);
    assert_eq!(b120.sample_count, 2);
    assert!((b120.mae_mins.unwrap() - 7.0).abs() < 1e-9);

    Ok(())
}

// ---------------------------------------------------------------------------
// (c') leadtime_accuracy honours the time window and the sanity filter.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn leadtime_accuracy_window_and_sanity_filter(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // RID A: finalised, in-window snapshot, sane final delay → counted.
    seed_outcome(&pool, "202406040000005", "C55555", "NOW() + INTERVAL '10 minutes'", 10).await?;
    seed_snapshot(&pool, "202406040000005", "C55555", 4, "NOW() - INTERVAL '2 minutes'").await?;

    // RID B: snapshot older than the 1-hour window → excluded by window.
    seed_outcome(&pool, "202406040000006", "C66666", "NOW() + INTERVAL '10 minutes'", 10).await?;
    seed_snapshot(&pool, "202406040000006", "C66666", 4, "NOW() - INTERVAL '5 hours'").await?;

    // RID C: insane final delay (700 > 600) → excluded by sanity filter.
    seed_outcome(&pool, "202406040000007", "C77777", "NOW() + INTERVAL '10 minutes'", 700).await?;
    seed_snapshot(&pool, "202406040000007", "C77777", 4, "NOW() - INTERVAL '2 minutes'").await?;

    let buckets = leadtime_accuracy(&pool, 1).await?;

    // Only RID A's single snapshot survives, in band [0,15).
    assert_eq!(buckets.len(), 1, "one populated band");
    assert_eq!(buckets[0].lower_bound_mins, 0);
    assert_eq!(buckets[0].sample_count, 1);
    assert!((buckets[0].mae_mins.unwrap() - 6.0).abs() < 1e-9);
    Ok(())
}
