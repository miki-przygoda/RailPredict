//! Tier B delay history persistence.
//!
//! On startup: `load_history(db)` rebuilds the in-memory `HistoricalStore` from the
//! most recent MAX_SAMPLES rows per (uid, weekday, origin_crs, departure_hour) pattern.
//!
//! On flush: `flush_history(db, store)` batch-inserts new observations using
//! `ON CONFLICT DO NOTHING` so repeated flushes are idempotent.
//!
//! Chunk size is 500 rows per INSERT to stay well under PG's 65535 parameter limit.
//!
//! ## Phase 3 (AdvancedAnalytics): departure_hour column
//! The `ServicePattern` key now includes `departure_hour: u8`, so the window function
//! partition and the unique conflict index include it. Existing rows in DB will have
//! `departure_hour = 0` (the DEFAULT from the migration) until replaced by fresh
//! observations with correct hour values.

use std::sync::Arc;

use chrono::Weekday;
use sqlx::{FromRow, QueryBuilder};

use crate::prediction::types::{DelayRecord, HistoricalStore, ServicePattern, MAX_SAMPLES};

use super::Db;

const FLUSH_CHUNK: usize = 500;

// ---------------------------------------------------------------------------
// Weekday helpers
// ---------------------------------------------------------------------------

fn weekday_to_num(w: Weekday) -> i16 {
    w.num_days_from_monday() as i16
}

fn num_to_weekday(n: i16) -> Weekday {
    match n {
        0 => Weekday::Mon,
        1 => Weekday::Tue,
        2 => Weekday::Wed,
        3 => Weekday::Thu,
        4 => Weekday::Fri,
        5 => Weekday::Sat,
        _ => Weekday::Sun,
    }
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------

/// Rebuild a `HistoricalStore` from the database.
///
/// Uses a window function to select only the MAX_SAMPLES most recent rows per
/// (uid, weekday, origin_crs, departure_hour) pattern, so the returned store is
/// immediately ready for predictions.
#[derive(FromRow)]
struct DelayRow {
    uid: String,
    weekday: i16,
    origin_crs: String,
    departure_hour: i16,
    delay_mins: i32,
    predicted_delay_mins: Option<i32>,
    recorded_at: chrono::DateTime<chrono::Utc>,
}

pub async fn load_history(db: &Db) -> anyhow::Result<HistoricalStore> {
    let rows = sqlx::query_as::<_, DelayRow>(
        r#"
        SELECT uid, weekday, origin_crs, departure_hour, delay_mins, predicted_delay_mins, recorded_at
        FROM (
            SELECT uid, weekday, origin_crs, departure_hour, delay_mins, predicted_delay_mins, recorded_at,
                   ROW_NUMBER() OVER (
                       PARTITION BY uid, weekday, origin_crs, departure_hour
                       ORDER BY recorded_at DESC
                   ) AS rn
            FROM delay_history
        ) ranked
        WHERE rn <= $1
        ORDER BY uid, weekday, origin_crs, departure_hour, recorded_at ASC
        "#,
    )
    .bind(MAX_SAMPLES as i64)
    .fetch_all(db)
    .await?;

    let count = rows.len();
    let store = HistoricalStore::new();
    for row in rows {
        let pattern = ServicePattern {
            uid: row.uid.trim().to_string(),
            weekday: num_to_weekday(row.weekday),
            origin_crs: row.origin_crs.trim().to_string(),
            departure_hour: row.departure_hour.clamp(0, 23) as u8,
        };
        store.insert(
            pattern,
            DelayRecord {
                delay_mins: row.delay_mins,
                predicted_delay_mins: row.predicted_delay_mins,
                recorded_at: row.recorded_at,
            },
        );
    }

    tracing::info!(records = count, "Loaded delay history from DB");
    Ok(store)
}

// ---------------------------------------------------------------------------
// Flush
// ---------------------------------------------------------------------------

/// Write all records in `store` to the database, ignoring duplicates.
///
/// Called every 60 seconds from a background task and once more on SIGTERM.
/// Chunked at FLUSH_CHUNK rows per batch to stay within PG parameter limits.
pub async fn flush_history(db: &Db, store: &Arc<HistoricalStore>) -> anyhow::Result<()> {
    let records = store.all_records();
    if records.is_empty() {
        return Ok(());
    }

    // Phase 2: record flush duration and rows inserted.
    let flush_start = std::time::Instant::now();

    let mut inserted_total = 0usize;
    for chunk in records.chunks(FLUSH_CHUNK) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO delay_history (uid, weekday, origin_crs, departure_hour, delay_mins, predicted_delay_mins, recorded_at) ",
        );
        qb.push_values(chunk, |mut b, (pattern, record)| {
            b.push_bind(&pattern.uid)
                .push_bind(weekday_to_num(pattern.weekday))
                .push_bind(&pattern.origin_crs)
                .push_bind(pattern.departure_hour as i16)
                .push_bind(record.delay_mins)
                .push_bind(record.predicted_delay_mins)
                .push_bind(record.recorded_at);
        });
        qb.push(
            " ON CONFLICT (uid, weekday, origin_crs, departure_hour, recorded_at) DO NOTHING",
        );
        let result = qb.build().execute(db).await?;
        inserted_total += result.rows_affected() as usize;
    }

    // Phase 2: emit flush duration histogram and inserted row counter.
    let flush_duration_ms = flush_start.elapsed().as_millis() as f64;
    metrics::histogram!("db_flush_duration_ms").record(flush_duration_ms);
    metrics::counter!("db_flush_rows_inserted_total").increment(inserted_total as u64);

    if inserted_total > 0 {
        tracing::debug!(inserted = inserted_total, flush_duration_ms, "Flushed delay history to DB");
    }
    Ok(())
}
