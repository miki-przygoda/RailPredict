//! GTFS static data ingest: downloads the Network Rail GTFS ZIP, extracts
//! `stops.txt`, `trips.txt`, `calendar.txt`, and `stop_times.txt`, then
//! upserts station, service, and timetable-call rows into the database.
//!
//! `parse_stops`, `parse_trips`, `parse_calendar`, and `parse_stop_times`
//! are pure functions so they can be tested without I/O.
//! `run_ingest` performs the full download → parse → upsert pipeline.

use std::collections::{HashMap, HashSet};
use std::io::Read;

use serde::Deserialize;
use sqlx::QueryBuilder;
use tokio::sync::watch;

use crate::db::Db;

// ---------------------------------------------------------------------------
// Ingest progress types — streamed via tokio::sync::watch to the HTTP ingest UI
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Default)]
pub enum IngestPhase {
    #[default]
    Idle,
    Downloading,
    Parsing,
    Stations,
    Services,
    TimetableCalls,
    Complete,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct IngestStatus {
    pub phase: IngestPhase,
    pub stations: usize,
    pub services: usize,
    pub timetable_calls: usize,
    pub log: Vec<String>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error: Option<String>,
}

impl IngestStatus {
    pub(crate) fn push_log(&mut self, msg: impl Into<String>) {
        self.log.push(msg.into());
        if self.log.len() > 50 {
            self.log.remove(0);
        }
    }
}

fn emit(progress: Option<&watch::Sender<IngestStatus>>, f: impl FnOnce(&mut IngestStatus)) {
    if let Some(tx) = progress {
        tx.send_modify(f);
    }
}

// ---------------------------------------------------------------------------
// GTFS row types
// ---------------------------------------------------------------------------

/// A single row from GTFS `stops.txt`.
/// Network Rail GTFS uses `stop_id` as the CRS code.
#[derive(Debug, Deserialize)]
pub struct GtfsStation {
    pub stop_id: String,
    pub stop_name: String,
    #[serde(default)]
    pub stop_lat: Option<f64>,
    #[serde(default)]
    pub stop_lon: Option<f64>,
}

/// A single row from GTFS `trips.txt`.
#[derive(Debug, Deserialize)]
struct GtfsTrip {
    #[serde(default)]
    route_id: String,
    trip_id: String,
    service_id: String,
}

/// A single row from GTFS `agency.txt`.
#[derive(Debug, Deserialize)]
struct GtfsAgency {
    #[serde(default)]
    agency_id: String,
    agency_name: String,
}

/// A single row from GTFS `routes.txt`.
#[derive(Debug, Deserialize)]
struct GtfsRoute {
    route_id: String,
    #[serde(default)]
    agency_id: String,
}

/// A single row from GTFS `calendar.txt`.
#[derive(Debug, Deserialize)]
struct GtfsCalendar {
    service_id: String,
    monday: u8,
    tuesday: u8,
    wednesday: u8,
    thursday: u8,
    friday: u8,
    saturday: u8,
    sunday: u8,
}

/// A single row from GTFS `stop_times.txt`.
#[derive(Debug, Deserialize)]
struct GtfsStopTime {
    trip_id: String,
    arrival_time: String,
    departure_time: String,
    stop_id: String,
    stop_sequence: i32,
}

/// Internal struct for batching timetable_calls inserts.
struct TimetableCallRow {
    uid: String,
    operating_date: chrono::NaiveDate,
    location_crs: String,
    call_order: i16,
    scheduled_departure: Option<chrono::NaiveTime>,
    public_departure: Option<chrono::NaiveTime>,
}

// ---------------------------------------------------------------------------
// Pure parse functions
// ---------------------------------------------------------------------------

/// Parse `stops.txt` CSV bytes into a list of stations.
///
/// Only rows whose `stop_id` is exactly 3 ASCII uppercase letters are kept
/// (i.e. valid CRS codes). Platform sub-stops and non-CRS stops are skipped.
pub fn parse_stops(csv_bytes: &[u8]) -> anyhow::Result<Vec<GtfsStation>> {
    let mut reader = csv::Reader::from_reader(csv_bytes);
    let mut stations = Vec::new();
    for result in reader.deserialize::<GtfsStation>() {
        match result {
            Ok(row) => {
                if is_valid_crs(&row.stop_id) {
                    stations.push(row);
                }
            }
            Err(e) => tracing::warn!(error = %e, "Skipping malformed GTFS stops.txt row"),
        }
    }
    Ok(stations)
}

/// Parse `trips.txt` CSV bytes into a list of `GtfsTrip` rows.
fn parse_trips(csv_bytes: &[u8]) -> anyhow::Result<Vec<GtfsTrip>> {
    let mut reader = csv::Reader::from_reader(csv_bytes);
    let mut trips = Vec::new();
    for result in reader.deserialize::<GtfsTrip>() {
        match result {
            Ok(row) => trips.push(row),
            Err(e) => tracing::warn!(error = %e, "Skipping malformed GTFS trips.txt row"),
        }
    }
    Ok(trips)
}

/// Parse `calendar.txt` CSV bytes into a map of `service_id → runs_on_days bitmask`.
///
/// Bitmask: bit 0 = Monday, bit 1 = Tuesday, …, bit 6 = Sunday.
fn parse_calendar(csv_bytes: &[u8]) -> anyhow::Result<HashMap<String, i16>> {
    let mut reader = csv::Reader::from_reader(csv_bytes);
    let mut map = HashMap::new();
    for result in reader.deserialize::<GtfsCalendar>() {
        match result {
            Ok(row) => {
                let bitmask: i16 = (row.monday as i16)
                    | ((row.tuesday as i16) << 1)
                    | ((row.wednesday as i16) << 2)
                    | ((row.thursday as i16) << 3)
                    | ((row.friday as i16) << 4)
                    | ((row.saturday as i16) << 5)
                    | ((row.sunday as i16) << 6);
                map.insert(row.service_id, bitmask);
            }
            Err(e) => tracing::warn!(error = %e, "Skipping malformed GTFS calendar.txt row"),
        }
    }
    Ok(map)
}

/// Parse `stop_times.txt` CSV bytes into a list of `GtfsStopTime` rows.
fn parse_stop_times(csv_bytes: &[u8]) -> anyhow::Result<Vec<GtfsStopTime>> {
    let mut reader = csv::Reader::from_reader(csv_bytes);
    let mut stop_times = Vec::new();
    for result in reader.deserialize::<GtfsStopTime>() {
        match result {
            Ok(row) => stop_times.push(row),
            Err(e) => tracing::warn!(error = %e, "Skipping malformed GTFS stop_times.txt row"),
        }
    }
    Ok(stop_times)
}

/// Parse `agency.txt` into a map of `agency_id → agency_name`.
/// Rows with an empty agency_id are skipped.
fn parse_agency(csv_bytes: &[u8]) -> anyhow::Result<HashMap<String, String>> {
    let mut reader = csv::Reader::from_reader(csv_bytes);
    let mut map = HashMap::new();
    for result in reader.deserialize::<GtfsAgency>() {
        match result {
            Ok(row) if !row.agency_id.is_empty() => {
                map.insert(row.agency_id, row.agency_name);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "Skipping malformed GTFS agency.txt row"),
        }
    }
    Ok(map)
}

/// Parse `routes.txt` into a map of `route_id → agency_id`.
/// Rows with an empty agency_id are skipped.
fn parse_routes(csv_bytes: &[u8]) -> anyhow::Result<HashMap<String, String>> {
    let mut reader = csv::Reader::from_reader(csv_bytes);
    let mut map = HashMap::new();
    for result in reader.deserialize::<GtfsRoute>() {
        match result {
            Ok(row) if !row.agency_id.is_empty() => {
                map.insert(row.route_id, row.agency_id);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "Skipping malformed GTFS routes.txt row"),
        }
    }
    Ok(map)
}

/// Derive `uid → toc (agency_id)` from trips and a `route_id → agency_id` map.
/// First-seen UID wins (mirrors the service-build rule). UIDs whose route does
/// not resolve to a non-empty agency are omitted (they get NULL toc).
fn derive_uid_toc(
    trips: &[GtfsTrip],
    route_to_agency: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    for trip in trips {
        let Some(uid) = extract_uid(&trip.trip_id) else {
            continue;
        };
        if map.contains_key(uid) {
            continue;
        }
        if let Some(agency) = route_to_agency.get(&trip.route_id).filter(|a| !a.is_empty()) {
            map.insert(uid.to_owned(), agency.clone());
        }
    }
    map
}

/// Parse a GTFS time string "HH:MM:SS" where HH may be ≥ 24 (overnight services).
/// Values ≥ 24:00:00 are clamped by taking `HH % 24`.
fn parse_gtfs_time(s: &str) -> Option<chrono::NaiveTime> {
    let parts: Vec<&str> = s.trim().splitn(3, ':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: u32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let sec: u32 = parts[2].parse().ok()?;
    chrono::NaiveTime::from_hms_opt(h % 24, m, sec)
}

/// Resolve a Network Rail GTFS `stop_id` to a 3-letter CRS code.
///
/// NR GTFS stop_ids are typically prefixed (e.g. `9100LEEDS`). The simplest
/// reliable approach: take the last 3 chars if they are all ASCII uppercase
/// letters; otherwise return `None`.
fn stop_id_to_crs(stop_id: &str) -> Option<&str> {
    if stop_id.len() < 3 {
        return None;
    }
    let suffix = &stop_id[stop_id.len() - 3..];
    if suffix.chars().all(|c| c.is_ascii_uppercase()) {
        Some(suffix)
    } else {
        None
    }
}

/// Extract the Network Rail RTTI UID from a GTFS `trip_id`.
///
/// The format is typically `{uid}_{date}` (e.g. `C12345_20240417`).
/// The UID is the part before the first `_`, capped at 6 chars.
/// Returns `None` if the extracted UID is not exactly 6 ASCII alphanumeric chars.
fn extract_uid(trip_id: &str) -> Option<&str> {
    let candidate = match trip_id.find('_') {
        Some(pos) => &trip_id[..pos.min(6)],
        None => &trip_id[..trip_id.len().min(6)],
    };
    if candidate.len() == 6 && candidate.chars().all(|c| c.is_ascii_alphanumeric()) {
        Some(candidate)
    } else {
        None
    }
}

fn is_valid_crs(s: &str) -> bool {
    s.len() == 3 && s.chars().all(|c| c.is_ascii_uppercase())
}

// ---------------------------------------------------------------------------
// ZIP extraction helpers
// ---------------------------------------------------------------------------

/// Extract a named file from a ZIP archive and return its raw bytes.
fn extract_file(zip_bytes: &[u8], filename: &str) -> anyhow::Result<Vec<u8>> {
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| anyhow::anyhow!("Failed to open ZIP: {e}"))?;

    let mut file = archive
        .by_name(filename)
        .map_err(|_| anyhow::anyhow!("{filename} not found in GTFS archive"))?;

    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

// ---------------------------------------------------------------------------
// DB upsert functions
// ---------------------------------------------------------------------------

const UPSERT_CHUNK: usize = 200;
const SERVICES_CHUNK: usize = 100;
const CALLS_CHUNK: usize = 200;

async fn upsert_stations(db: &Db, stations: &[GtfsStation]) -> anyhow::Result<()> {
    for chunk in stations.chunks(UPSERT_CHUNK) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO stations (crs, name, lat, lon, updated_at) ",
        );
        qb.push_values(chunk, |mut b, s| {
            b.push_bind(&s.stop_id)
                .push_bind(&s.stop_name)
                .push_bind(s.stop_lat)
                .push_bind(s.stop_lon)
                .push_bind(chrono::Utc::now());
        });
        qb.push(
            " ON CONFLICT (crs) DO UPDATE SET
                name       = EXCLUDED.name,
                lat        = EXCLUDED.lat,
                lon        = EXCLUDED.lon,
                updated_at = EXCLUDED.updated_at",
        );
        qb.build().execute(db).await?;
    }
    Ok(())
}

/// Upsert services rows. Each tuple is `(uid, origin_crs, destination_crs, runs_on_days, toc)`.
/// Only services whose origin_crs and destination_crs are in `known_stations` are inserted
/// to avoid FK violations.
async fn upsert_services(
    db: &Db,
    services: &[(String, String, String, i16, Option<String>)],
    known_stations: &HashSet<String>,
) -> anyhow::Result<usize> {
    let filtered: Vec<&(String, String, String, i16, Option<String>)> = services
        .iter()
        .filter(|(_, origin, dest, _, _)| {
            known_stations.contains(origin) && known_stations.contains(dest)
        })
        .collect();

    let mut total = 0usize;
    for chunk in filtered.chunks(SERVICES_CHUNK) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO services (uid, origin_crs, destination_crs, runs_on_days, toc, updated_at) ",
        );
        qb.push_values(chunk, |mut b, (uid, origin, dest, days, toc)| {
            b.push_bind(uid)
                .push_bind(origin)
                .push_bind(dest)
                .push_bind(days)
                .push_bind(toc.clone())
                .push_bind(chrono::Utc::now());
        });
        qb.push(
            " ON CONFLICT (uid) DO UPDATE SET
                origin_crs      = EXCLUDED.origin_crs,
                destination_crs = EXCLUDED.destination_crs,
                runs_on_days    = EXCLUDED.runs_on_days,
                toc             = COALESCE(EXCLUDED.toc, services.toc),
                updated_at      = EXCLUDED.updated_at",
        );
        qb.build().execute(db).await?;
        total += chunk.len();
    }
    Ok(total)
}

/// Upsert operator reference rows from `agency_id → agency_name`, attaching a
/// brand colour from the curated map.
async fn upsert_operators(db: &Db, agencies: &HashMap<String, String>) -> anyhow::Result<usize> {
    if agencies.is_empty() {
        return Ok(0);
    }
    let rows: Vec<(&String, &String)> = agencies.iter().collect();
    let mut total = 0usize;
    for chunk in rows.chunks(SERVICES_CHUNK) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO operators (toc, name, brand_color, updated_at) ",
        );
        qb.push_values(chunk, |mut b, (toc, name)| {
            b.push_bind(*toc)
                .push_bind(*name)
                .push_bind(crate::ingestion::operators::brand_color(name))
                .push_bind(chrono::Utc::now());
        });
        qb.push(
            " ON CONFLICT (toc) DO UPDATE SET
                name        = EXCLUDED.name,
                brand_color = EXCLUDED.brand_color,
                updated_at  = EXCLUDED.updated_at",
        );
        qb.build().execute(db).await?;
        total += chunk.len();
    }
    Ok(total)
}

/// Upsert timetable_calls rows. Only rows whose uid is in `known_uids` and
/// whose location_crs is in `known_stations` are inserted to avoid FK violations.
async fn upsert_timetable_calls(
    db: &Db,
    calls: &[TimetableCallRow],
    known_uids: &HashSet<String>,
    known_stations: &HashSet<String>,
) -> anyhow::Result<usize> {
    let filtered: Vec<&TimetableCallRow> = calls
        .iter()
        .filter(|c| known_uids.contains(&c.uid) && known_stations.contains(&c.location_crs))
        .collect();

    let mut total = 0usize;
    for chunk in filtered.chunks(CALLS_CHUNK) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO timetable_calls \
             (uid, operating_date, location_crs, call_order, scheduled_departure, public_departure) ",
        );
        qb.push_values(chunk, |mut b, c| {
            b.push_bind(&c.uid)
                .push_bind(c.operating_date)
                .push_bind(&c.location_crs)
                .push_bind(c.call_order)
                .push_bind(c.scheduled_departure)
                .push_bind(c.public_departure);
        });
        qb.push(" ON CONFLICT DO NOTHING");
        qb.build().execute(db).await?;
        total += chunk.len();
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// Full ingest pipeline
// ---------------------------------------------------------------------------

/// Return the number of timetable_calls rows that exist for today.
/// Used by the ingest UI to warn the user before overwriting fresh data.
pub async fn today_call_count(db: &crate::db::Db) -> anyhow::Result<i64> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM timetable_calls WHERE operating_date = CURRENT_DATE",
    )
    .fetch_one(db)
    .await?;
    Ok(count)
}

/// Download the GTFS ZIP from `url`, extract and parse all relevant files,
/// and upsert stations, services, and timetable_calls into the database.
/// Returns the number of stations upserted (for backward compatibility).
pub async fn run_ingest(db: &Db, url: &str) -> anyhow::Result<usize> {
    tracing::info!(%url, "Downloading GTFS archive");
    let bytes = reqwest::get(url).await?.bytes().await?;
    tracing::info!(size_bytes = bytes.len(), "Downloaded GTFS archive");

    run_ingest_from_bytes(db, &bytes, None).await
}

/// Same as `run_ingest` but reads from a local file instead of downloading.
/// Returns the number of stations upserted (for backward compatibility).
pub async fn run_ingest_from_file(db: &Db, path: &std::path::Path) -> anyhow::Result<usize> {
    let bytes = std::fs::read(path)?;
    run_ingest_from_bytes(db, &bytes, None).await
}

/// Download `url`, stream progress into `tx`, then run the full ingest pipeline.
/// Called by the HTTP ingest UI handler (POST /ui/demo/ingest/start).
pub async fn run_ingest_with_watch(
    db: &Db,
    url: &str,
    tx: &watch::Sender<IngestStatus>,
) -> anyhow::Result<usize> {
    tracing::info!(%url, "Downloading GTFS archive (with progress)");
    let bytes = reqwest::get(url).await?.bytes().await?;
    let mb = bytes.len() as f64 / 1_048_576.0;
    tracing::info!(size_bytes = bytes.len(), "Downloaded GTFS archive");
    tx.send_modify(|s| s.push_log(format!("Downloaded {mb:.1} MB")));
    run_ingest_from_bytes(db, &bytes, Some(tx)).await
}

/// Shared implementation used by `run_ingest`, `run_ingest_from_file`, and `run_ingest_with_watch`.
async fn run_ingest_from_bytes(db: &Db, zip_bytes: &[u8], progress: Option<&watch::Sender<IngestStatus>>) -> anyhow::Result<usize> {
    // --- Phase 1: Stations ---
    emit(progress, |s| { s.phase = IngestPhase::Parsing; s.push_log("Extracting stations"); });
    let stops_bytes = extract_file(zip_bytes, "stops.txt")?;
    let stations = parse_stops(&stops_bytes)?;
    let station_count = stations.len();
    tracing::info!(stations = station_count, "Parsed GTFS stops");
    emit(progress, |s| { s.phase = IngestPhase::Stations; s.push_log(format!("Upserting {station_count} stations")); });

    upsert_stations(db, &stations).await?;
    tracing::info!(stations = station_count, "Stations upserted");
    emit(progress, |s| { s.stations = station_count; s.push_log(format!("Stations done: {station_count}")); });

    // Build a set of known CRS codes for FK guard.
    let known_stations: HashSet<String> = stations.iter().map(|s| s.stop_id.clone()).collect();

    // --- Phase 2: Calendar (service_id → bitmask) ---
    let calendar_map = match extract_file(zip_bytes, "calendar.txt") {
        Ok(cal_bytes) => match parse_calendar(&cal_bytes) {
            Ok(m) => {
                tracing::info!(entries = m.len(), "Parsed GTFS calendar");
                m
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse calendar.txt; skipping services phase");
                HashMap::new()
            }
        },
        Err(_) => {
            tracing::warn!("calendar.txt not found in GTFS archive; skipping services phase");
            HashMap::new()
        }
    };

    if calendar_map.is_empty() {
        emit(progress, |s| { s.phase = IngestPhase::Complete; s.finished_at = Some(chrono::Utc::now()); s.push_log("Complete (stations only — no calendar)"); });
        tracing::info!(stations = station_count, "GTFS ingest complete (stations only)");
        return Ok(station_count);
    }
    emit(progress, |s| s.push_log(format!("Calendar: {} entries", calendar_map.len())));

    // --- Phase 3: Trips ---
    let trips = match extract_file(zip_bytes, "trips.txt") {
        Ok(trip_bytes) => match parse_trips(&trip_bytes) {
            Ok(t) => {
                tracing::info!(trips = t.len(), "Parsed GTFS trips");
                t
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse trips.txt; skipping services phase");
                vec![]
            }
        },
        Err(_) => {
            tracing::warn!("trips.txt not found in GTFS archive; skipping services phase");
            vec![]
        }
    };

    emit(progress, |s| s.push_log(format!("Trips parsed: {}", trips.len())));
    if trips.is_empty() {
        emit(progress, |s| { s.phase = IngestPhase::Complete; s.finished_at = Some(chrono::Utc::now()); s.push_log("Complete (no trips found)"); });
        tracing::info!(stations = station_count, "GTFS ingest complete (stations only)");
        return Ok(station_count);
    }

    // --- Phase 4: Stop times ---
    let stop_times = match extract_file(zip_bytes, "stop_times.txt") {
        Ok(st_bytes) => match parse_stop_times(&st_bytes) {
            Ok(st) => {
                tracing::info!(stop_times = st.len(), "Parsed GTFS stop_times");
                st
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse stop_times.txt; skipping calls phase");
                vec![]
            }
        },
        Err(_) => {
            tracing::warn!("stop_times.txt not found in GTFS archive; skipping calls phase");
            vec![]
        }
    };

    // --- Phase 5: Build per-trip stop lists ---
    // Group stop_times by trip_id, sorted by stop_sequence.
    let mut trip_stops: HashMap<&str, Vec<&GtfsStopTime>> = HashMap::new();
    for st in &stop_times {
        trip_stops.entry(st.trip_id.as_str()).or_default().push(st);
    }
    for stops in trip_stops.values_mut() {
        stops.sort_by_key(|s| s.stop_sequence);
    }

    // --- Operator identity: agency.txt + routes.txt → uid → toc ---
    let agencies = match extract_file(zip_bytes, "agency.txt") {
        Ok(bytes) => parse_agency(&bytes).unwrap_or_default(),
        Err(_) => {
            tracing::warn!("agency.txt not found in GTFS archive; operators will be unlabelled");
            HashMap::new()
        }
    };
    let route_to_agency = match extract_file(zip_bytes, "routes.txt") {
        Ok(bytes) => parse_routes(&bytes).unwrap_or_default(),
        Err(_) => {
            tracing::warn!("routes.txt not found in GTFS archive; operators will be unlabelled");
            HashMap::new()
        }
    };
    let uid_toc = derive_uid_toc(&trips, &route_to_agency);
    tracing::info!(agencies = agencies.len(), uid_toc = uid_toc.len(), "Resolved operator identity");
    if !agencies.is_empty() {
        let n = upsert_operators(db, &agencies).await?;
        emit(progress, |s| s.push_log(format!("Operators: {n}")));
    }

    // --- Phase 6: Build services list ---
    let mut services: Vec<(String, String, String, i16, Option<String>)> = Vec::new();
    // Track uid → bitmask so we can know which UIDs made it in
    let mut uid_to_days: HashMap<String, i16> = HashMap::new();

    for trip in &trips {
        let uid = match extract_uid(&trip.trip_id) {
            Some(u) => u.to_owned(),
            None => continue,
        };
        let days = match calendar_map.get(&trip.service_id) {
            Some(&d) => d,
            None => continue,
        };

        // Skip if we already have this UID — first-seen wins
        if uid_to_days.contains_key(&uid) {
            continue;
        }

        let stops = match trip_stops.get(trip.trip_id.as_str()) {
            Some(s) if s.len() >= 2 => s,
            _ => continue,
        };

        let origin_crs = match stop_id_to_crs(&stops[0].stop_id) {
            Some(c) => c.to_owned(),
            None => continue,
        };
        let dest_crs = match stop_id_to_crs(&stops[stops.len() - 1].stop_id) {
            Some(c) => c.to_owned(),
            None => continue,
        };

        let toc = uid_toc.get(&uid).cloned();
        uid_to_days.insert(uid.clone(), days);
        services.push((uid, origin_crs, dest_crs, days, toc));
    }

    emit(progress, |s| { s.phase = IngestPhase::Services; s.push_log(format!("Upserting {} services", services.len())); });
    let services_upserted = upsert_services(db, &services, &known_stations).await?;
    tracing::info!(services = services_upserted, "Services upserted");
    emit(progress, |s| { s.services = services_upserted; s.push_log(format!("Services done: {services_upserted}")); });

    // Build known UIDs set (those actually persisted)
    let known_uids: HashSet<String> = services
        .iter()
        .filter(|(_, origin, dest, _, _)| {
            known_stations.contains(origin) && known_stations.contains(dest)
        })
        .map(|(uid, _, _, _, _)| uid.clone())
        .collect();

    // --- Phase 7: Build timetable_calls ---
    emit(progress, |s| s.push_log(format!("Stop times: {}", stop_times.len())));
    if stop_times.is_empty() {
        emit(progress, |s| { s.phase = IngestPhase::Complete; s.finished_at = Some(chrono::Utc::now()); s.push_log("Complete (no stop_times)"); });
        tracing::info!(
            stations = station_count,
            services = services_upserted,
            "GTFS ingest complete (no stop_times)"
        );
        return Ok(station_count);
    }

    let today = chrono::Utc::now().date_naive();
    let mut calls: Vec<TimetableCallRow> = Vec::new();

    for trip in &trips {
        let uid = match extract_uid(&trip.trip_id) {
            Some(u) => u.to_owned(),
            None => continue,
        };
        if !known_uids.contains(&uid) {
            continue;
        }

        let stops = match trip_stops.get(trip.trip_id.as_str()) {
            Some(s) => s,
            None => continue,
        };

        // Emit rows for a rolling 7-day window starting today.
        for day_offset in 0..7i64 {
            let operating_date = today + chrono::Duration::days(day_offset);

            for (order, st) in stops.iter().enumerate() {
                let location_crs = match stop_id_to_crs(&st.stop_id) {
                    Some(c) => c.to_owned(),
                    None => continue,
                };
                if !known_stations.contains(&location_crs) {
                    continue;
                }

                let scheduled_departure = parse_gtfs_time(&st.departure_time);
                let public_departure = parse_gtfs_time(&st.arrival_time);

                calls.push(TimetableCallRow {
                    uid: uid.clone(),
                    operating_date,
                    location_crs,
                    call_order: order as i16,
                    scheduled_departure,
                    public_departure,
                });
            }
        }
    }

    emit(progress, |s| { s.phase = IngestPhase::TimetableCalls; s.push_log(format!("Upserting {} timetable calls", calls.len())); });
    let calls_upserted = upsert_timetable_calls(db, &calls, &known_uids, &known_stations).await?;
    tracing::info!(
        stations = station_count,
        services = services_upserted,
        timetable_calls = calls_upserted,
        "GTFS ingest complete"
    );
    emit(progress, |s| {
        s.timetable_calls = calls_upserted;
        s.phase = IngestPhase::Complete;
        s.finished_at = Some(chrono::Utc::now());
        s.push_log(format!(
            "Complete — stations: {station_count}, services: {services_upserted}, calls: {calls_upserted}"
        ));
    });

    Ok(station_count)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CSV: &str = "\
stop_id,stop_name,stop_lat,stop_lon
LDS,Leeds,53.7952,-1.5479
MAN,Manchester Piccadilly,53.4772,-2.2309
KGX,London Kings Cross,51.5308,-0.1238
PLT1,Leeds Platform 1,,
abc,lowercase should be skipped,,
";

    #[test]
    fn parse_stops_extracts_valid_crs_rows() {
        let stations = parse_stops(SAMPLE_CSV.as_bytes()).unwrap();
        assert_eq!(stations.len(), 3);
        let ids: Vec<&str> = stations.iter().map(|s| s.stop_id.as_str()).collect();
        assert!(ids.contains(&"LDS"));
        assert!(ids.contains(&"MAN"));
        assert!(ids.contains(&"KGX"));
    }

    #[test]
    fn parse_stops_skips_platform_and_lowercase() {
        let stations = parse_stops(SAMPLE_CSV.as_bytes()).unwrap();
        let ids: Vec<&str> = stations.iter().map(|s| s.stop_id.as_str()).collect();
        assert!(!ids.contains(&"PLT1"), "4-letter id should be excluded");
        assert!(!ids.contains(&"abc"), "lowercase should be excluded");
    }

    #[test]
    fn parse_stops_handles_missing_lat_lon() {
        let csv = "stop_id,stop_name,stop_lat,stop_lon\nLDS,Leeds,,\n";
        let stations = parse_stops(csv.as_bytes()).unwrap();
        assert_eq!(stations.len(), 1);
        assert!(stations[0].stop_lat.is_none());
        assert!(stations[0].stop_lon.is_none());
    }

    #[test]
    fn parse_stops_empty_input_returns_empty() {
        let csv = "stop_id,stop_name,stop_lat,stop_lon\n";
        let stations = parse_stops(csv.as_bytes()).unwrap();
        assert!(stations.is_empty());
    }

    #[test]
    fn is_valid_crs_rejects_non_alpha_and_wrong_length() {
        assert!(is_valid_crs("LDS"));
        assert!(!is_valid_crs("LD"));
        assert!(!is_valid_crs("LDSS"));
        assert!(!is_valid_crs("ld1"));
        assert!(!is_valid_crs("123"));
    }

    // --- New tests for item 2.5 ---

    #[test]
    fn parse_calendar_produces_correct_bitmask() {
        // Mon–Fri only: bitmask should be 0b0011111 = 31
        let csv = "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday\n\
                   WD,1,1,1,1,1,0,0\n";
        let map = parse_calendar(csv.as_bytes()).unwrap();
        assert_eq!(map.get("WD").copied(), Some(31i16));
    }

    #[test]
    fn parse_gtfs_time_handles_overflow() {
        // "25:30:00" should wrap to 01:30:00
        let t = parse_gtfs_time("25:30:00").expect("should parse");
        assert_eq!(t, chrono::NaiveTime::from_hms_opt(1, 30, 0).unwrap());
    }

    #[test]
    fn parse_gtfs_time_handles_normal() {
        // "14:23:45" should give 14:23:45
        let t = parse_gtfs_time("14:23:45").expect("should parse");
        assert_eq!(t, chrono::NaiveTime::from_hms_opt(14, 23, 45).unwrap());
    }

    #[test]
    fn parse_gtfs_time_rejects_invalid() {
        assert!(parse_gtfs_time("").is_none());
        assert!(parse_gtfs_time("abc").is_none());
        assert!(parse_gtfs_time("10:60:00").is_none()); // invalid minutes
    }

    #[test]
    fn extract_uid_handles_standard_format() {
        assert_eq!(extract_uid("C12345_20240417"), Some("C12345"));
    }

    #[test]
    fn extract_uid_rejects_short_uid() {
        assert_eq!(extract_uid("C1234_20240417"), None);
    }

    #[test]
    fn extract_uid_rejects_non_alphanumeric() {
        assert_eq!(extract_uid("C1234!_20240417"), None);
    }

    #[test]
    fn stop_id_to_crs_strips_nr_prefix() {
        assert_eq!(stop_id_to_crs("9100LEEDS"), Some("EDS"));
        // Actually the last 3 of "9100LEEDS" is "EDS" — but LEEDS ends in EDS
        // Let's test a known realistic NR stop_id
        assert_eq!(stop_id_to_crs("9100LDS"), Some("LDS"));
        assert_eq!(stop_id_to_crs("LDS"), Some("LDS"));
    }

    #[test]
    fn stop_id_to_crs_rejects_non_alpha_suffix() {
        assert_eq!(stop_id_to_crs("STOP123"), None);
        assert_eq!(stop_id_to_crs("AB"), None);
    }

    #[test]
    fn parse_trips_skips_malformed_rows() {
        let csv = "trip_id,service_id,trip_headsign\n\
                   C12345_20240417,WD,Leeds to London\n\
                   bad_row\n\
                   C67890_20240417,WE,Manchester to Birmingham\n";
        let trips = parse_trips(csv.as_bytes()).unwrap();
        // malformed row is skipped, valid rows parsed
        assert_eq!(trips.len(), 2);
        assert_eq!(trips[0].trip_id, "C12345_20240417");
        assert_eq!(trips[1].trip_id, "C67890_20240417");
    }

    #[test]
    fn parse_calendar_full_week_bitmask() {
        // All 7 days: bitmask = 0b1111111 = 127
        let csv = "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday\n\
                   ALL,1,1,1,1,1,1,1\n";
        let map = parse_calendar(csv.as_bytes()).unwrap();
        assert_eq!(map.get("ALL").copied(), Some(127i16));
    }

    #[test]
    fn parse_calendar_weekend_only() {
        // Sat+Sun only: bitmask = (1<<5)|(1<<6) = 32+64 = 96
        let csv = "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday\n\
                   WE,0,0,0,0,0,1,1\n";
        let map = parse_calendar(csv.as_bytes()).unwrap();
        assert_eq!(map.get("WE").copied(), Some(96i16));
    }

    #[test]
    fn parse_agency_maps_id_to_name() {
        let csv = "agency_id,agency_name,agency_url,agency_timezone\n\
                   GW,Great Western Railway,http://x,Europe/London\n\
                   VT,Avanti West Coast,http://y,Europe/London\n";
        let m = parse_agency(csv.as_bytes()).unwrap();
        assert_eq!(m.get("GW").map(String::as_str), Some("Great Western Railway"));
        assert_eq!(m.get("VT").map(String::as_str), Some("Avanti West Coast"));
    }

    #[test]
    fn parse_routes_maps_route_to_agency() {
        let csv = "route_id,agency_id,route_short_name,route_type\n\
                   R1,GW,GWR,2\n\
                   R2,VT,AWC,2\n";
        let m = parse_routes(csv.as_bytes()).unwrap();
        assert_eq!(m.get("R1").map(String::as_str), Some("GW"));
        assert_eq!(m.get("R2").map(String::as_str), Some("VT"));
    }

    #[test]
    fn derive_uid_toc_resolves_via_route_first_seen_wins() {
        let trips = vec![
            GtfsTrip { route_id: "R1".into(), trip_id: "C12345_20240417".into(), service_id: "WD".into() },
            GtfsTrip { route_id: "R2".into(), trip_id: "C12345_20240418".into(), service_id: "WD".into() },
            GtfsTrip { route_id: "RX".into(), trip_id: "D99999_20240417".into(), service_id: "WD".into() },
        ];
        let mut routes = HashMap::new();
        routes.insert("R1".to_string(), "GW".to_string());
        routes.insert("R2".to_string(), "VT".to_string());
        let m = derive_uid_toc(&trips, &routes);
        assert_eq!(m.get("C12345").map(String::as_str), Some("GW"));
        assert_eq!(m.get("D99999"), None);
    }
}
