//! GTFS static data ingest: downloads the Network Rail GTFS ZIP, extracts
//! `stops.txt`, and upserts station rows into the database.
//!
//! `parse_stops` is a pure function so it can be tested without I/O.
//! `run_ingest` performs the full download → parse → upsert pipeline.

use std::io::Read;

use serde::Deserialize;
use sqlx::QueryBuilder;

use crate::db::Db;

// ---------------------------------------------------------------------------
// GTFS row type
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

// ---------------------------------------------------------------------------
// Pure parse
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

fn is_valid_crs(s: &str) -> bool {
    s.len() == 3 && s.chars().all(|c| c.is_ascii_uppercase())
}

// ---------------------------------------------------------------------------
// Full ingest pipeline
// ---------------------------------------------------------------------------

/// Download the GTFS ZIP from `url`, extract `stops.txt`, parse it, and
/// upsert all valid stations into the database. Idempotent.
pub async fn run_ingest(db: &Db, url: &str) -> anyhow::Result<usize> {
    tracing::info!(%url, "Downloading GTFS archive");
    let bytes = reqwest::get(url).await?.bytes().await?;
    tracing::info!(size_bytes = bytes.len(), "Downloaded GTFS archive");

    let stops_bytes = extract_stops_txt(&bytes)?;
    let stations = parse_stops(&stops_bytes)?;
    let count = stations.len();
    tracing::info!(stations = count, "Parsed GTFS stops");

    upsert_stations(db, &stations).await?;
    tracing::info!(stations = count, "GTFS ingest complete");
    Ok(count)
}

/// Same as `run_ingest` but reads from a local file instead of downloading.
pub async fn run_ingest_from_file(db: &Db, path: &std::path::Path) -> anyhow::Result<usize> {
    let bytes = std::fs::read(path)?;
    let stops_bytes = extract_stops_txt(&bytes)?;
    let stations = parse_stops(&stops_bytes)?;
    let count = stations.len();
    tracing::info!(stations = count, "Parsed GTFS stops from file");
    upsert_stations(db, &stations).await?;
    tracing::info!(stations = count, "GTFS ingest complete");
    Ok(count)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_stops_txt(zip_bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| anyhow::anyhow!("Failed to open ZIP: {e}"))?;

    let mut file = archive
        .by_name("stops.txt")
        .map_err(|_| anyhow::anyhow!("stops.txt not found in GTFS archive"))?;

    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

const UPSERT_CHUNK: usize = 200;

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
}
