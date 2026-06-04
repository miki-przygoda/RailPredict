//! Persisted cancellation observations.
//!
//! One row per cancelled service, written best-effort when a cancelled train
//! deactivates (see `ingestion/mod.rs`). The pattern fields (weekday, departure_hour)
//! are derived from the scheduled departure so cancellations can be sliced the same
//! way as `delay_history`. See `migrations/..._create_cancellations.sql`.

use chrono::{DateTime, Datelike, Timelike, Utc};

use super::Db;

/// Record one cancelled service. Derives weekday (0 = Mon) and departure hour from the
/// scheduled departure. Caller ensures the service was actually cancelled.
pub async fn record_cancellation(
    db: &Db,
    uid: &str,
    origin_crs: &str,
    scheduled_departure: DateTime<Utc>,
) -> sqlx::Result<()> {
    let weekday = scheduled_departure.weekday().num_days_from_monday() as i16;
    let hour = scheduled_departure.hour() as i16;
    sqlx::query(
        "INSERT INTO cancellations (uid, origin_crs, weekday, departure_hour)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(uid)
    .bind(origin_crs)
    .bind(weekday)
    .bind(hour)
    .execute(db)
    .await?;
    Ok(())
}

/// Count cancellations recorded in the last `hours` hours.
pub async fn count_recent(db: &Db, hours: i32) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM cancellations
         WHERE recorded_at > NOW() - $1::INT * INTERVAL '1 hour'",
    )
    .bind(hours)
    .fetch_one(db)
    .await
}
