#!/usr/bin/env python3
"""
Generate synthetic training data for RailPredict and persist to delay_history_synthetic.

Produces N weeks (default 3) of artificial operating days covering the full spectrum:
  good    — ~95% on-time, low rolling stats (trains running normally)
  average — ~70% on-time, moderate rolling stats (typical mixed operating day)
  (bad days are not generated — real Darwin data already covers that end)

Service patterns are cloned from the largest real day in delay_history so all UIDs,
origin stations, and departure hours are real observations.  Only the delay signal
and its self-consistent derived features (rolling mean/std, congestion, cascade) are
synthetic — generated to match the day type rather than the disrupted real history.

Writes to delay_history_synthetic, keyed on generation tag.  Re-running with the
same tag is idempotent (existing rows for that generation are replaced).

Usage:
    python generate_synthetic.py                     # 3 weeks, default generation tag
    python generate_synthetic.py --weeks 4           # 4 weeks
    python generate_synthetic.py --rows-per-day 300000
    python generate_synthetic.py --generation my-tag --force
    python generate_synthetic.py --dry-run           # print plan, don't touch DB
"""

import argparse
import os
import sys
from datetime import date, timedelta
from pathlib import Path

import numpy as np
import pandas as pd
from dotenv import load_dotenv
from sqlalchemy import create_engine, text

# ---------------------------------------------------------------------------
# Schedule definition
# ---------------------------------------------------------------------------

# Three 7-day weeks anchored in April (clearly before real May Darwin data).
# Each tuple: (weekday 0=Mon…6=Sun, day_type).
# Week A: fully punctual — model learns what on-time operation looks like.
# Week B: all average — model learns the 70/30 mixed operating day.
# Week C: mixed — two good days, rest average; spreads the contrast.
DEFAULT_SCHEDULE = [
    # Week A (2026-04-07 Mon → 2026-04-13 Sun)
    (0, "good"),    (1, "good"),    (2, "good"),    (3, "good"),
    (4, "good"),    (5, "average"), (6, "average"),
    # Week B (2026-04-14 Mon → 2026-04-20 Sun)
    (0, "average"), (1, "average"), (2, "average"), (3, "average"),
    (4, "average"), (5, "good"),    (6, "average"),
    # Week C (2026-04-21 Mon → 2026-04-27 Sun)
    (0, "good"),    (1, "average"), (2, "good"),    (3, "average"),
    (4, "average"), (5, "average"), (6, "good"),
]

# Week anchor Mondays — one per week, sequentially.
WEEK_MONDAYS = ["2026-04-07", "2026-04-14", "2026-04-21"]

# Feature distributions per day type.
# Each value is (mean, std, clip_lo, clip_hi).
_DIST: dict[str, dict[str, tuple[float, float, float, float]]] = {
    "good": {
        # mean=-5 shifts ~95% of N(-5,3) below 0: Φ(5/3)=Φ(1.67)≈95.3% on-time
        "delay":        ( -5,  3, -10,  8),
        "roll_mean":    ( -2,  3,  -5,  5),
        "roll_std":     (  3,  2,   1,  8),
        "roll_ontime":  ( 90,  5,  75, 99),
        "congestion":   (  1,  2,   0,  8),
        "cascade":      (  1,  2,   0,  8),
        "preceding":    ( -2,  2,  -6,  3),
    },
    "average": {
        # bimodal: 80% in on-time pool, 20% in late pool.
        # on-time pool N(-3,2.5): Φ(3/2.5)=Φ(1.2)≈88.5% ≤ 0 → 80%×88.5%=70.8% total on-time
        "delay_ok":     ( -3,  2.5, -8,  4),
        "delay_late":   ( 22, 15,    8, 90),
        "roll_mean":    (  5,  4,   -2, 20),
        "roll_std":     ( 10,  4,    2, 20),
        "roll_ontime":  ( 65,  8,   45, 82),
        "congestion":   (  6,  5,    0, 25),
        "cascade":      (  5,  5,    0, 25),
        "preceding":    (  3,  5,   -3, 20),
    },
}

# Columns stored in delay_history_synthetic (matches migration schema).
_DB_COLS = [
    "uid", "weekday", "origin_crs", "departure_hour", "delay_mins",
    "rolling_mean_7d", "rolling_std_7d", "rolling_ontime_7d",
    "rolling_mean_14d", "rolling_std_14d",
    "current_delay_mins", "preceding_delay_mins", "mins_until_departure",
    "wind_mph", "volatility_score", "station_congestion_30m",
    "operator_cascade_delay", "predecessor_train_delay",
    "day_type", "synth_week", "synthetic_date", "generation",
]


# ---------------------------------------------------------------------------
# Core generation
# ---------------------------------------------------------------------------

def _sample_normal(rng: np.random.Generator, dist: tuple, n: int) -> np.ndarray:
    mean, std, lo, hi = dist
    return rng.normal(mean, std, n).clip(lo, hi).astype(np.float32)


def make_day(
    pool: pd.DataFrame,
    weekday: int,
    day_type: str,
    synth_week: int,
    synthetic_date: date,
    generation: str,
    rows_per_day: int,
    seed: int,
) -> pd.DataFrame:
    """Clone `rows_per_day` rows from `pool`, replacing all delay/feature signals."""
    rng = np.random.default_rng(seed)
    n = min(rows_per_day, len(pool))
    idx = rng.choice(len(pool), size=n, replace=False)
    day = pool.iloc[idx].copy()
    d = _DIST[day_type]

    if day_type == "good":
        delay    = _sample_normal(rng, d["delay"], n).round().astype(np.int32)
        roll_m   = _sample_normal(rng, d["roll_mean"], n)
        roll_s   = _sample_normal(rng, d["roll_std"], n)
        roll_ot  = _sample_normal(rng, d["roll_ontime"], n)
        cong     = _sample_normal(rng, d["congestion"], n)
        cascade  = _sample_normal(rng, d["cascade"], n)
        prec     = _sample_normal(rng, d["preceding"], n)
    else:  # average — bimodal: 80% on-time pool, 20% late pool → ~70% total on-time
        on_time_mask = rng.random(n) < 0.80
        delay = np.where(
            on_time_mask,
            rng.normal(*d["delay_ok"][:2],  n).clip(*d["delay_ok"][2:]),
            rng.normal(*d["delay_late"][:2], n).clip(*d["delay_late"][2:]),
        ).round().astype(np.int32)
        roll_m   = _sample_normal(rng, d["roll_mean"], n)
        roll_s   = _sample_normal(rng, d["roll_std"], n)
        roll_ot  = _sample_normal(rng, d["roll_ontime"], n)
        cong     = _sample_normal(rng, d["congestion"], n)
        cascade  = _sample_normal(rng, d["cascade"], n)
        prec     = _sample_normal(rng, d["preceding"], n)

    day["delay_mins"]              = delay
    day["weekday"]                 = np.int16(weekday)
    day["is_peak"]                 = np.where(
        day["departure_hour"].isin([7, 8, 16, 17, 18]) & (weekday < 5),
        np.float32(1), np.float32(0),
    )
    day["rolling_mean_7d"]         = roll_m
    day["rolling_std_7d"]          = roll_s
    day["rolling_ontime_7d"]       = roll_ot
    day["rolling_mean_14d"]        = (roll_m * rng.normal(1.0, 0.1, n)).clip(-2, 35).astype(np.float32)
    day["rolling_std_14d"]         = (roll_s  * rng.normal(1.0, 0.1, n)).clip( 1, 22).astype(np.float32)
    day["current_delay_mins"]      = (delay.astype(np.float32) * rng.uniform(0.85, 1.15, n)).astype(np.float32)
    day["preceding_delay_mins"]    = prec
    day["station_congestion_30m"]  = cong
    day["operator_cascade_delay"]  = cascade
    day["predecessor_train_delay"] = (prec * rng.normal(1.0, 0.2, n)).clip(-4, 50).astype(np.float32)
    day["volatility_score"]        = np.float32(0)
    # wind and mins_until_departure cloned from pool row (structural, not delay-derived)

    day["day_type"]       = day_type
    day["synth_week"]     = np.int16(synth_week)
    day["synthetic_date"] = synthetic_date
    day["generation"]     = generation
    return day


def generate(
    pool: pd.DataFrame,
    schedule: list[tuple[int, str]],
    generation: str,
    rows_per_day: int,
) -> pd.DataFrame:
    """Run the full schedule and return the combined DataFrame."""
    days_per_week = 7
    week_mondays = [pd.Timestamp(m) for m in WEEK_MONDAYS]
    weekday_names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]

    frames = []
    for i, (weekday, day_type) in enumerate(schedule):
        week_idx  = i // days_per_week
        monday    = week_mondays[week_idx]
        syn_date  = (monday + pd.Timedelta(days=weekday)).date()
        synth_wk  = week_idx + 1
        chunk = make_day(
            pool, weekday, day_type, synth_wk,
            syn_date, generation, rows_per_day, seed=3000 + i,
        )
        frames.append(chunk)
        avg = chunk["delay_mins"].mean()
        ontime_pct = (chunk["delay_mins"] <= 0).mean() * 100
        print(
            f"  Week {synth_wk} {weekday_names[weekday]} {syn_date}"
            f"  ({day_type:7s})  {len(chunk):>7,} rows"
            f"  avg={avg:+5.1f} min  on-time={ontime_pct:.0f}%"
        )

    return pd.concat(frames, ignore_index=True)


# ---------------------------------------------------------------------------
# DB I/O
# ---------------------------------------------------------------------------

def _load_pool(engine) -> pd.DataFrame:
    """Load the largest real day from delay_history as the pattern template."""
    pool = pd.read_sql(
        """
        SELECT uid, weekday, origin_crs, departure_hour, departure_hour AS dep_hour_raw
        FROM delay_history
        WHERE (recorded_at AT TIME ZONE 'UTC')::date = (
            SELECT (recorded_at AT TIME ZONE 'UTC')::date AS d
            FROM delay_history
            GROUP BY d ORDER BY COUNT(*) DESC LIMIT 1
        )
        """,
        engine,
    )
    # Structural columns not in delay_history — set to neutral defaults; the
    # generation step overwrites all delay-derived signals anyway.
    pool["wind_mph"]            = np.float32(0)
    pool["mins_until_departure"] = np.float32(0)
    if pool.empty:
        sys.exit("delay_history is empty — run the Darwin ingestion pipeline first.")
    print(f"  Pattern template: {len(pool):,} rows from largest real day")
    return pool


def generation_exists(engine, generation: str) -> int:
    with engine.connect() as conn:
        return conn.execute(
            text("SELECT COUNT(*) FROM delay_history_synthetic WHERE generation = :g"),
            {"g": generation},
        ).scalar() or 0


def save(df: pd.DataFrame, engine, generation: str) -> None:
    with engine.begin() as conn:
        conn.execute(
            text("DELETE FROM delay_history_synthetic WHERE generation = :g"),
            {"g": generation},
        )
    out = df[[c for c in _DB_COLS if c in df.columns]].copy()
    total = len(out)
    chunk = 10_000
    for start in range(0, total, chunk):
        out.iloc[start:start + chunk].to_sql(
            "delay_history_synthetic", engine,
            if_exists="append", index=False, method="multi",
        )
    print(f"  Saved {total:,} rows → delay_history_synthetic (generation={generation})")


# ---------------------------------------------------------------------------
# Public API (imported by compare_models.py)
# ---------------------------------------------------------------------------

def load_or_generate(
    engine,
    generation: str,
    schedule: list[tuple[int, str]] | None = None,
    rows_per_day: int = 250_000,
    force: bool = False,
) -> pd.DataFrame:
    """Return synthetic data from DB if cached, otherwise generate and save."""
    if schedule is None:
        schedule = DEFAULT_SCHEDULE

    existing = generation_exists(engine, generation)
    if existing and not force:
        print(f"  Cached: {existing:,} rows for generation={generation}")
        return pd.read_sql(
            "SELECT * FROM delay_history_synthetic WHERE generation = %(g)s",
            engine, params={"g": generation},
        )

    pool = _load_pool(engine)
    df   = generate(pool, schedule, generation, rows_per_day)
    save(df, engine, generation)
    return df


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------

def _build_engine(database_url: str):
    url = database_url.replace("@db:", "@localhost:").replace(
        "postgres://", "postgresql+psycopg2://", 1
    )
    return create_engine(url)


def main() -> None:
    load_dotenv(Path(__file__).parent.parent / ".env")

    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--weeks",        type=int,   default=3,            help="Number of synthetic weeks (default 3)")
    parser.add_argument("--rows-per-day", type=int,   default=250_000,      help="Rows sampled per synthetic day (default 250,000)")
    parser.add_argument("--generation",   type=str,   default=None,         help="Generation tag (default: auto from today's date)")
    parser.add_argument("--force",        action="store_true",               help="Regenerate even if generation already exists in DB")
    parser.add_argument("--dry-run",      action="store_true",               help="Print plan without writing to DB")
    args = parser.parse_args()

    raw_url = os.environ.get("DATABASE_URL", "")
    if not raw_url:
        sys.exit("DATABASE_URL not set — copy .env.example to .env and fill it in.")

    from datetime import date as _date
    generation = args.generation or f"synth-{_date.today().isoformat()}"
    days_per_week = 7
    # Extend WEEK_MONDAYS if more than 3 weeks requested
    global WEEK_MONDAYS
    while len(WEEK_MONDAYS) < args.weeks:
        last = pd.Timestamp(WEEK_MONDAYS[-1])
        WEEK_MONDAYS.append((last + pd.Timedelta(weeks=1)).strftime("%Y-%m-%d"))

    # Build schedule for requested weeks (repeat the 3-week pattern cyclically)
    base = DEFAULT_SCHEDULE  # 21 entries (3 × 7)
    schedule: list[tuple[int, str]] = []
    for w in range(args.weeks):
        week_template_idx = w % 3
        week_slice = base[week_template_idx * 7: week_template_idx * 7 + 7]
        schedule.extend(week_slice)

    print(f"\n══ Synthetic data generation ═════════════════════════════════")
    print(f"  Weeks:        {args.weeks}")
    print(f"  Days:         {len(schedule)}  ({sum(1 for _, t in schedule if t=='good')} good, "
          f"{sum(1 for _, t in schedule if t=='average')} average)")
    print(f"  Rows/day:     {args.rows_per_day:,}")
    print(f"  Total rows:   ~{len(schedule) * args.rows_per_day:,}")
    print(f"  Generation:   {generation}")
    print(f"  Mode:         {'DRY RUN' if args.dry_run else 'WRITE'}")

    if args.dry_run:
        weekday_names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
        print("\n  Schedule:")
        for i, (wd, dt) in enumerate(schedule):
            wk = i // days_per_week + 1
            monday = pd.Timestamp(WEEK_MONDAYS[i // days_per_week])
            syn_date = (monday + pd.Timedelta(days=wd)).date()
            print(f"    Week {wk}  {weekday_names[wd]}  {syn_date}  {dt}")
        return

    engine = _build_engine(raw_url)
    load_or_generate(engine, generation, schedule, args.rows_per_day, force=args.force)
    print("\n  Done.")


if __name__ == "__main__":
    main()
