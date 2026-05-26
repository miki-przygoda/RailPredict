//! Per-train predicted-vs-actual outcome ledger.
//!
//! Two write entry points:
//!   - `insert_first_prediction` — called from the ingestion pipeline the first time
//!     the engine produces a prediction for a given RID. `ON CONFLICT DO NOTHING`
//!     keeps the original prediction (not the latest one) so the outcome comparison
//!     is fair.
//!   - `finalise_outcome` — called when a train deactivates, writing the last-known
//!     reported delay into `final_delay_mins` / `finalised_at`.
//!
//! Two read entry points:
//!   - `recent_predictions` — feeds the `/demo/predictions` developer panel.
//!   - `prediction_for_rid` — surfaces the persisted snapshot on the train detail page.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;

use crate::types::{TrainId, TrainStatus};

use super::Db;

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct PredictionOutcome {
    pub rid: String,
    pub uid: String,
    pub origin_crs: String,
    pub destination_crs: Option<String>,
    pub scheduled_departure: DateTime<Utc>,
    pub predicted_delay_mins: i32,
    pub prediction_confidence: Option<f32>,
    pub correlation_preceding_rid: Option<String>,
    pub correlation_preceding_delay_mins: Option<i32>,
    pub predicted_at: DateTime<Utc>,
    pub final_delay_mins: Option<i32>,
    pub finalised_at: Option<DateTime<Utc>>,
}

impl PredictionOutcome {
    /// Absolute prediction error in minutes, once an outcome has been recorded.
    pub fn abs_error_mins(&self) -> Option<i32> {
        self.final_delay_mins
            .map(|actual| (actual - self.predicted_delay_mins).abs())
    }
}

/// Persist the engine's first prediction for this train instance.
///
/// Returns `Ok(true)` if a row was inserted, `Ok(false)` if a row already
/// existed for this RID or if required fields were missing.
pub async fn insert_first_prediction(db: &Db, status: &TrainStatus) -> sqlx::Result<bool> {
    let TrainId::Rid(rid) = &status.id else {
        return Ok(false);
    };
    let Some(uid) = status.uid.as_deref() else { return Ok(false); };
    let Some(origin) = status.origin_crs.as_deref() else { return Ok(false); };
    let Some(predicted) = status.predicted_delay_mins.value else { return Ok(false); };

    let (corr_rid, corr_delay) = match &status.volatility.correlation_signal {
        Some(sig) => (Some(sig.preceding_rid.as_str().to_string()), Some(sig.preceding_delay_mins)),
        None => (None, None),
    };

    let result = sqlx::query(
        r#"
        INSERT INTO prediction_outcomes
            (rid, uid, origin_crs, destination_crs, scheduled_departure,
             predicted_delay_mins, prediction_confidence,
             correlation_preceding_rid, correlation_preceding_delay_mins)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (rid) DO NOTHING
        "#,
    )
    .bind(rid)
    .bind(uid)
    .bind(origin)
    .bind(status.destination_crs.as_deref())
    .bind(status.scheduled_departure.value)
    .bind(predicted)
    .bind(status.volatility.historical_reliability)
    .bind(corr_rid)
    .bind(corr_delay)
    .execute(db)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Record the final observed delay for a train. Only updates rows that haven't
/// already been finalised — repeated calls are safe (no-op after the first).
///
/// Returns `Ok(true)` if a row was updated.
pub async fn finalise_outcome(
    db: &Db,
    rid: &str,
    final_delay_mins: i32,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        r#"
        UPDATE prediction_outcomes
        SET final_delay_mins = $1,
            finalised_at     = NOW()
        WHERE rid = $2
          AND finalised_at IS NULL
        "#,
    )
    .bind(final_delay_mins)
    .bind(rid)
    .execute(db)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Most recent predictions (finalised or not), ordered newest first.
/// Feeds the `/demo/predictions` developer panel.
pub async fn recent_predictions(db: &Db, limit: i64) -> sqlx::Result<Vec<PredictionOutcome>> {
    sqlx::query_as::<_, PredictionOutcome>(
        r#"
        SELECT rid, uid, origin_crs, destination_crs, scheduled_departure,
               predicted_delay_mins, prediction_confidence,
               correlation_preceding_rid, correlation_preceding_delay_mins,
               predicted_at, final_delay_mins, finalised_at
        FROM prediction_outcomes
        ORDER BY predicted_at DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Lookup the persisted prediction (and any final outcome) for a specific RID.
pub async fn prediction_for_rid(db: &Db, rid: &str) -> sqlx::Result<Option<PredictionOutcome>> {
    sqlx::query_as::<_, PredictionOutcome>(
        r#"
        SELECT rid, uid, origin_crs, destination_crs, scheduled_departure,
               predicted_delay_mins, prediction_confidence,
               correlation_preceding_rid, correlation_preceding_delay_mins,
               predicted_at, final_delay_mins, finalised_at
        FROM prediction_outcomes
        WHERE rid = $1
        "#,
    )
    .bind(rid)
    .fetch_optional(db)
    .await
}

/// Rolling accuracy summary across the last `window_hours` of finalised outcomes.
/// Powers the "current model accuracy" card on the dev panel.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AccuracySummary {
    pub finalised_count: i64,
    pub mean_abs_error_mins: Option<f64>,
    pub mean_predicted_mins: Option<f64>,
    pub mean_actual_mins: Option<f64>,
}

pub async fn accuracy_summary(db: &Db, window_hours: i32) -> sqlx::Result<AccuracySummary> {
    sqlx::query_as::<_, AccuracySummary>(
        r#"
        SELECT
            COUNT(*)                                                       AS finalised_count,
            AVG(ABS(final_delay_mins - predicted_delay_mins)::FLOAT8)      AS mean_abs_error_mins,
            AVG(predicted_delay_mins::FLOAT8)                              AS mean_predicted_mins,
            AVG(final_delay_mins::FLOAT8)                                  AS mean_actual_mins
        FROM prediction_outcomes
        WHERE finalised_at IS NOT NULL
          AND finalised_at >= NOW() - $1::INT * INTERVAL '1 hour'
        "#,
    )
    .bind(window_hours)
    .fetch_one(db)
    .await
}
