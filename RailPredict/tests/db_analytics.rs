//! Integration tests for `src/db/analytics.rs` — the `/predictions` explorer data layer.
//!
//! Each test gets a freshly migrated, isolated Postgres database via `#[sqlx::test]`,
//! so concurrent agents and concurrent test functions never collide. Rows are seeded
//! directly into `prediction_outcomes` with the runtime `sqlx::query` API (no compile-time
//! `query!`, so no offline `.sqlx/` snapshot is required).
//!
//! `prediction_outcomes` has no foreign keys, so seeding is a plain INSERT. `rid` is the
//! CHAR(15) primary key — every seeded row uses a unique, exactly-15-char id.

use railpredict::db::analytics::{
    accuracy_over_time, calibration_curve, confidence_error, error_distribution,
};

/// Insert one finalised prediction outcome. `rid` must be exactly 15 chars (PK).
/// `finalised_offset_days` shifts both `finalised_at` and `scheduled_departure` back
/// by N days so `accuracy_over_time` can be grouped across calendar days.
async fn seed(
    pool: &sqlx::PgPool,
    rid: &str,
    predicted: i32,
    final_delay: i32,
    confidence: Option<f32>,
    finalised_offset_days: i32,
) -> sqlx::Result<()> {
    assert_eq!(rid.len(), 15, "rid must be exactly CHAR(15)");
    sqlx::query(
        r#"
        INSERT INTO prediction_outcomes
            (rid, uid, origin_crs, destination_crs, scheduled_departure,
             predicted_delay_mins, prediction_confidence,
             final_delay_mins, finalised_at)
        VALUES
            ($1, 'W12345', 'KGX', 'EDB',
             NOW() - ($5::INT * INTERVAL '1 day'),
             $2, $3, $4,
             NOW() - ($5::INT * INTERVAL '1 day'))
        "#,
    )
    .bind(rid)
    .bind(predicted)
    .bind(confidence)
    .bind(final_delay)
    .bind(finalised_offset_days)
    .execute(pool)
    .await?;
    Ok(())
}

/// Insert an un-finalised prediction (no `final_delay_mins` / `finalised_at`).
/// Must be ignored by every analytics query.
async fn seed_open(pool: &sqlx::PgPool, rid: &str, predicted: i32) -> sqlx::Result<()> {
    assert_eq!(rid.len(), 15);
    sqlx::query(
        r#"
        INSERT INTO prediction_outcomes
            (rid, uid, origin_crs, scheduled_departure, predicted_delay_mins)
        VALUES ($1, 'W99999', 'KGX', NOW(), $2)
        "#,
    )
    .bind(rid)
    .bind(predicted)
    .execute(pool)
    .await?;
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
async fn calibration_buckets_and_mean_actual(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Two rows in the <=0 band (lower 0), one in the 6-15 band (lower 6).
    seed(&pool, "RID000000000001", 0, 2, Some(0.9), 0).await?;
    seed(&pool, "RID000000000002", -3, 4, Some(0.9), 0).await?;
    seed(&pool, "RID000000000003", 10, 12, Some(0.9), 0).await?;
    // Un-finalised row must be excluded.
    seed_open(&pool, "RID000000000099", 10).await?;

    let curve = calibration_curve(&pool, 24).await?;
    assert_eq!(curve.len(), 2, "expected two non-empty bands");

    let band0 = &curve[0];
    assert_eq!(band0.predicted_lower_mins, 0);
    assert_eq!(band0.sample_count, 2);
    // mean actual of {2, 4} = 3.0
    assert!((band0.mean_actual.unwrap() - 3.0).abs() < 1e-9);
    // mean predicted of {0, -3} = -1.5
    assert!((band0.mean_predicted.unwrap() - (-1.5)).abs() < 1e-9);

    let band6 = &curve[1];
    assert_eq!(band6.predicted_lower_mins, 6);
    assert_eq!(band6.sample_count, 1);
    assert!((band6.mean_actual.unwrap() - 12.0).abs() < 1e-9);

    // Ordered ascending by lower bound.
    assert!(curve[0].predicted_lower_mins < curve[1].predicted_lower_mins);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
async fn confidence_bands_and_mae(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Band 0.0 (conf 0.1): |10-5| = 5
    seed(&pool, "RID000000000010", 5, 10, Some(0.1), 0).await?;
    // Band 0.8 (conf 0.95): two rows, errors |3-0|=3 and |7-5|=2 -> mae 2.5
    seed(&pool, "RID000000000011", 0, 3, Some(0.95), 0).await?;
    seed(&pool, "RID000000000012", 5, 7, Some(0.9), 0).await?;
    // NULL confidence must be skipped entirely.
    seed(&pool, "RID000000000013", 5, 50, None, 0).await?;

    let buckets = confidence_error(&pool, 24).await?;
    assert_eq!(buckets.len(), 2, "NULL-confidence row excluded; two bands remain");

    let low = &buckets[0];
    assert!((low.confidence_lower - 0.0).abs() < 1e-9);
    assert_eq!(low.sample_count, 1);
    assert!((low.mae_mins.unwrap() - 5.0).abs() < 1e-9);

    let high = &buckets[1];
    assert!((high.confidence_lower - 0.8).abs() < 1e-9);
    assert_eq!(high.sample_count, 2);
    assert!((high.mae_mins.unwrap() - 2.5).abs() < 1e-9);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
async fn signed_error_distribution_separates_bias(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Over-prediction: predicted 20, actual 0 -> signed error -20 -> underflow band -999.
    seed(&pool, "RID000000000020", 20, 0, Some(0.5), 0).await?;
    // Under-prediction: predicted 0, actual 20 -> signed error +20 -> band lower 15.
    seed(&pool, "RID000000000021", 0, 20, Some(0.5), 0).await?;
    // Near-perfect: predicted 5, actual 6 -> signed error +1 -> centre band lower -2.
    seed(&pool, "RID000000000022", 5, 6, Some(0.5), 0).await?;

    let dist = error_distribution(&pool, 24).await?;

    let lower_bounds: Vec<i32> = dist.iter().map(|b| b.lower_bound_mins).collect();
    assert!(lower_bounds.contains(&-999), "over-prediction lands in underflow band");
    assert!(lower_bounds.contains(&15), "under-prediction lands in >=15 band");
    assert!(lower_bounds.contains(&-2), "near-perfect lands in centre band");

    // Each band here holds exactly one row, and ordering is ascending.
    for b in &dist {
        assert_eq!(b.sample_count, 1);
    }
    assert!(lower_bounds.windows(2).all(|w| w[0] < w[1]), "ordered ascending");
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
async fn accuracy_over_time_groups_by_day(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Yesterday: one row, error |10-4| = 6.
    seed(&pool, "RID000000000030", 4, 10, Some(0.7), 1).await?;
    // Today: two rows, errors |0-2|=2 and |6-4|=2 -> mae 2.0.
    seed(&pool, "RID000000000031", 2, 0, Some(0.7), 0).await?;
    seed(&pool, "RID000000000032", 4, 6, Some(0.7), 0).await?;

    let series = accuracy_over_time(&pool, 72).await?;
    assert_eq!(series.len(), 2, "two distinct calendar days");

    // Oldest -> newest.
    assert!(series[0].day < series[1].day);

    let yesterday = &series[0];
    assert_eq!(yesterday.sample_count, 1);
    assert!((yesterday.mae_mins.unwrap() - 6.0).abs() < 1e-9);

    let today = &series[1];
    assert_eq!(today.sample_count, 2);
    assert!((today.mae_mins.unwrap() - 2.0).abs() < 1e-9);
    // mean actual today {0, 6} = 3.0; mean predicted {2, 4} = 3.0
    assert!((today.mean_actual.unwrap() - 3.0).abs() < 1e-9);
    assert!((today.mean_predicted.unwrap() - 3.0).abs() < 1e-9);
    Ok(())
}

#[sqlx::test(migrations = "../migrations")]
async fn window_excludes_old_and_insane_rows(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // In-window, sane.
    seed(&pool, "RID000000000040", 5, 6, Some(0.5), 0).await?;
    // Out of the 1-hour window (finalised ~2 days ago).
    seed(&pool, "RID000000000041", 5, 6, Some(0.5), 2).await?;
    // Sanity-filter violation: final_delay 9999 > 600, in-window.
    seed(&pool, "RID000000000042", 5, 9999, Some(0.5), 0).await?;

    // 1-hour window: only the single fresh, sane row survives.
    let curve = calibration_curve(&pool, 1).await?;
    let total: i64 = curve.iter().map(|p| p.sample_count).sum();
    assert_eq!(total, 1, "old + insane rows excluded by window/sanity filter");
    Ok(())
}
