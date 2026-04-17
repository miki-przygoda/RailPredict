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
