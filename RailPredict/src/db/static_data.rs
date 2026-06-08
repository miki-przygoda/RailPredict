//! Tier A static data queries: stations, timetable calls, fares.
//!
//! All functions accept `&Db` (a `PgPool`) and are safe to call concurrently.
//! These tables are populated by the GTFS ingest command and refreshed weekly;
//! they are read-only at runtime.

use chrono::{NaiveDate, NaiveTime};
use sqlx::FromRow;

use super::Db;

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow)]
pub struct Station {
    pub crs: String,
    pub name: String,
    pub nlc: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub is_active: bool,
}

#[derive(Debug, Clone, FromRow)]
pub struct TimetableCall {
    pub id: i64,
    pub uid: String,
    pub operating_date: NaiveDate,
    pub location_crs: String,
    pub call_order: i16,
    pub scheduled_departure: Option<NaiveTime>,
    pub public_departure: Option<NaiveTime>,
    pub platform: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct Fare {
    pub origin_crs: String,
    pub destination_crs: String,
    pub fare_class: String,
    pub price_pence: i32,
    pub valid_from: NaiveDate,
    pub valid_to: Option<NaiveDate>,
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// Look up a station by its 3-letter CRS code.
pub async fn get_station(db: &Db, crs: &str) -> anyhow::Result<Option<Station>> {
    let station = sqlx::query_as::<_, Station>(
        "SELECT crs, name, nlc, lat, lon, is_active FROM stations WHERE crs = $1",
    )
    .bind(crs)
    .fetch_optional(db)
    .await?;
    Ok(station)
}

/// All scheduled departures from a station on a given operating date, ordered by
/// scheduled departure time. Used to populate the departure board.
pub async fn departures_from(
    db: &Db,
    crs: &str,
    date: NaiveDate,
) -> anyhow::Result<Vec<TimetableCall>> {
    let rows = sqlx::query_as::<_, TimetableCall>(
        "SELECT id, uid, operating_date, location_crs, call_order,
                scheduled_departure, public_departure, platform
         FROM timetable_calls
         WHERE location_crs = $1 AND operating_date = $2
         ORDER BY scheduled_departure NULLS LAST",
    )
    .bind(crs)
    .bind(date)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// The cheapest fare between two stations that is valid on `today`.
pub async fn cheapest_fare(
    db: &Db,
    origin_crs: &str,
    destination_crs: &str,
    today: NaiveDate,
) -> anyhow::Result<Option<Fare>> {
    let fare = sqlx::query_as::<_, Fare>(
        "SELECT origin_crs, destination_crs, fare_class, price_pence, valid_from, valid_to
         FROM fares
         WHERE origin_crs = $1
           AND destination_crs = $2
           AND valid_from <= $3
           AND (valid_to IS NULL OR valid_to >= $3)
         ORDER BY price_pence ASC
         LIMIT 1",
    )
    .bind(origin_crs)
    .bind(destination_crs)
    .bind(today)
    .fetch_optional(db)
    .await?;
    Ok(fare)
}

/// Direct (no-change) journeys from `from_crs` to `to_crs` on `date`: services whose
/// timetable call at the origin precedes a call at the destination (same uid + operating
/// date, higher `call_order`). Returns `(uid, scheduled_departure, platform)` ordered by
/// departure time; `limit` caps the rows (`None` = unbounded).
pub async fn direct_journeys(
    db: &Db,
    from_crs: &str,
    to_crs: &str,
    date: NaiveDate,
    limit: Option<i64>,
) -> sqlx::Result<Vec<(String, NaiveTime, Option<String>)>> {
    let mut sql = String::from(
        "SELECT tc_from.uid, tc_from.scheduled_departure, tc_from.platform \
         FROM timetable_calls tc_from \
         JOIN timetable_calls tc_to \
             ON tc_to.uid            = tc_from.uid \
            AND tc_to.operating_date = tc_from.operating_date \
            AND tc_to.location_crs   = $2 \
            AND tc_to.call_order     > tc_from.call_order \
         WHERE tc_from.location_crs  = $1 \
           AND tc_from.operating_date = $3 \
         ORDER BY tc_from.scheduled_departure",
    );
    // Static append of a bind placeholder — no user input is interpolated.
    if limit.is_some() {
        sql.push_str(" LIMIT $4");
    }
    let mut query = sqlx::query_as::<_, (String, NaiveTime, Option<String>)>(&sql)
        .bind(from_crs)
        .bind(to_crs)
        .bind(date);
    if let Some(n) = limit {
        query = query.bind(n);
    }
    query.fetch_all(db).await
}
