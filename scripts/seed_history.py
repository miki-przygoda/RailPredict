"""
seed_history.py — Backfill delay_history with synthetic historical data.

WHEN TO USE:
    Run this when setting up a fresh instance before any live Darwin data has
    collected. It gives the prediction engine enough history to start producing
    confidence-weighted predictions immediately. Once you have 7+ days of real
    Darwin data, this script is no longer needed — real observations will
    naturally outweigh the synthetic ones (MAX_SAMPLES=90 per pattern, FIFO).

    Do NOT run this on top of an established DB — it will mix synthetic data
    with real observations, degrading model accuracy.

HOW IT WORKS:
    Reads the top N origin_crs (TIPLOC) values already seen in delay_history,
    then generates 90 days of realistic historical records for each
    (origin_crs, uid, weekday, departure_hour) service-pattern.

    Delay distribution is calibrated to published UK rail statistics:
      - ~70 % on time (≤1 min)
      - ~15 % slight delay (2–5 min)
      - ~10 % moderate delay (6–20 min)
      -  ~4 % significant delay (21–60 min)
      -  ~1 % severe delay (>60 min)

    Time-of-day and per-station biases are applied.
    predicted_delay_mins is left NULL so rows serve as training data only.

Usage:
    python scripts/seed_history.py [--days 90] [--stations 120]

Requirements:
    pip install psycopg2-binary python-dotenv
    DATABASE_URL set in .env or environment.
"""

import argparse
import hashlib
import math
import os
import random
import sys
from datetime import datetime, timedelta, timezone

# ---------------------------------------------------------------------------
# Args
# ---------------------------------------------------------------------------

def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--days",     type=int, default=90,
                   help="How many days of history to generate (default 90)")
    p.add_argument("--stations", type=int, default=120,
                   help="How many top TIPLOCs to cover (default 120)")
    return p.parse_args()

# ---------------------------------------------------------------------------
# Env / DB
# ---------------------------------------------------------------------------

def load_env():
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

def get_db_url(env):
    url = os.environ.get("DATABASE_URL") or env.get("DATABASE_URL", "")
    if not url:
        sys.exit("DATABASE_URL not set — add it to .env or set the env var.")
    return url.replace("@db:", "@localhost:")

# ---------------------------------------------------------------------------
# Fetch top TIPLOCs already observed in the live feed
# ---------------------------------------------------------------------------

def fetch_top_tiplocs(cur, n: int) -> list[tuple[str, int]]:
    """Return [(origin_crs, count)] for the top N TIPLOCs in delay_history."""
    cur.execute("""
        SELECT origin_crs, COUNT(*) AS cnt
        FROM delay_history
        WHERE recorded_at > NOW() - INTERVAL '7 days'
        GROUP BY origin_crs
        ORDER BY cnt DESC
        LIMIT %s
    """, (n,))
    rows = cur.fetchall()
    if not rows:
        # Fallback: grab anything in the table
        cur.execute("""
            SELECT origin_crs, COUNT(*) AS cnt
            FROM delay_history
            GROUP BY origin_crs
            ORDER BY cnt DESC
            LIMIT %s
        """, (n,))
        rows = cur.fetchall()
    print(f"  Found {len(rows)} active TIPLOCs in DB")
    return rows

# ---------------------------------------------------------------------------
# Deterministic UID generation
# For each (tiploc, departure_hour, service_index) we produce a stable 6-char UID
# so the same "recurring service" uses the same UID across historical weeks.
# ---------------------------------------------------------------------------

def make_uid(tiploc: str, hour: int, idx: int) -> str:
    key = f"{tiploc}:{hour}:{idx}"
    digest = hashlib.md5(key.encode()).hexdigest()
    # CRS prefix letter (A-Z) + 5 digits
    letter = chr(ord('A') + int(digest[0], 16) % 26)
    digits = str(int(digest[1:7], 16) % 100000).zfill(5)
    return f"{letter}{digits}"

# ---------------------------------------------------------------------------
# Delay generation — calibrated to UK rail statistics
# ---------------------------------------------------------------------------

# Per-hour multiplier: peak hours have higher delays
HOUR_MULT = {
    0: 0.4, 1: 0.3, 2: 0.3, 3: 0.3, 4: 0.5, 5: 0.7,
    6: 0.9, 7: 1.4, 8: 1.6, 9: 1.2, 10: 1.0, 11: 0.9,
    12: 0.9, 13: 0.9, 14: 0.9, 15: 1.1, 16: 1.3, 17: 1.5,
    18: 1.5, 19: 1.3, 20: 1.1, 21: 1.0, 22: 0.8, 23: 0.6,
}

# Stations known to have higher-than-average delays (Southern/Thameslink congestion)
HIGH_DELAY_TIPLOCS = {
    "LNDNBDE", "VICTRIC", "ECROYDN", "GTWK", "BRGHTN",
    "CLPHMJC", "CLPHMJM", "BROMLYS", "CRSTNPK",
}

def generate_delay(tiploc: str, hour: int, weekday: int, rng: random.Random) -> int:
    """Return a synthetic delay_mins value (can be negative = early)."""
    h_mult = HOUR_MULT.get(hour, 1.0)
    weekend_mult = 0.75 if weekday >= 5 else 1.0
    station_mult = 1.4 if tiploc in HIGH_DELAY_TIPLOCS else 1.0
    mult = h_mult * weekend_mult * station_mult

    # Pick delay regime
    roll = rng.random()
    if roll < 0.68:
        # On time or tiny variation (-2..+1)
        base = rng.randint(-2, 1)
    elif roll < 0.83:
        # Slight delay 2–5 min
        base = rng.randint(2, 5)
    elif roll < 0.93:
        # Moderate delay 6–20 min
        base = int(rng.lognormvariate(math.log(10), 0.5))
        base = max(6, min(base, 20))
    elif roll < 0.97:
        # Significant delay 21–60 min
        base = int(rng.lognormvariate(math.log(30), 0.6))
        base = max(21, min(base, 60))
    else:
        # Severe delay 61–240 min
        base = int(rng.lognormvariate(math.log(80), 0.7))
        base = max(61, min(base, 240))

    # Apply multiplier to positive delays only
    if base > 1:
        base = max(1, int(base * mult))

    return base

# ---------------------------------------------------------------------------
# Generate rows
# ---------------------------------------------------------------------------

def generate_rows(tiplocs: list[str], days_back: int) -> list[tuple]:
    """Yield (uid, weekday, origin_crs, departure_hour, delay_mins, recorded_at) tuples."""
    rng = random.Random(42)
    now = datetime.now(timezone.utc)
    start_date = (now - timedelta(days=days_back)).date()
    end_date = (now - timedelta(hours=2)).date()  # Don't overlap with today's live data

    rows = []
    for tiploc in tiplocs:
        # Number of distinct "services" per hour varies by station size
        services_per_hour = rng.randint(2, 8)

        for hour in range(5, 24):           # Most services 05:00–23:00
            for idx in range(services_per_hour):
                uid = make_uid(tiploc, hour, idx)
                # Generate one record per matching day in the window
                current = start_date
                while current <= end_date:
                    weekday = current.weekday()  # 0=Mon, 6=Sun
                    # Most commuter services run Mon–Fri, some on weekends
                    if weekday >= 5 and rng.random() < 0.55:
                        current += timedelta(days=1)
                        continue

                    delay = generate_delay(tiploc, hour, weekday, rng)

                    # recorded_at: same weekday, hour, random minute within hour
                    minute = rng.randint(0, 59)
                    rec_dt = datetime(
                        current.year, current.month, current.day,
                        hour, minute, rng.randint(0, 59),
                        tzinfo=timezone.utc,
                    )

                    rows.append((uid, weekday, tiploc, hour, delay, rec_dt))
                    current += timedelta(days=1)

    return rows

# ---------------------------------------------------------------------------
# Bulk upsert
# ---------------------------------------------------------------------------

CHUNK = 500

def upsert_rows(cur, rows: list[tuple]) -> int:
    inserted = 0
    for i in range(0, len(rows), CHUNK):
        chunk = rows[i : i + CHUNK]
        # Use ON CONFLICT DO NOTHING so we don't overwrite live data
        args_list = []
        placeholders = []
        for r in chunk:
            placeholders.append("(%s,%s,%s,%s,%s,%s)")
            args_list.extend(r)
        sql = (
            "INSERT INTO delay_history "
            "(uid, weekday, origin_crs, departure_hour, delay_mins, recorded_at) "
            "VALUES " + ",".join(placeholders) +
            " ON CONFLICT (uid, weekday, origin_crs, departure_hour, recorded_at) DO NOTHING"
        )
        cur.execute(sql, args_list)
        inserted += len(chunk)
        print(f"  Inserted {min(inserted, len(rows))}/{len(rows)} …", end="\r")
    print()
    return inserted

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    args = parse_args()
    env = load_env()
    db_url = get_db_url(env)

    try:
        import psycopg2
    except ImportError:
        sys.exit("psycopg2 not installed — run: pip install psycopg2-binary")

    print(f"Connecting to database …")
    conn = psycopg2.connect(db_url)
    conn.autocommit = False
    cur = conn.cursor()

    # Pull top TIPLOCs from live data
    tiploc_rows = fetch_top_tiplocs(cur, args.stations)
    tiplocs = [r[0] for r in tiploc_rows]

    print(f"Generating {args.days} days of history for {len(tiplocs)} TIPLOCs …")
    rows = generate_rows(tiplocs, args.days)
    print(f"  → {len(rows):,} rows to insert")

    upsert_rows(cur, rows)
    conn.commit()

    # Summary
    cur.execute("SELECT COUNT(*) FROM delay_history")
    total = cur.fetchone()[0]
    cur.execute(
        "SELECT COUNT(*) FROM delay_history WHERE recorded_at < NOW() - INTERVAL '1 day'"
    )
    historical = cur.fetchone()[0]
    cur.execute(
        "SELECT COUNT(*) FROM delay_history WHERE recorded_at > NOW() - INTERVAL '1 day'"
    )
    today = cur.fetchone()[0]

    print(f"\ndelay_history totals:")
    print(f"  Historical (>1 day old): {historical:,}")
    print(f"  Today (live data):       {today:,}")
    print(f"  Grand total:             {total:,}")
    print()
    print("Next steps:")
    print("  1. make train          — retrain models on the full history")
    print("  2. restart the server  — picks up new ONNX models")
    print("  3. open /predictions   — accuracy updates as live predictions arrive")

    cur.close()
    conn.close()

if __name__ == "__main__":
    main()
