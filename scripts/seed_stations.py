"""
seed_stations.py — Populate the `stations` table from OpenStreetMap.

Uses the Overpass API to fetch every UK National Rail station that has a CRS
code (ref:crs tag).  No account or API key required — Overpass is free and
publicly accessible.  The OSM data also includes TIPLOC codes (ref:tiploc) and
coordinates for almost all stations, which the ingest pipeline cannot derive
from GTFS alone.

Run:
    python scripts/seed_stations.py

Or via make:
    make seed-stations

Environment:
    DATABASE_URL   Postgres connection string (read from .env if not set).
                   Uses @localhost: substitution if @db: is present (Docker host).
"""

import json
import os
import sys
import time
import urllib.request
import urllib.error

# ---------------------------------------------------------------------------
# Env / DB
# ---------------------------------------------------------------------------

def load_env() -> dict:
    env = {}
    env_path = os.path.join(os.path.dirname(__file__), "..", ".env")
    if os.path.exists(env_path):
        with open(env_path) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith("#") and "=" in line:
                    k, _, v = line.partition("=")
                    env[k.strip()] = v.strip()
    return env


def get_db_url(env: dict) -> str:
    url = os.environ.get("DATABASE_URL") or env.get("DATABASE_URL", "")
    if not url:
        sys.exit("DATABASE_URL not set — add it to .env or set the env var.")
    # Swap Docker service hostname for localhost when running on the host.
    return url.replace("@db:", "@localhost:")


# ---------------------------------------------------------------------------
# Overpass fetch
# ---------------------------------------------------------------------------

OVERPASS_URL = "https://overpass-api.de/api/interpreter"

OVERPASS_QUERY = """
[out:json][timeout:120];
(
  node["railway"="station"]["ref:crs"]["network"~"National Rail",i];
  node["railway"="station"]["ref:crs"]["operator"~"Network Rail",i];
  node["railway"="station"]["ref:crs"]["ref:naptanDisambiguation"~".*"];
  node["railway"="station"]["ref:crs"];
);
out body;
"""

def fetch_stations_from_osm() -> list[dict]:
    print("Querying Overpass API for UK National Rail stations …")
    encoded = OVERPASS_QUERY.encode("utf-8")
    req = urllib.request.Request(
        OVERPASS_URL,
        data=encoded,
        headers={
            "Content-Type": "application/x-www-form-urlencoded",
            "User-Agent": "RailPredict/1.9 (station-seed-script)",
        },
        method="POST",
    )
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=180) as resp:
                raw = resp.read().decode("utf-8")
            break
        except urllib.error.HTTPError as e:
            if e.code == 429 and attempt < 2:
                wait = 30 * (attempt + 1)
                print(f"  Rate limited (429) — waiting {wait}s …")
                time.sleep(wait)
            else:
                sys.exit(f"Overpass HTTP error {e.code}: {e.reason}")
        except Exception as e:
            sys.exit(f"Overpass request failed: {e}")

    data = json.loads(raw)
    elements = data.get("elements", [])
    print(f"  → {len(elements)} OSM nodes returned")
    return elements


def parse_stations(elements: list[dict]) -> list[dict]:
    seen_crs: dict[str, dict] = {}
    for el in elements:
        tags = el.get("tags", {})
        crs = tags.get("ref:crs", "").strip().upper()
        if len(crs) != 3 or not crs.isalpha():
            continue
        name = (
            tags.get("name:en")
            or tags.get("name")
            or tags.get("official_name")
            or ""
        ).strip()
        if not name:
            continue

        lat = el.get("lat")
        lon = el.get("lon")
        tiploc = tags.get("ref:tiploc", "").strip().upper() or None

        if crs not in seen_crs:
            seen_crs[crs] = {
                "crs": crs,
                "name": name,
                "lat": lat,
                "lon": lon,
                "tiploc": tiploc,
            }
        else:
            # Prefer the entry that has a TIPLOC
            if tiploc and not seen_crs[crs]["tiploc"]:
                seen_crs[crs]["tiploc"] = tiploc

    stations = list(seen_crs.values())
    print(f"  → {len(stations)} distinct CRS codes parsed")
    with_tiploc = sum(1 for s in stations if s["tiploc"])
    print(f"  → {with_tiploc} stations have a TIPLOC mapping")
    return stations


# ---------------------------------------------------------------------------
# DB upsert
# ---------------------------------------------------------------------------

def upsert_stations(db_url: str, stations: list[dict]) -> None:
    try:
        import psycopg2
    except ImportError:
        sys.exit(
            "psycopg2 not installed — run: pip install psycopg2-binary"
        )

    print(f"Connecting to database …")
    conn = psycopg2.connect(db_url)
    conn.autocommit = False
    cur = conn.cursor()

    CHUNK = 200
    total = 0
    for i in range(0, len(stations), CHUNK):
        chunk = stations[i : i + CHUNK]
        for s in chunk:
            cur.execute(
                """
                INSERT INTO stations (crs, name, lat, lon, tiploc, updated_at)
                VALUES (%s, %s, %s, %s, %s, NOW())
                ON CONFLICT (crs) DO UPDATE
                    SET name       = EXCLUDED.name,
                        lat        = EXCLUDED.lat,
                        lon        = EXCLUDED.lon,
                        tiploc     = COALESCE(EXCLUDED.tiploc, stations.tiploc),
                        updated_at = EXCLUDED.updated_at
                """,
                (s["crs"], s["name"], s["lat"], s["lon"], s["tiploc"]),
            )
        conn.commit()
        total += len(chunk)
        print(f"  Upserted {total}/{len(stations)} …", end="\r")

    print(f"\nDone — {total} stations upserted.")
    cur.close()
    conn.close()


# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

def print_summary(db_url: str) -> None:
    try:
        import psycopg2
        conn = psycopg2.connect(db_url)
        cur = conn.cursor()
        cur.execute("SELECT COUNT(*) FROM stations")
        total = cur.fetchone()[0]
        cur.execute("SELECT COUNT(*) FROM stations WHERE tiploc IS NOT NULL")
        with_tiploc = cur.fetchone()[0]
        cur.execute("SELECT COUNT(*) FROM stations WHERE lat IS NOT NULL")
        with_coords = cur.fetchone()[0]
        print(f"\nStations table: {total} rows total")
        print(f"  With TIPLOC:      {with_tiploc}")
        print(f"  With coordinates: {with_coords}")
        cur.close()
        conn.close()
    except Exception:
        pass


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    env = load_env()
    db_url = get_db_url(env)

    elements = fetch_stations_from_osm()
    stations = parse_stations(elements)

    if not stations:
        sys.exit("No stations parsed — check Overpass query or network.")

    upsert_stations(db_url, stations)
    print_summary(db_url)
    print("\nRestart the RailPredict server to pick up the new station data.")
