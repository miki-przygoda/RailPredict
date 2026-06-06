//! TIPLOC → (latitude, longitude) lookup, for plotting locations on a map.
//!
//! Mirrors [`super::location_names`]: an embedded reference derived from the public
//! Darwin-built dataset at <https://github.com/fasteroute/national-rail-stations>
//! (which carries `lat`/`lon` alongside the name). The TSV is `TIPLOC\tlat\tlon`,
//! WGS84 decimal degrees, parsed once into a static map. Regenerate the TSV with
//! `scripts/build_tiploc_coords.py`. No runtime network.

use std::collections::HashMap;
use std::sync::LazyLock;

static TSV: &str = include_str!("tiploc_coords.tsv");

static COORDS: LazyLock<HashMap<&'static str, (f64, f64)>> = LazyLock::new(|| {
    TSV.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let tiploc = parts.next()?;
            let lat = parts.next()?.parse::<f64>().ok()?;
            let lon = parts.next()?.parse::<f64>().ok()?;
            Some((tiploc, (lat, lon)))
        })
        .collect()
});

/// `(latitude, longitude)` in WGS84 decimal degrees for a TIPLOC code, if known.
pub fn coords(tiploc: &str) -> Option<(f64, f64)> {
    COORDS.get(tiploc).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tiploc_resolves_to_gb_coords() {
        let (lat, lon) = coords("WATRLMN").expect("Waterloo should resolve");
        // London is ~51.5, -0.11; assert a generous GB-ish box, not exact values.
        assert!((49.0..61.0).contains(&lat), "lat {lat} not in GB range");
        assert!((-8.0..2.0).contains(&lon), "lon {lon} not in GB range");
    }

    #[test]
    fn every_row_is_within_great_britain() {
        // Guards against a malformed regeneration (swapped lat/lon, bad parse).
        for (tiploc, (lat, lon)) in COORDS.iter() {
            assert!((49.0..61.0).contains(lat), "{tiploc}: lat {lat} out of range");
            assert!((-8.5..2.0).contains(lon), "{tiploc}: lon {lon} out of range");
        }
    }

    #[test]
    fn unknown_tiploc_is_none() {
        assert_eq!(coords("ZZZZZZZ"), None);
    }
}
