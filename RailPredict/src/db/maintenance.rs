//! DB maintenance utilities: pruning old rows and warm-up queries.
//!
//! - `prune_old_rows`: called daily to delete expired timetable and history rows.
//! - `load_todays_calls`: called at startup to pre-populate the registry from today's timetable.

use super::Db;

/// Delete timetable_calls older than 7 days and delay_history older than 91 days.
/// Returns (timetable_rows_deleted, history_rows_deleted).
pub async fn prune_old_rows(db: &Db) -> anyhow::Result<(u64, u64)> {
    let tc = sqlx::query(
        "DELETE FROM timetable_calls WHERE operating_date < CURRENT_DATE - INTERVAL '7 days'",
    )
    .execute(db)
    .await?;

    let dh = sqlx::query(
        "DELETE FROM delay_history WHERE recorded_at < NOW() - INTERVAL '91 days'",
    )
    .execute(db)
    .await?;

    Ok((tc.rows_affected(), dh.rows_affected()))
}

/// A single scheduled calling point from today's timetable.
#[derive(sqlx::FromRow)]
pub struct ScheduledCall {
    pub uid: String,
    pub location_crs: String,
    pub scheduled_departure: Option<chrono::NaiveTime>,
}

/// Load today's future scheduled departures from the DB timetable.
/// Used at startup to pre-warm the train registry with Tier A static data.
/// Returns an empty vec if the timetable is not yet populated.
pub async fn load_todays_calls(db: &Db) -> anyhow::Result<Vec<ScheduledCall>> {
    let rows = sqlx::query_as::<_, ScheduledCall>(
        "SELECT uid, location_crs, scheduled_departure
         FROM timetable_calls
         WHERE operating_date = CURRENT_DATE
           AND scheduled_departure > (NOW() AT TIME ZONE 'UTC')::time
         ORDER BY scheduled_departure",
    )
    .fetch_all(db)
    .await?;
    Ok(rows)
}
