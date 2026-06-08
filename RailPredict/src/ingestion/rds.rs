//! Rail Settlement Plan (RSP / Rail Delivery Group) reference-data CSV loader.
//!
//! Loads the headerless RSP export CSVs in `imports/` into the Tier A `stations` and
//! `operators` tables. Column meaning is **positional** (see `imports/README.md`) — these
//! are raw RSP exports, so we map by index and parse with a real CSV reader (quoted address
//! fields contain commas). They supersede the OSM-derived station list with official CRS/NLC.
//!
//! Loaded:
//!   - `rds_station.csv` (+ `rds_station_coords.csv` for lon/lat, joined on 4-digit NLC) → `stations`
//!   - `rds_toc.csv` → `operators`
//!
//! `rds_railcard.csv` / `rds_ticket_type.csv` are reference-only (no target table yet) — skipped.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use sqlx::QueryBuilder;

use crate::db::Db;

/// Rows per batched upsert — well under PG's 65535 bind-parameter limit.
const CHUNK: usize = 500;

/// One station parsed from `rds_station.csv`, joined with coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct RdsStation {
    pub crs: String,
    pub name: String,
    pub nlc: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

/// Counts from a reference ingest run.
#[derive(Debug, Default, Clone, Copy)]
pub struct RdsSummary {
    pub stations: usize,
    pub operators: usize,
}

fn reader(bytes: &[u8]) -> csv::Reader<&[u8]> {
    csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(bytes)
}

/// Parse `rds_station_coords.csv` → map of 4-digit NLC → (lon, lat). Skips `0,0` (no fix).
pub fn parse_coords(bytes: &[u8]) -> anyhow::Result<HashMap<String, (f64, f64)>> {
    let mut out = HashMap::new();
    for rec in reader(bytes).records() {
        let rec = rec?;
        let nlc = rec.get(1).unwrap_or("").trim().to_string();
        let lon: f64 = rec.get(2).unwrap_or("").trim().parse().unwrap_or(0.0);
        let lat: f64 = rec.get(3).unwrap_or("").trim().parse().unwrap_or(0.0);
        if nlc.is_empty() || (lon == 0.0 && lat == 0.0) {
            continue;
        }
        out.insert(nlc, (lon, lat));
    }
    Ok(out)
}

/// Parse `rds_station.csv` → stations that carry a public CRS (col 7), joined to coordinates
/// by 4-digit NLC (col 6). NLC-only settlement/group rows (no CRS) are skipped, as are
/// duplicate CRS rows (first occurrence wins).
pub fn parse_stations(
    bytes: &[u8],
    coords: &HashMap<String, (f64, f64)>,
) -> anyhow::Result<Vec<RdsStation>> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for rec in reader(bytes).records() {
        let rec = rec?;
        let crs = rec.get(7).unwrap_or("").trim().to_string();
        if crs.len() != 3 || !seen.insert(crs.clone()) {
            continue; // no public CRS, or a duplicate validity row
        }
        let name = rec.get(1).unwrap_or("").trim().to_string();
        let nlc = rec.get(6).unwrap_or("").trim().to_string();
        let (lon, lat) = match coords.get(&nlc) {
            Some(&(lon, lat)) => (Some(lon), Some(lat)),
            None => (None, None),
        };
        out.push(RdsStation { crs, name, nlc, lat, lon });
    }
    Ok(out)
}

/// Parse `rds_toc.csv` → (ATOC TOC code, name) pairs, deduped by code (first wins).
pub fn parse_operators(bytes: &[u8]) -> anyhow::Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for rec in reader(bytes).records() {
        let rec = rec?;
        let toc = rec.get(1).unwrap_or("").trim().to_string();
        let name = rec.get(2).unwrap_or("").trim().to_string();
        if toc.is_empty() || name.is_empty() || !seen.insert(toc.clone()) {
            continue;
        }
        out.push((toc, name));
    }
    Ok(out)
}

async fn upsert_stations(db: &Db, stations: &[RdsStation]) -> anyhow::Result<usize> {
    let mut total = 0;
    for chunk in stations.chunks(CHUNK) {
        let mut qb = QueryBuilder::new("INSERT INTO stations (crs, name, nlc, lat, lon) ");
        qb.push_values(chunk, |mut b, s| {
            let nlc: Option<&str> = if s.nlc.is_empty() { None } else { Some(s.nlc.as_str()) };
            b.push_bind(&s.crs)
                .push_bind(&s.name)
                .push_bind(nlc)
                .push_bind(s.lat)
                .push_bind(s.lon);
        });
        // Refresh name/nlc always; only overwrite coords when the new row actually has them.
        qb.push(
            " ON CONFLICT (crs) DO UPDATE SET \
               name = EXCLUDED.name, \
               nlc = COALESCE(EXCLUDED.nlc, stations.nlc), \
               lat = COALESCE(EXCLUDED.lat, stations.lat), \
               lon = COALESCE(EXCLUDED.lon, stations.lon), \
               updated_at = now()",
        );
        total += qb.build().execute(db).await?.rows_affected() as usize;
    }
    Ok(total)
}

async fn upsert_operators(db: &Db, ops: &[(String, String)]) -> anyhow::Result<usize> {
    let mut total = 0;
    for chunk in ops.chunks(CHUNK) {
        let mut qb = QueryBuilder::new("INSERT INTO operators (toc, name) ");
        qb.push_values(chunk, |mut b, (toc, name)| {
            b.push_bind(toc).push_bind(name);
        });
        // Keep any curated brand_color already set; just refresh the name.
        qb.push(" ON CONFLICT (toc) DO UPDATE SET name = EXCLUDED.name, updated_at = now()");
        total += qb.build().execute(db).await?.rows_affected() as usize;
    }
    Ok(total)
}

/// Load the RSP reference CSVs from `dir` into the `stations` and `operators` tables.
pub async fn run_ingest_rds(db: &Db, dir: &Path) -> anyhow::Result<RdsSummary> {
    let read = |name: &str| -> anyhow::Result<Vec<u8>> {
        let path = dir.join(name);
        std::fs::read(&path).map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))
    };

    let coords = parse_coords(&read("rds_station_coords.csv")?)?;
    let stations = parse_stations(&read("rds_station.csv")?, &coords)?;
    let operators = parse_operators(&read("rds_toc.csv")?)?;

    let n_stations = upsert_stations(db, &stations).await?;
    let n_operators = upsert_operators(db, &operators).await?;

    tracing::info!(
        stations = n_stations,
        operators = n_operators,
        "RDS reference ingest complete"
    );
    Ok(RdsSummary { stations: n_stations, operators: n_operators })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coords_skip_zero_fix() {
        let csv = b"1,5131,0.12141,51.49107,\"Abbey Wood (London) Rail Station, SE2\"\n\
                    2,9999,0,0,\"no fix\"\n";
        let coords = parse_coords(csv).unwrap();
        assert_eq!(coords.get("5131"), Some(&(0.12141, 51.49107)));
        assert!(!coords.contains_key("9999"), "0,0 rows are skipped");
    }

    #[test]
    fn stations_need_crs_and_join_coords() {
        let coords = HashMap::from([("5131".to_string(), (0.12141, 51.49107))]);
        // Row 1 has CRS ABW; row 2 is an NLC-only settlement row (no CRS) → skipped.
        let csv = b"1,Abbey Wood,ABBEY WOOD      ,Abbey Wood,7051310,705131,5131,ABW,true,true\n\
                    2,Settlement Only,X,X,7000000,700000,7000,,true,true\n";
        let st = parse_stations(csv, &coords).unwrap();
        assert_eq!(st.len(), 1, "NLC-only row without a CRS is skipped");
        assert_eq!(st[0].crs, "ABW");
        assert_eq!(st[0].name, "Abbey Wood");
        assert_eq!(st[0].nlc, "5131");
        assert_eq!(st[0].lon, Some(0.12141));
        assert_eq!(st[0].lat, Some(51.49107));
    }

    #[test]
    fn stations_dedup_by_crs() {
        let coords = HashMap::new();
        let csv = b"1,Leeds,LEEDS,Leeds,8074400,807440,4400,LDS,true,true\n\
                    2,Leeds (old validity),LEEDS,Leeds,8074400,807440,4400,LDS,true,true\n";
        let st = parse_stations(csv, &coords).unwrap();
        assert_eq!(st.len(), 1, "duplicate CRS rows collapse to the first");
        assert_eq!(st[0].name, "Leeds");
    }

    #[test]
    fn operators_parsed_and_deduped() {
        let csv = b"1,AR,ANGLIA RAILWAYS TRAIN SERVICES,AR.png,ANGLIA RAILWAYS TRAIN SERVICES,0\n\
                    2,AR,DUP,AR.png,DUP,0\n\
                    3,GW,GREAT WESTERN RAILWAY,GW.png,GREAT WESTERN RAILWAY,0\n";
        let ops = parse_operators(csv).unwrap();
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0], ("AR".to_string(), "ANGLIA RAILWAYS TRAIN SERVICES".to_string()));
        assert_eq!(ops[1].0, "GW");
    }
}
