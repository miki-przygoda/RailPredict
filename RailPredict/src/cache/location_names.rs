//! TIPLOC → friendly station-name lookup.
//!
//! `delay_history`, `prediction_outcomes`, and the live registry key locations by
//! Darwin **TIPLOC** code (e.g. `WATRLMN`), which carries no friendly name on its
//! own. This resolves it from an embedded reference derived from the public,
//! Darwin-built dataset at <https://github.com/fasteroute/national-rail-stations>
//! (TIPLOC → name). ~3000 entries, parsed once into a static map.
//!
//! Note: that dataset also carries a *managing*-TOC per location — deliberately not
//! used for the operator league, which needs the per-service `uid → toc` mapping
//! that only the GTFS/CIF timetable ingest provides.

use std::collections::HashMap;
use std::sync::LazyLock;

static TSV: &str = include_str!("tiploc_names.tsv");

static NAMES: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| TSV.lines().filter_map(|line| line.split_once('\t')).collect());

/// Friendly station name for a TIPLOC code, if known.
pub fn name(tiploc: &str) -> Option<&'static str> {
    NAMES.get(tiploc).copied()
}

/// Friendly name if known, else the raw code unchanged.
pub fn name_or_code(tiploc: &str) -> &str {
    NAMES.get(tiploc).copied().unwrap_or(tiploc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tiplocs_resolve() {
        assert_eq!(name("WATRLMN"), Some("London Waterloo"));
        assert_eq!(name("GLGC"), Some("Glasgow Central"));
    }

    #[test]
    fn unknown_falls_back_to_code() {
        assert_eq!(name("ZZZZZZZ"), None);
        assert_eq!(name_or_code("ZZZZZZZ"), "ZZZZZZZ");
    }
}
