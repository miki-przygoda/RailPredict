# RailPredict OS — Stage 1: TIPLOC→coordinate data

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provide an offline TIPLOC→(latitude, longitude) lookup so the map (Stage 2) can place trains on Great Britain.

**Architecture:** Mirror the existing `cache::location_names` pattern exactly. A one-time Python tool fetches the public fasteroute "national-rail-stations" dataset (the same source `location_names` already uses — it carries `lat`/`lon` too) and writes a committed `tiploc_coords.tsv`; a new `cache::location_coords` module embeds that TSV via `include_str!` and parses it once into a static map. No runtime network, no DB table — purely an embedded, offline reference.

**Tech Stack:** Rust (std `LazyLock` + `HashMap`, `include_str!`, in-file `#[cfg(test)]`); a small one-time Python 3 script (stdlib only).

---

## File Structure

- `scripts/build_tiploc_coords.py` *(create)* — one-time data-gen: fetch fasteroute `stations.json`, write the committed TSV. Not part of the build; run by hand when refreshing coordinates.
- `RailPredict/src/cache/tiploc_coords.tsv` *(create, committed)* — the reference data: one `TIPLOC\tlat\tlon` row per resolvable location.
- `RailPredict/src/cache/location_coords.rs` *(create)* — embeds the TSV, parses once, exposes `coords(tiploc) -> Option<(f64, f64)>`. One responsibility: the coordinate lookup.
- `RailPredict/src/cache/mod.rs` *(modify)* — register `pub mod location_coords;`.

Interface other stages depend on: `railpredict::cache::location_coords::coords(tiploc: &str) -> Option<(f64, f64)>` returning `(latitude, longitude)` in WGS84 decimal degrees.

---

### Task 1: Generate and commit `tiploc_coords.tsv`

**Files:**
- Create: `scripts/build_tiploc_coords.py`
- Create: `RailPredict/src/cache/tiploc_coords.tsv` (output of the script)

This task is a one-time data-gen tool, verified by running it and sanity-checking the output (not unit-tested — it's a throwaway generator, like the other `scripts/`).

- [ ] **Step 1: Write the generator script**

Create `scripts/build_tiploc_coords.py` with exactly:

```python
#!/usr/bin/env python3
"""One-time: build src/cache/tiploc_coords.tsv (TIPLOC -> lat lon) from the public
fasteroute/national-rail-stations dataset — the same source cache/tiploc_names.tsv
uses. Run from the repo root:  python3 scripts/build_tiploc_coords.py
"""
import json
import sys
import urllib.request

# The dataset's default branch has moved before; try both.
URLS = [
    "https://raw.githubusercontent.com/fasteroute/national-rail-stations/main/stations.json",
    "https://raw.githubusercontent.com/fasteroute/national-rail-stations/master/stations.json",
]
OUT = "RailPredict/src/cache/tiploc_coords.tsv"


def fetch():
    for url in URLS:
        try:
            with urllib.request.urlopen(url, timeout=30) as r:
                if r.status == 200:
                    return json.load(r)
        except Exception as e:  # noqa: BLE001 - one-time tool, report and try next
            print(f"  {url} -> {e}", file=sys.stderr)
    sys.exit("Could not fetch stations.json from any known URL")


def rows(data):
    # The file is either a list of station dicts or a dict keyed by TIPLOC.
    return data.values() if isinstance(data, dict) else data


def main():
    data = fetch()
    seen = {}
    for r in rows(data):
        if not isinstance(r, dict):
            continue
        tip = (r.get("tiploc") or "").strip().upper()
        lat = r.get("lat")
        lon = r.get("lon")
        if not tip or lat in (None, 0, 0.0) or lon in (None, 0, 0.0):
            continue
        # First valid wins; coordinates are stable enough for plotting.
        seen.setdefault(tip, (float(lat), float(lon)))
    with open(OUT, "w", encoding="utf-8") as f:
        for tip in sorted(seen):
            lat, lon = seen[tip]
            f.write(f"{tip}\t{lat:.5f}\t{lon:.5f}\n")
    print(f"wrote {len(seen)} coords -> {OUT}")


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Run it (one-time; needs network) and sanity-check**

Run from the repo root:
```bash
python3 scripts/build_tiploc_coords.py
```
Expected: prints `wrote <N> coords -> RailPredict/src/cache/tiploc_coords.tsv` with N in the low thousands (the dataset has ~2,500–3,000 located stations). Then verify a few known TIPLOCs are present with sane GB coordinates:
```bash
grep -E '^(WATRLMN|GLGC|KNGX|EDINBUR)\b' RailPredict/src/cache/tiploc_coords.tsv
wc -l RailPredict/src/cache/tiploc_coords.tsv
```
Expected: `WATRLMN` resolves to roughly `51.50  -0.11` (London), `GLGC`/`GLGC`-area to roughly `55.86  -4.25` (Glasgow); total line count > 2000. If a known station is missing or a coordinate is wildly outside GB (lat not in 49–61, lon not in -8–2), stop and report — the dataset structure may have changed and the script's field names need adjusting.

- [ ] **Step 3: Check coverage against our real data** (so Stage 2 knows what to expect)

The replay plots trains by their `prediction_outcomes.origin_crs` / `destination_crs`, which are TIPLOC-style codes. Measure how many resolve:
```bash
psql postgresql://railpredict:railpredict@localhost:5432/railpredict_v2 -tc "
  SELECT origin_crs FROM prediction_outcomes
  WHERE finalised_at IS NOT NULL AND predicted_delay_mins <> 0
  GROUP BY origin_crs" | tr -d ' ' | sort -u > /tmp/replay_tiplocs.txt
cut -f1 RailPredict/src/cache/tiploc_coords.tsv | sort -u > /tmp/have_coords.txt
echo "replay origin tiplocs: $(grep -c . /tmp/replay_tiplocs.txt)"
echo "of which resolved:      $(comm -12 /tmp/replay_tiplocs.txt /tmp/have_coords.txt | grep -c .)"
```
Record the two numbers in the commit message. This is informational — there is no hard pass/fail, but if far less than ~half resolve, note it: Stage 2 will simply omit unresolved trains from the map, and a NaPTAN/OSM gap-fill becomes a worthwhile Stage 1b follow-up. Do **not** block on it.

- [ ] **Step 4: Commit**

```bash
git add scripts/build_tiploc_coords.py RailPredict/src/cache/tiploc_coords.tsv
git commit -m "feat(os): TIPLOC->lat/lon reference data (fasteroute) + generator

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `cache::location_coords` loader

**Files:**
- Create: `RailPredict/src/cache/location_coords.rs`
- Modify: `RailPredict/src/cache/mod.rs` (add `pub mod location_coords;` after `pub mod location_names;`)

- [ ] **Step 1: Register the module**

In `RailPredict/src/cache/mod.rs`, add this line immediately after the existing `pub mod location_names;`:

```rust
pub mod location_coords;
```

- [ ] **Step 2: Write the failing test + the loader**

Create `RailPredict/src/cache/location_coords.rs` with exactly:

```rust
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
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --manifest-path RailPredict/Cargo.toml --lib cache::location_coords`
Expected: PASS (3 tests). If `known_tiploc_resolves_to_gb_coords` fails because `WATRLMN` is absent, pick any TIPLOC you confirmed present in Task 1 Step 2 and use it instead. If `every_row_is_within_great_britain` fails, the TSV has a bad row (likely lat/lon swapped) — fix `build_tiploc_coords.py` and regenerate before continuing.

- [ ] **Step 4: Confirm the whole crate still builds**

Run: `cargo build --release --manifest-path RailPredict/Cargo.toml`
Expected: `Finished` with no errors.

- [ ] **Step 5: Commit**

```bash
git add RailPredict/src/cache/mod.rs RailPredict/src/cache/location_coords.rs
git commit -m "feat(os): cache::location_coords TIPLOC->lat/lon lookup

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for later stages (not part of Stage 1)

- Stage 2's data gather calls `cache::location_coords::coords(&train.origin)` / `(&train.dest)` and omits trains where either is `None` (logged), never faking a position.
- If Task 1 Step 3 showed weak coverage, Stage 1b (a NaPTAN RailReferences → CRS → our stations-table lat/lon join, plus OSM `ref:tiploc` for gaps) raises it. Deferred until coverage is shown to be a problem.
- No version bump for Stage 1 — it adds internal plumbing with no user-facing surface. The first bump lands with Stage 3 (the visible OS shell).
