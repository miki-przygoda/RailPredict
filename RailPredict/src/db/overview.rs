//! Read-only aggregate metrics for the overview cockpit (`/`).
//!
//! All queries bound on a rolling window of `hours` and apply the same delay
//! sanity filter (`delay_mins BETWEEN -120 AND 600`) used elsewhere, so the
//! severe-delay tail of the Darwin feed can't distort the headline figures.

use sqlx::FromRow;

use super::Db;

/// Headline figures for the KPI strip over a rolling window.
#[derive(Debug, Clone, Default, FromRow)]
pub struct HeadlineMetrics {
    pub on_time_pct: Option<f64>,
    pub avg_delay_mins: Option<f64>,
    pub mae_mins: Option<f64>,
    pub sample_count: i64,
}

/// One day's aggregates, for sparklines.
#[derive(Debug, Clone, FromRow)]
pub struct DailyPoint {
    pub on_time_pct: Option<f64>,
    pub avg_delay_mins: Option<f64>,
    pub mae_mins: Option<f64>,
}

/// Headline metrics over the last `hours` hours.
pub async fn headline_metrics(db: &Db, hours: i32) -> sqlx::Result<HeadlineMetrics> {
    sqlx::query_as::<_, HeadlineMetrics>(
        r#"
        SELECT
            (AVG(CASE WHEN delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8  AS on_time_pct,
            AVG(delay_mins::float8)                                              AS avg_delay_mins,
            (AVG(ABS(predicted_delay_mins - delay_mins))
                FILTER (WHERE predicted_delay_mins IS NOT NULL))::float8         AS mae_mins,
            COUNT(*)                                                            AS sample_count
        FROM delay_history
        WHERE recorded_at > NOW() - $1::INT * INTERVAL '1 hour'
          AND delay_mins BETWEEN -120 AND 600
        "#,
    )
    .bind(hours)
    .fetch_one(db)
    .await
}

/// Per-day aggregates over the last `hours` hours, ordered oldest → newest.
pub async fn daily_series(db: &Db, hours: i32) -> sqlx::Result<Vec<DailyPoint>> {
    sqlx::query_as::<_, DailyPoint>(
        r#"
        SELECT
            (AVG(CASE WHEN delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8  AS on_time_pct,
            AVG(delay_mins::float8)                                              AS avg_delay_mins,
            (AVG(ABS(predicted_delay_mins - delay_mins))
                FILTER (WHERE predicted_delay_mins IS NOT NULL))::float8         AS mae_mins
        FROM delay_history
        WHERE recorded_at > NOW() - $1::INT * INTERVAL '1 hour'
          AND delay_mins BETWEEN -120 AND 600
        GROUP BY date_trunc('day', recorded_at AT TIME ZONE 'Europe/London')
        ORDER BY date_trunc('day', recorded_at AT TIME ZONE 'Europe/London')
        "#,
    )
    .bind(hours)
    .fetch_all(db)
    .await
}

/// Range-independent data-coverage totals shown in the cockpit footer strip.
#[derive(Debug, Clone, Default, FromRow)]
pub struct CoverageCounts {
    pub stations: i64,
    pub real_records: i64,
    pub synthetic_records: i64,
}

/// Total station and delay-history row counts (all-time, not windowed).
pub async fn coverage_counts(db: &Db) -> sqlx::Result<CoverageCounts> {
    sqlx::query_as::<_, CoverageCounts>(
        r#"
        SELECT
            (SELECT COUNT(*) FROM stations)                  AS stations,
            (SELECT COUNT(*) FROM delay_history)             AS real_records,
            (SELECT COUNT(*) FROM delay_history_synthetic)   AS synthetic_records
        "#,
    )
    .fetch_one(db)
    .await
}
