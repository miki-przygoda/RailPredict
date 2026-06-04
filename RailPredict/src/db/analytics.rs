//! Read-only prediction-accuracy analytics for the `/predictions` explorer.
//!
//! Reads finalised rows from `prediction_outcomes`. All queries are windowed on a
//! rolling `hours` bound and apply the same delay sanity filter used elsewhere
//! (`final_delay_mins BETWEEN -120 AND 600`) so the severe-delay tail of the
//! Darwin feed can't distort calibration and error figures.
//!
//! Four read functions back the `/predictions` explorer:
//!   - [`calibration_curve`]   — predicted-delay bands vs mean actual delay.
//!   - [`confidence_error`]    — mean absolute error per confidence band.
//!   - [`error_distribution`]  — histogram of *signed* error to reveal bias.
//!   - [`accuracy_over_time`]  — per-day MAE and means, oldest → newest.
//!
//! The outcomes ledger has no day-ahead/real-time discriminator column, so these
//! queries deliberately do **not** split by model variant.

use chrono::NaiveDate;
use sqlx::FromRow;

use super::Db;

/// One predicted-delay band: how the band's mean prediction compares to the
/// mean actual delay that materialised for those services.
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct CalibrationPoint {
    /// Lower edge (minutes) of the predicted-delay band this row aggregates.
    pub predicted_lower_mins: i32,
    /// Mean predicted delay within the band (NULL if the band is empty).
    pub mean_predicted: Option<f64>,
    /// Mean actual (finalised) delay within the band.
    pub mean_actual: Option<f64>,
    /// Number of finalised outcomes in the band.
    pub sample_count: i64,
}

/// One confidence band: mean absolute error of predictions whose model
/// reliability score falls in `[confidence_lower, confidence_lower + 0.2)`.
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct ConfidenceBucket {
    /// Lower edge of the confidence band (0.0, 0.2, 0.4, 0.6, 0.8).
    pub confidence_lower: f64,
    /// Mean absolute error (minutes) within the band.
    pub mae_mins: Option<f64>,
    /// Number of finalised, non-NULL-confidence outcomes in the band.
    pub sample_count: i64,
}

/// One signed-error histogram band, used to reveal systematic over/under-prediction.
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct ErrorBucket {
    /// Lower edge (minutes) of the signed-error band (`actual - predicted`).
    pub lower_bound_mins: i32,
    /// Number of finalised outcomes whose signed error falls in this band.
    pub sample_count: i64,
}

/// One calendar day (Europe/London) of accuracy aggregates.
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct AccuracyPoint {
    /// The local calendar day (Europe/London) the outcomes finalised on.
    pub day: NaiveDate,
    /// Mean absolute error (minutes) for the day.
    pub mae_mins: Option<f64>,
    /// Mean predicted delay for the day.
    pub mean_predicted: Option<f64>,
    /// Mean actual (finalised) delay for the day.
    pub mean_actual: Option<f64>,
    /// Number of finalised outcomes finalised on the day.
    pub sample_count: i64,
}

/// Calibration curve: bucket finalised rows by predicted delay and compare each
/// band's mean prediction against the mean actual delay.
///
/// Bands (lower edge on `predicted_delay_mins`): `<=0 → 0`, `1–5 → 1`,
/// `6–15 → 6`, `16–30 → 16`, `31–60 → 31`, `>60 → 61`. Ordered by `predicted_lower_mins`.
pub async fn calibration_curve(db: &Db, hours: i32) -> sqlx::Result<Vec<CalibrationPoint>> {
    sqlx::query_as::<_, CalibrationPoint>(
        r#"
        SELECT
            CASE
                WHEN predicted_delay_mins <= 0  THEN 0
                WHEN predicted_delay_mins <= 5  THEN 1
                WHEN predicted_delay_mins <= 15 THEN 6
                WHEN predicted_delay_mins <= 30 THEN 16
                WHEN predicted_delay_mins <= 60 THEN 31
                ELSE 61
            END                                  AS predicted_lower_mins,
            AVG(predicted_delay_mins::float8)    AS mean_predicted,
            AVG(final_delay_mins)::float8        AS mean_actual,
            COUNT(*)                             AS sample_count
        FROM prediction_outcomes
        WHERE finalised_at IS NOT NULL
          AND finalised_at > NOW() - $1::INT * INTERVAL '1 hour'
          AND final_delay_mins BETWEEN -120 AND 600
        GROUP BY predicted_lower_mins
        ORDER BY predicted_lower_mins
        "#,
    )
    .bind(hours)
    .fetch_all(db)
    .await
}

/// Confidence-vs-error: bucket finalised rows by `prediction_confidence` into
/// width-0.2 bands and report the mean absolute error per band.
///
/// Bands (lower edge): `0.0, 0.2, 0.4, 0.6, 0.8`. Rows with NULL confidence are
/// skipped. Ordered by `confidence_lower`.
pub async fn confidence_error(db: &Db, hours: i32) -> sqlx::Result<Vec<ConfidenceBucket>> {
    sqlx::query_as::<_, ConfidenceBucket>(
        r#"
        SELECT
            (CASE
                WHEN prediction_confidence < 0.2 THEN 0.0
                WHEN prediction_confidence < 0.4 THEN 0.2
                WHEN prediction_confidence < 0.6 THEN 0.4
                WHEN prediction_confidence < 0.8 THEN 0.6
                ELSE 0.8
            END)::float8                                              AS confidence_lower,
            AVG(ABS(final_delay_mins - predicted_delay_mins))::float8  AS mae_mins,
            COUNT(*)                                                  AS sample_count
        FROM prediction_outcomes
        WHERE finalised_at IS NOT NULL
          AND finalised_at > NOW() - $1::INT * INTERVAL '1 hour'
          AND final_delay_mins BETWEEN -120 AND 600
          AND prediction_confidence IS NOT NULL
        GROUP BY confidence_lower
        ORDER BY confidence_lower
        "#,
    )
    .bind(hours)
    .fetch_all(db)
    .await
}

/// Signed-error histogram: bucket finalised rows by `final_delay_mins -
/// predicted_delay_mins` to expose systematic bias.
///
/// Bands (lower edge): `<-15 → -999` (underflow), `[-15,-5) → -15`,
/// `[-5,-2) → -5`, `[-2,2) → -2`, `[2,5) → 2`, `[5,15) → 5`, `>=15 → 15`.
/// Negative signed error = over-prediction; positive = under-prediction.
/// Ordered by `lower_bound_mins`.
pub async fn error_distribution(db: &Db, hours: i32) -> sqlx::Result<Vec<ErrorBucket>> {
    sqlx::query_as::<_, ErrorBucket>(
        r#"
        SELECT
            CASE
                WHEN (final_delay_mins - predicted_delay_mins) < -15 THEN -999
                WHEN (final_delay_mins - predicted_delay_mins) <  -5 THEN -15
                WHEN (final_delay_mins - predicted_delay_mins) <  -2 THEN -5
                WHEN (final_delay_mins - predicted_delay_mins) <   2 THEN -2
                WHEN (final_delay_mins - predicted_delay_mins) <   5 THEN 2
                WHEN (final_delay_mins - predicted_delay_mins) <  15 THEN 5
                ELSE 15
            END             AS lower_bound_mins,
            COUNT(*)        AS sample_count
        FROM prediction_outcomes
        WHERE finalised_at IS NOT NULL
          AND finalised_at > NOW() - $1::INT * INTERVAL '1 hour'
          AND final_delay_mins BETWEEN -120 AND 600
        GROUP BY lower_bound_mins
        ORDER BY lower_bound_mins
        "#,
    )
    .bind(hours)
    .fetch_all(db)
    .await
}

/// Per-day accuracy: MAE, mean predicted and mean actual delay, grouped by the
/// local (Europe/London) calendar day the outcome finalised on. Ordered oldest → newest.
pub async fn accuracy_over_time(db: &Db, hours: i32) -> sqlx::Result<Vec<AccuracyPoint>> {
    sqlx::query_as::<_, AccuracyPoint>(
        r#"
        SELECT
            date_trunc('day', finalised_at AT TIME ZONE 'Europe/London')::date AS day,
            AVG(ABS(final_delay_mins - predicted_delay_mins))::float8           AS mae_mins,
            AVG(predicted_delay_mins::float8)                                  AS mean_predicted,
            AVG(final_delay_mins)::float8                                      AS mean_actual,
            COUNT(*)                                                           AS sample_count
        FROM prediction_outcomes
        WHERE finalised_at IS NOT NULL
          AND finalised_at > NOW() - $1::INT * INTERVAL '1 hour'
          AND final_delay_mins BETWEEN -120 AND 600
        GROUP BY day
        ORDER BY day
        "#,
    )
    .bind(hours)
    .fetch_all(db)
    .await
}
