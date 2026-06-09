//! TIPLOC → CRS (3-letter station code) lookup.
//!
//! The live registry and the journey tables key locations by Darwin **TIPLOC**
//! (e.g. `WATRLMN`). Rail staff read the 3-letter **CRS** code (`WAT`) day to
//! day, so the live map surfaces CRS rather than the long friendly name. Resolved
//! from an embedded reference derived from the same public, Darwin-built dataset
//! as [`super::location_names`] (<https://github.com/fasteroute/national-rail-stations>).
//!
//! Only real passenger stations have a CRS; sidings, depots and junctions have
//! none and fall back to their TIPLOC unchanged (which is itself a code).

use std::collections::HashMap;
use std::sync::LazyLock;

static TSV: &str = include_str!("tiploc_crs.tsv");

static CODES: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| TSV.lines().filter_map(|line| line.split_once('\t')).collect());

/// CRS code for a TIPLOC, if it is a real station.
pub fn code(tiploc: &str) -> Option<&'static str> {
    CODES.get(tiploc).copied()
}

/// CRS code if known, else the raw TIPLOC unchanged (sidings/depots/junctions).
pub fn code_or_tiploc(tiploc: &str) -> &str {
    CODES.get(tiploc).copied().unwrap_or(tiploc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_stations_resolve_to_crs() {
        assert_eq!(code("WATRLMN"), Some("WAT"));
        assert_eq!(code("GLGC"), Some("GLC"));
    }

    #[test]
    fn sidings_fall_back_to_tiploc() {
        // A depot/siding TIPLOC has no CRS — returned unchanged.
        assert_eq!(code_or_tiploc("ZZZZZZZ"), "ZZZZZZZ");
    }
}
