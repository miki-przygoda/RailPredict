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
//!   - `recent_predictions` — feeds the `/dev/predictions` developer panel.
//!   - `prediction_for_rid` — surfaces the persisted snapshot on the train detail page.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value as JsonValue;
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

    let features = status.volatility.prediction_features.as_ref();

    let result = sqlx::query(
        r#"
        INSERT INTO prediction_outcomes
            (rid, uid, origin_crs, destination_crs, scheduled_departure,
             predicted_delay_mins, prediction_confidence,
             correlation_preceding_rid, correlation_preceding_delay_mins,
             features)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
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
    .bind(features.cloned())
    .execute(db)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Append a prediction snapshot — one row per significant prediction event per train.
/// Returns `Ok(())` on success; errors are non-fatal (logged by the caller).
pub async fn insert_snapshot(
    db: &Db,
    rid: &str,
    uid: &str,
    predicted_delay_mins: i32,
    features: Option<&JsonValue>,
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO prediction_snapshots (rid, uid, predicted_delay_mins, features)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(rid)
    .bind(uid)
    .bind(predicted_delay_mins)
    .bind(features.cloned())
    .execute(db)
    .await?;
    Ok(())
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
/// Feeds the `/dev/predictions` developer panel.
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
    /// Finalised predictions whose absolute error was within ±5 minutes.
    pub within_5_count: i64,
    pub mean_abs_error_mins: Option<f64>,
    pub mean_predicted_mins: Option<f64>,
    pub mean_actual_mins: Option<f64>,
}

pub async fn accuracy_summary(db: &Db, window_hours: i32) -> sqlx::Result<AccuracySummary> {
    sqlx::query_as::<_, AccuracySummary>(
        r#"
        SELECT
            COUNT(*)                                                       AS finalised_count,
            COUNT(*) FILTER (
                WHERE ABS(final_delay_mins - predicted_delay_mins) <= 5)   AS within_5_count,
            AVG(ABS(final_delay_mins - predicted_delay_mins)::FLOAT8)      AS mean_abs_error_mins,
            AVG(predicted_delay_mins::FLOAT8)                              AS mean_predicted_mins,
            AVG(final_delay_mins::FLOAT8)                                  AS mean_actual_mins
        FROM prediction_outcomes
        WHERE finalised_at IS NOT NULL
          AND finalised_at > NOW() - $1::INT * INTERVAL '1 hour'
          AND final_delay_mins BETWEEN -120 AND 600
        "#,
    )
    .bind(window_hours)
    .fetch_one(db)
    .await
}
/// A recently settled prediction — for the live board's "just settled" zone.
/// Operator name/brand fall back to NULL until `services.toc` is populated.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct SettledOutcome {
    pub rid: String,
    pub uid: String,
    pub operator: Option<String>,
    pub brand_color: Option<String>,
    pub origin_crs: String,
    pub destination_crs: Option<String>,
    pub predicted_delay_mins: i32,
    pub final_delay_mins: i32,
}

/// The most-recently finalised predictions, newest first, capped at `limit`.
pub async fn recent_settled(db: &Db, limit: i64) -> sqlx::Result<Vec<SettledOutcome>> {
    sqlx::query_as::<_, SettledOutcome>(
        r#"
        SELECT
            o.rid                   AS rid,
            o.uid                   AS uid,
            op.name                 AS operator,
            op.brand_color          AS brand_color,
            o.origin_crs            AS origin_crs,
            o.destination_crs       AS destination_crs,
            o.predicted_delay_mins  AS predicted_delay_mins,
            o.final_delay_mins      AS final_delay_mins
        FROM prediction_outcomes o
        LEFT JOIN services  s  ON s.uid = o.uid
        LEFT JOIN operators op ON op.toc = s.toc
        WHERE o.finalised_at IS NOT NULL
          AND o.final_delay_mins IS NOT NULL
          AND o.final_delay_mins BETWEEN -120 AND 600
        ORDER BY o.finalised_at DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

// ---------------------------------------------------------------------------
// prediction_snapshots read path
//
// `prediction_snapshots` accumulates MANY rows per RID — one per prediction
// event as the train progresses through its journey. Joining it against the
// finalised `prediction_outcomes` row lets us reconstruct how each prediction
// converged toward the eventual actual delay as departure approached.
// ---------------------------------------------------------------------------

/// One prediction event for a single train instance (RID).
///
/// Many of these accumulate per RID across the journey; ordered playback gives
/// the convergence trail.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct PredictionSnapshot {
    pub rid: String,
    pub uid: String,
    pub predicted_delay_mins: i32,
    pub snapshotted_at: DateTime<Utc>,
}

/// Every snapshot recorded for one RID, oldest first.
pub async fn snapshots_for_rid(db: &Db, rid: &str) -> sqlx::Result<Vec<PredictionSnapshot>> {
    sqlx::query_as::<_, PredictionSnapshot>(
        r#"
        SELECT rid, uid, predicted_delay_mins, snapshotted_at
        FROM prediction_snapshots
        WHERE rid = $1
        ORDER BY snapshotted_at ASC
        "#,
    )
    .bind(rid)
    .fetch_all(db)
    .await
}

/// A single point on a train's prediction-convergence curve: how far before
/// departure the prediction was made versus how wrong it turned out to be.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ConvergencePoint {
    /// Minutes before scheduled departure that this snapshot was taken.
    /// Positive and shrinking toward 0 as the train approaches departure.
    pub lead_time_mins: f64,
    pub predicted_delay_mins: i32,
    pub final_delay_mins: i32,
    pub abs_error_mins: i32,
}

/// Convergence trail for one RID: each snapshot joined against the finalised
/// outcome, ordered far-from-departure first (largest lead time → 0).
///
/// Only returns points once the RID's outcome has been finalised.
pub async fn convergence_for_rid(db: &Db, rid: &str) -> sqlx::Result<Vec<ConvergencePoint>> {
    sqlx::query_as::<_, ConvergencePoint>(
        r#"
        SELECT
            (EXTRACT(EPOCH FROM (o.scheduled_departure - s.snapshotted_at)) / 60.0)::float8
                                                              AS lead_time_mins,
            s.predicted_delay_mins                            AS predicted_delay_mins,
            o.final_delay_mins                                AS final_delay_mins,
            ABS(o.final_delay_mins - s.predicted_delay_mins)  AS abs_error_mins
        FROM prediction_snapshots s
        JOIN prediction_outcomes  o ON o.rid = s.rid
        WHERE s.rid = $1
          AND o.finalised_at IS NOT NULL
        ORDER BY lead_time_mins DESC
        "#,
    )
    .bind(rid)
    .fetch_all(db)
    .await
}

/// Mean absolute error grouped by how far before departure the prediction was
/// made. Lower `lower_bound_mins` band edges are 0/15/30/60/120.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct LeadTimeBucket {
    pub lower_bound_mins: i32,
    pub sample_count: i64,
    pub mae_mins: Option<f64>,
}

/// Lead-time accuracy across ALL finalised trains whose snapshots fall within
/// the last `hours` hours. Buckets each snapshot by minutes-before-departure and
/// reports mean abs error per band, ordered by ascending band edge.
///
/// A sanity filter drops outcomes outside `[-120, 600]` minutes.
pub async fn leadtime_accuracy(db: &Db, hours: i32) -> sqlx::Result<Vec<LeadTimeBucket>> {
    sqlx::query_as::<_, LeadTimeBucket>(
        r#"
        WITH points AS (
            SELECT
                CASE
                    WHEN EXTRACT(EPOCH FROM (o.scheduled_departure - s.snapshotted_at)) / 60.0 <  15  THEN 0
                    WHEN EXTRACT(EPOCH FROM (o.scheduled_departure - s.snapshotted_at)) / 60.0 <  30  THEN 15
                    WHEN EXTRACT(EPOCH FROM (o.scheduled_departure - s.snapshotted_at)) / 60.0 <  60  THEN 30
                    WHEN EXTRACT(EPOCH FROM (o.scheduled_departure - s.snapshotted_at)) / 60.0 < 120  THEN 60
                    ELSE 120
                END                                                       AS lower_bound_mins,
                ABS(o.final_delay_mins - s.predicted_delay_mins)::float8  AS abs_error_mins
            FROM prediction_snapshots s
            JOIN prediction_outcomes  o ON o.rid = s.rid
            WHERE o.finalised_at IS NOT NULL
              AND o.final_delay_mins BETWEEN -120 AND 600
              AND s.snapshotted_at > NOW() - $1::INT * INTERVAL '1 hour'
        )
        SELECT
            lower_bound_mins              AS lower_bound_mins,
            COUNT(*)                      AS sample_count,
            AVG(abs_error_mins)::float8   AS mae_mins
        FROM points
        GROUP BY lower_bound_mins
        ORDER BY lower_bound_mins ASC
        "#,
    )
    .bind(hours)
    .fetch_all(db)
    .await
}
