//! Full-Journey Capture persistence.
//!
//! One `journeys` header row + N `journey_calls` rows per finalised service, written
//! best-effort when a train deactivates (see `ingestion/mod.rs`). Distinct from
//! `delay_history` (pattern-aggregated, feeds the live model): this is the whole-journey
//! "fat record" — arrival delay, per-stop vector, reason, operator, recovery — for
//! regression / relationship analysis and dashboards.
//!
//! Both inserts use `ON CONFLICT DO NOTHING` so a re-finalisation is idempotent, mirroring
//! `flush_history`. The header carries cheap rollups computed at assembly time; richer
//! cross-journey analytics are query-time.

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::QueryBuilder;

use super::Db;

/// Stay well under PG's 65535 bind-parameter limit (13 params per call row).
const CALLS_CHUNK: usize = 500;

/// The `journeys` header — one row per finalised service.
#[derive(Debug, Clone)]
pub struct JourneyRecord {
    pub rid: String,
    pub uid: String,
    pub ssd: NaiveDate,
    pub weekday: i16,
    pub departure_hour: i16,
    pub toc: Option<String>,
    pub train_category: Option<String>,
    pub origin_tpl: String,
    pub destination_tpl: Option<String>,
    pub scheduled_departure: DateTime<Utc>,
    pub actual_departure: Option<DateTime<Utc>>,
    pub origin_delay_mins: Option<i32>,
    pub arrival_delay_mins: Option<i32>,
    pub late_reason_code: Option<i16>,
    pub cancel_reason_code: Option<i16>,
    pub reason_tiploc: Option<String>,
    pub reason_class: i16,
    pub was_cancelled: bool,
    pub partial_cancel: bool,
    pub n_calls: i16,
    pub max_delay_mins: Option<i32>,
    pub min_delay_mins: Option<i32>,
    pub recovered_mins: Option<i32>,
    pub origin_platform: Option<String>,
    pub platform_confirmed: Option<bool>,
    pub wind_mph: Option<f32>,
}

/// One `journey_calls` row — a single stop along the journey.
#[derive(Debug, Clone)]
pub struct JourneyCallRecord {
    pub seq: i16,
    pub tpl: String,
    pub sched_arr: Option<DateTime<Utc>>,
    pub actual_arr: Option<DateTime<Utc>>,
    pub arr_delay_mins: Option<i32>,
    pub sched_dep: Option<DateTime<Utc>>,
    pub actual_dep: Option<DateTime<Utc>>,
    pub dep_delay_mins: Option<i32>,
    pub platform: Option<String>,
    pub plat_confirmed: Option<bool>,
    pub is_cancelled: bool,
    pub dwell_secs: Option<i32>,
}

/// Persist a finalised journey: one header insert + a batched calls insert. Idempotent.
pub async fn insert_journey(
    db: &Db,
    journey: &JourneyRecord,
    calls: &[JourneyCallRecord],
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO journeys (
            rid, uid, ssd, weekday, departure_hour, toc, train_category,
            origin_tpl, destination_tpl, scheduled_departure, actual_departure,
            origin_delay_mins, arrival_delay_mins,
            late_reason_code, cancel_reason_code, reason_tiploc, reason_class,
            was_cancelled, partial_cancel, n_calls,
            max_delay_mins, min_delay_mins, recovered_mins,
            origin_platform, platform_confirmed, wind_mph
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7,
            $8, $9, $10, $11,
            $12, $13,
            $14, $15, $16, $17,
            $18, $19, $20,
            $21, $22, $23,
            $24, $25, $26
        )
        ON CONFLICT (rid) DO NOTHING
        "#,
    )
    .bind(&journey.rid)
    .bind(&journey.uid)
    .bind(journey.ssd)
    .bind(journey.weekday)
    .bind(journey.departure_hour)
    .bind(journey.toc.as_deref())
    .bind(journey.train_category.as_deref())
    .bind(&journey.origin_tpl)
    .bind(journey.destination_tpl.as_deref())
    .bind(journey.scheduled_departure)
    .bind(journey.actual_departure)
    .bind(journey.origin_delay_mins)
    .bind(journey.arrival_delay_mins)
    .bind(journey.late_reason_code)
    .bind(journey.cancel_reason_code)
    .bind(journey.reason_tiploc.as_deref())
    .bind(journey.reason_class)
    .bind(journey.was_cancelled)
    .bind(journey.partial_cancel)
    .bind(journey.n_calls)
    .bind(journey.max_delay_mins)
    .bind(journey.min_delay_mins)
    .bind(journey.recovered_mins)
    .bind(journey.origin_platform.as_deref())
    .bind(journey.platform_confirmed)
    .bind(journey.wind_mph)
    .execute(db)
    .await?;

    for chunk in calls.chunks(CALLS_CHUNK) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO journey_calls (rid, seq, tpl, sched_arr, actual_arr, arr_delay_mins, \
             sched_dep, actual_dep, dep_delay_mins, platform, plat_confirmed, is_cancelled, dwell_secs) ",
        );
        qb.push_values(chunk, |mut b, c| {
            b.push_bind(&journey.rid)
                .push_bind(c.seq)
                .push_bind(&c.tpl)
                .push_bind(c.sched_arr)
                .push_bind(c.actual_arr)
                .push_bind(c.arr_delay_mins)
                .push_bind(c.sched_dep)
                .push_bind(c.actual_dep)
                .push_bind(c.dep_delay_mins)
                .push_bind(c.platform.as_deref())
                .push_bind(c.plat_confirmed)
                .push_bind(c.is_cancelled)
                .push_bind(c.dwell_secs);
        });
        qb.push(" ON CONFLICT (rid, seq) DO NOTHING");
        qb.build().execute(db).await?;
    }

    Ok(())
}

/// Count journeys captured in the last `hours` hours (for ops/health visibility).
pub async fn count_recent(db: &Db, hours: i32) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM journeys WHERE finalised_at > NOW() - $1::INT * INTERVAL '1 hour'",
    )
    .bind(hours)
    .fetch_one(db)
    .await
}
