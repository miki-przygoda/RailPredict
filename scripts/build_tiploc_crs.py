#!/usr/bin/env python3
"""One-time: build src/cache/tiploc_crs.tsv (TIPLOC -> CRS) from the public
fasteroute/national-rail-stations dataset — the same source cache/tiploc_names.tsv
and cache/tiploc_coords.tsv use. Run from the repo root:

    python3 scripts/build_tiploc_crs.py

CRS (the 3-letter station code rail staff use day to day) is only defined for
real passenger stations; sidings, depots and junctions have no CRS and are left
to fall back to their TIPLOC downstream.
"""
import json
import sys
import urllib.request

# The dataset's default branch has moved before; try both.
URLS = [
    "https://raw.githubusercontent.com/fasteroute/national-rail-stations/main/stations.json",
    "https://raw.githubusercontent.com/fasteroute/national-rail-stations/master/stations.json",
]
OUT = "RailPredict/src/cache/tiploc_crs.tsv"


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
    if isinstance(data, list):
        return data
    if isinstance(data, dict):
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
        crs = (r.get("crs") or "").strip().upper()
        if not tip or not crs:
            continue
        seen.setdefault(tip, crs)
    with open(OUT, "w", encoding="utf-8") as f:
        for tip in sorted(seen):
            f.write(f"{tip}\t{seen[tip]}\n")
    print(f"wrote {len(seen)} TIPLOC->CRS pairs -> {OUT}")


if __name__ == "__main__":
    main()
