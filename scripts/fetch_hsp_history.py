"""
fetch_hsp_history.py — Parallel HSP historical data ingestion via major O-D pairs.

Strategy: query serviceMetrics for each (from, to) pair to enumerate RIDs, then
serviceDetails per RID to extract departure-delay records for EVERY stop on the
route.  One serviceDetails call covers 5-20+ stations — far more efficient than
per-station queries, and sidesteps the from_loc == to_loc API restriction.

Progress is tracked in the hsp_fetch_progress DB table (created at startup).
Safe to Ctrl-C and rerun — already-completed (from, to, date) triplets are skipped.

Quick-start (4 accounts, 20 workers each):
    HSP_USERNAME=u1 HSP_PASSWORD=p1 \
        python3 fetch_hsp_history.py --shard-id 0 --shards 4 --days 90 --workers 20 &
    HSP_USERNAME=u2 HSP_PASSWORD=p2 \
        python3 fetch_hsp_history.py --shard-id 1 --shards 4 --days 90 --workers 20 &
    # … repeat for shards 2 and 3

Single-account dry-run (verbose output, no DB writes):
    HSP_USERNAME=u HSP_PASSWORD=p \
        python3 fetch_hsp_history.py --shards 1 --shard-id 0 --days 3 --workers 20 --dry-run

Environment:
    DATABASE_URL  — postgres connection string
    HSP_USERNAME  — opendata.nationalrail.co.uk email address
    HSP_PASSWORD  — opendata.nationalrail.co.uk password (HSP subscription must be ticked)
"""

import argparse
import os
import sys
import time
import random
import logging
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import date, timedelta, datetime, timezone
from pathlib import Path

import requests
from dotenv import load_dotenv
import psycopg2
from psycopg2.extras import execute_values

load_dotenv()

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s  %(levelname)-7s  %(message)s",
    datefmt="%H:%M:%S",
)
log = logging.getLogger("fetch_hsp")

HSP_BASE    = "https://hsp-prod.rockshore.net/api/v1"
CHUNK       = 2000
MAX_RETRIES = 6
RETRY_BASE  = 1.0

# ---------------------------------------------------------------------------
# Major O-D pairs — together these cover the vast majority of UK rail stations.
# Both directions included for key corridors so terminal stations get departure
# delay records (not just arrival).
# ---------------------------------------------------------------------------
MAJOR_OD_PAIRS = [
    # East Coast Main Line
    ("KGX", "EDB"), ("EDB", "KGX"),
    ("KGX", "ABD"), ("KGX", "NCL"), ("KGX", "LDS"), ("KGX", "YRK"),
    ("EDB", "LDS"),
    # West Coast Main Line
    ("EUS", "GLC"), ("GLC", "EUS"),
    ("EUS", "MAN"), ("MAN", "EUS"),
    ("EUS", "LIV"), ("EUS", "BHM"), ("EUS", "CAR"),
    # Great Western Main Line
    ("PAD", "PLY"), ("PLY", "PAD"),
    ("PAD", "BRI"), ("BRI", "PAD"),
    ("PAD", "SWA"), ("PAD", "EXD"), ("PAD", "CDF"), ("CDF", "PAD"),
    # Midland Main Line
    ("STP", "SHF"), ("SHF", "STP"),
    ("STP", "LDS"),
    # South West Main Line
    ("WAT", "BMH"), ("BMH", "WAT"),
    ("WAT", "EXD"), ("WAT", "SOU"),
    # Southern / Thameslink
    ("VIC", "GTW"), ("VIC", "BTN"), ("BTN", "VIC"),
    ("LBG", "GTW"),
    # Southeastern
    ("CHX", "AFK"), ("AFK", "CHX"),
    # Anglia
    ("LST", "NRW"), ("NRW", "LST"),
    ("LST", "IPS"),
    # TransPennine
    ("LIV", "HUL"), ("HUL", "LIV"),
    ("MAN", "YRK"), ("MAN", "NCL"), ("LDS", "HUL"),
    # CrossCountry
    ("BHM", "EDB"), ("EDB", "BHM"),
    ("BHM", "MAN"), ("BHM", "BRI"), ("BRI", "BHM"),
    ("BHM", "PLY"), ("PLY", "BHM"),
    # Northern England
    ("LDS", "NCL"), ("NCL", "LDS"),
    ("MAN", "LDS"), ("LIV", "MAN"), ("LIV", "LDS"),
    # Scotland
    ("EDB", "GLC"), ("GLC", "EDB"),
    ("EDB", "ABD"), ("ABD", "EDB"),
    ("GLC", "IVN"), ("IVN", "GLC"),
    # Wales
    ("CDF", "SWA"), ("SWA", "CDF"),
    # Commuter / regional
    ("EUS", "MKC"), ("WAT", "BSK"), ("BSK", "WAT"),
    ("STP", "LUT"), ("PAD", "RDG"), ("RDG", "PAD"),
]

BANK_HOLIDAYS = {
    date(2024,  1,  1), date(2024,  3, 29), date(2024,  4,  1),
    date(2024,  5,  6), date(2024,  8, 26), date(2024, 12, 25), date(2024, 12, 26),
    date(2025,  1,  1), date(2025,  4, 18), date(2025,  4, 21),
    date(2025,  5,  5), date(2025,  8, 25), date(2025, 12, 25), date(2025, 12, 26),
    date(2026,  1,  1), date(2026,  4,  3), date(2026,  4,  6),
    date(2026,  5,  4), date(2026,  5, 25), date(2026,  8, 31),
    date(2026, 12, 25), date(2026, 12, 28),
}

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def parse_args():
    p = argparse.ArgumentParser(
        description="Parallel HSP history ingest via major O-D pairs",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument("--shard-id",  type=int, default=0,  help="This process's shard index (0-based, default 0)")
    p.add_argument("--shards",    type=int, default=1,  help="Total number of parallel shards (default 1 = all pairs)")
    p.add_argument("--days",      type=int, default=90, help="Random non-holiday days to sample (default 90)")
    p.add_argument("--seed",      type=int, default=42, help="RNG seed — same seed across shards gives same day pool (default 42)")
    p.add_argument("--workers",   type=int, default=20, help="Concurrent serviceDetails threads (default 20)")
    p.add_argument("--dry-run",   action="store_true",  help="Fetch but don't write to DB (implies --verbose)")
    p.add_argument("--verbose",   action="store_true",  help="Enable INFO-level logging (default: WARNING only)")
    return p.parse_args()

# ---------------------------------------------------------------------------
# Day sampling
# ---------------------------------------------------------------------------

def sample_days(n: int, seed: int) -> list[date]:
    yesterday = date.today() - timedelta(days=1)
    pool = []
    for i in range(1, 366):
        d = yesterday - timedelta(days=i)
        if d not in BANK_HOLIDAYS:
            pool.append(d)
    rng = random.Random(seed)
    rng.shuffle(pool)
    return sorted(pool[:n])

def day_type_for(d: date) -> str:
    if d.weekday() == 5: return "SATURDAY"
    if d.weekday() == 6: return "SUNDAY"
    return "WEEKDAY"

# ---------------------------------------------------------------------------
# DB helpers
# ---------------------------------------------------------------------------

def db_connect():
    url = os.environ.get("DATABASE_URL", "")
    url = url.replace("@db:", "@localhost:")
    return psycopg2.connect(url)

def fetch_all_stations(conn) -> list[str]:
    with conn.cursor() as cur:
        cur.execute("SELECT crs FROM stations WHERE is_active = true ORDER BY crs")
        return [row[0].strip() for row in cur.fetchall()]

_insert_lock   = threading.Lock()
_insert_buf:   list = []
_progress_buf: list = []   # (from_crs, to_crs, date_str) tuples

def enqueue(row):
    with _insert_lock:
        _insert_buf.append(row)

def enqueue_progress(from_crs: str, to_crs: str, date_str: str):
    with _insert_lock:
        _progress_buf.append((from_crs, to_crs, date_str))

def flush(conn, dry_run: bool, force: bool = False) -> int:
    with _insert_lock:
        if not force and len(_insert_buf) < CHUNK:
            return 0
        rows = _insert_buf[:]
        prog = _progress_buf[:]
        _insert_buf.clear()
        _progress_buf.clear()
    if not rows and not prog:
        return 0
    if dry_run:
        log.info("[dry-run] would insert %d rows / %d progress marks", len(rows), len(prog))
        return 0
    with conn.cursor() as cur:
        if rows:
            execute_values(cur, """
                INSERT INTO delay_history
                    (uid, origin_crs, weekday, departure_hour, delay_mins,
                     predicted_delay_mins, recorded_at)
                VALUES %s
                ON CONFLICT DO NOTHING
            """, rows, template="(%s,%s,%s,%s,%s,NULL,%s)")
        if prog:
            execute_values(cur, """
                INSERT INTO hsp_fetch_progress (from_crs, to_crs, dt)
                VALUES %s ON CONFLICT DO NOTHING
            """, prog, template="(%s,%s,%s::date)")
    conn.commit()
    return len(rows)

# ---------------------------------------------------------------------------
# Progress tracking — DB-backed
# ---------------------------------------------------------------------------

def create_progress_table(conn):
    with conn.cursor() as cur:
        # Migrate from old per-station schema (crs, dt) if present.
        cur.execute("""
            DO $$ BEGIN
                IF NOT EXISTS (
                    SELECT 1 FROM information_schema.columns
                    WHERE table_name = 'hsp_fetch_progress'
                      AND column_name = 'from_crs'
                ) THEN
                    DROP TABLE IF EXISTS hsp_fetch_progress;
                END IF;
            END $$;
        """)
        cur.execute("""
            CREATE TABLE IF NOT EXISTS hsp_fetch_progress (
                from_crs TEXT NOT NULL,
                to_crs   TEXT NOT NULL,
                dt       DATE NOT NULL,
                PRIMARY KEY (from_crs, to_crs, dt)
            )
        """)
    conn.commit()

def load_progress_db(conn) -> set:
    with conn.cursor() as cur:
        cur.execute("SELECT from_crs, to_crs, dt::text FROM hsp_fetch_progress")
        return {(r[0], r[1], r[2]) for r in cur.fetchall()}

# ---------------------------------------------------------------------------
# HSP API — no rate cap, exponential back-off on 429/503
# ---------------------------------------------------------------------------

def hsp_post(endpoint: str, payload: dict, auth: tuple) -> dict | None:
    url = f"{HSP_BASE}/{endpoint}"
    backoff = RETRY_BASE
    for attempt in range(MAX_RETRIES):
        try:
            resp = requests.post(url, json=payload, auth=auth, timeout=30)
            if resp.status_code == 404:
                return None
            if resp.status_code in (429, 503):
                wait = backoff * (2 ** attempt) + random.uniform(0, 1)
                log.debug("HTTP %d on %s — backing off %.1fs", resp.status_code, endpoint, wait)
                time.sleep(wait)
                continue
            resp.raise_for_status()
            return resp.json()
        except requests.HTTPError as e:
            log.warning("HSP %s HTTP error: %s", endpoint, e)
            return None
        except requests.RequestException as e:
            wait = backoff * (2 ** attempt)
            log.warning("HSP %s network error (attempt %d): %s — retry in %.1fs",
                        endpoint, attempt + 1, e, wait)
            time.sleep(wait)
    log.error("HSP %s gave up after %d attempts", endpoint, MAX_RETRIES)
    return None

def get_rids(from_crs: str, to_crs: str, date_str: str, dt: str, auth: tuple) -> list[str]:
    data = hsp_post("serviceMetrics", {
        "from_loc":  from_crs,
        "to_loc":    to_crs,
        "from_time": "0000",
        "to_time":   "2359",
        "from_date": date_str,
        "to_date":   date_str,
        "days":      dt,
    }, auth)
    if not data:
        return []
    rids = []
    for svc in (data.get("Services") or []):
        for rid in (svc.get("serviceAttributesMetrics", {}).get("rids") or []):
            rids.append(str(rid))
    return rids

def fetch_delays_for_service(rid: str, date_str: str, day: date,
                             auth: tuple, known_stations: set) -> list:
    """Return one delay row per stop in the service that has departure data."""
    data = hsp_post("serviceDetails", {"date": date_str, "rid": rid}, auth)
    if not data:
        return []
    details = data.get("serviceAttributesDetails") or {}
    if not isinstance(details, dict):
        return []
    rows = []
    for loc in details.get("locations", []):
        crs = (loc.get("location") or "").strip().upper()[:3]
        if not crs or crs not in known_stations:
            continue
        gbtt   = loc.get("gbtt_ptd") or ""
        actual = loc.get("actual_td") or ""
        if len(gbtt) < 4 or len(actual) < 4:
            continue
        try:
            sm = int(gbtt[:2])   * 60 + int(gbtt[2:4])
            am = int(actual[:2]) * 60 + int(actual[2:4])
            delay = am - sm
            if delay < -120: delay += 1440
            if delay >  720: delay -= 1440
            if not (-120 <= delay <= 600):
                continue
            dep_h = int(gbtt[:2])
            uid   = rid[8:] if len(rid) > 8 and rid[:8].isdigit() else rid
            recorded_at = datetime(day.year, day.month, day.day,
                                   dep_h, int(gbtt[2:4]), tzinfo=timezone.utc)
            rows.append((uid, crs, day.weekday(), dep_h, int(delay), recorded_at))
        except (ValueError, IndexError):
            continue
    return rows

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    args = parse_args()
    load_dotenv(Path(__file__).parent.parent / ".env")

    log.setLevel(logging.INFO if (args.verbose or args.dry_run) else logging.WARNING)

    username = os.environ.get("HSP_USERNAME", "")
    password = os.environ.get("HSP_PASSWORD", "")
    if not username or not password:
        sys.exit(
            "HSP_USERNAME / HSP_PASSWORD not set.\n"
            "Register at https://opendata.nationalrail.co.uk/ and tick 'HSP' "
            "under Subscription Type in your profile."
        )
    auth = (username, password)

    conn = db_connect()
    create_progress_table(conn)
    log.info("DB connected")

    known_stations = set(fetch_all_stations(conn))
    if not known_stations:
        sys.exit("No stations found in DB — run GTFS ingest first")

    shard_pairs = [p for i, p in enumerate(MAJOR_OD_PAIRS) if i % args.shards == args.shard_id]
    days = sample_days(args.days, args.seed)

    log.info("Shard %d/%d — %d O-D pairs  |  %d days  |  %d workers  |  %d known stations",
             args.shard_id, args.shards, len(shard_pairs), args.days, args.workers, len(known_stations))
    log.info("Day pool: %s … %s", days[0].isoformat(), days[-1].isoformat())

    done = load_progress_db(conn)
    log.info("Resuming from DB: %d pair-days already complete", len(done))

    total_inserted = 0
    total_skipped  = 0

    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        for (from_crs, to_crs) in shard_pairs:
            pair_rows = 0
            for day in days:
                date_str = day.isoformat()
                if (from_crs, to_crs, date_str) in done:
                    total_skipped += 1
                    continue

                dt   = day_type_for(day)
                rids = get_rids(from_crs, to_crs, date_str, dt, auth)
                enqueue_progress(from_crs, to_crs, date_str)

                if rids:
                    futures = {
                        pool.submit(fetch_delays_for_service, rid, date_str, day, auth, known_stations): rid
                        for rid in rids
                    }
                    for future in as_completed(futures):
                        for row in (future.result() or []):
                            enqueue(row)

                n = flush(conn, args.dry_run)
                if n:
                    total_inserted += n
                    pair_rows      += n

            if pair_rows:
                log.info("  %s→%s: +%d rows  (total: %d)", from_crs, to_crs, pair_rows, total_inserted)

    n = flush(conn, args.dry_run, force=True)
    total_inserted += n

    conn.close()
    log.info("Done — %d rows inserted  (%d pair-days skipped as already done)",
             total_inserted, total_skipped)

if __name__ == "__main__":
    main()
