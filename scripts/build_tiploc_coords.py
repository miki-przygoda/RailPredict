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
    # The file is either a list of station dicts, a dict keyed by TIPLOC,
    # or (as of the fasteroute dataset) {"locations": [...]} wrapper.
    if isinstance(data, list):
        return data
    if isinstance(data, dict):
        # {"locations": [...]} wrapper form used by fasteroute
        if "locations" in data and isinstance(data["locations"], list):
            return data["locations"]
        return data.values()
    return data


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
        lat, lon = float(lat), float(lon)
        # Great Britain only — the map is GB, so drop international termini
        # (Eurostar: Paris, Brussels, Avignon, …) that would plot off-frame.
        if not (49.0 <= lat <= 61.0 and -8.5 <= lon <= 2.0):
            continue
        seen.setdefault(tip, (lat, lon))
    with open(OUT, "w", encoding="utf-8") as f:
        for tip in sorted(seen):
            lat, lon = seen[tip]
            f.write(f"{tip}\t{lat:.5f}\t{lon:.5f}\n")
    print(f"wrote {len(seen)} coords -> {OUT}")


if __name__ == "__main__":
    main()
