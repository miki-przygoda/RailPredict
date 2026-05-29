"""
export_dataset.py — Export delay_history to Parquet and regenerate dataset/README.md.

Run after each model retrain or when the dataset has grown significantly:
    python3 scripts/export_dataset.py

Outputs (both in dataset/):
    delay_history.parquet   — clean Parquet file ready to push to HuggingFace
    README.md               — HuggingFace dataset card with current stats injected

Requirements:
    pip install pyarrow pandas python-dotenv sqlalchemy psycopg2-binary
"""

import os
import sys
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import pandas as pd
import pyarrow as pa
import pyarrow.parquet as pq
from dotenv import load_dotenv
from sqlalchemy import create_engine, text

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

REPO_ROOT   = Path(__file__).parent.parent
DATASET_DIR = REPO_ROOT / "dataset"
PARQUET_OUT = DATASET_DIR / "delay_history.parquet"
README_OUT  = DATASET_DIR / "README.md"

# Days with known data quality issues (startup reconnect artifacts).
BAD_DAYS = ("2026-05-21", "2026-05-27")

# ---------------------------------------------------------------------------
# Load
# ---------------------------------------------------------------------------

def load(database_url: str) -> pd.DataFrame:
    engine = create_engine(database_url)
    bad_sql = ", ".join(f"'{d}'" for d in BAD_DAYS)
    sql = text(f"""
        SELECT
            uid,
            origin_crs,
            weekday,
            departure_hour,
            delay_mins,
            recorded_at
        FROM delay_history
        WHERE delay_mins BETWEEN -30 AND 240
          AND recorded_at::date NOT IN ({bad_sql})
        ORDER BY recorded_at
    """)
    with engine.connect() as conn:
        df = pd.read_sql(sql, conn, parse_dates=["recorded_at"])
    if df["recorded_at"].dt.tz is None:
        df["recorded_at"] = df["recorded_at"].dt.tz_localize("UTC")
    print(f"  Loaded {len(df):,} rows from DB")
    return df

# ---------------------------------------------------------------------------
# Export
# ---------------------------------------------------------------------------

def export_parquet(df: pd.DataFrame) -> dict:
    schema = pa.schema([
        pa.field("uid",            pa.string()),
        pa.field("origin_crs",     pa.string()),
        pa.field("weekday",        pa.int8()),
        pa.field("departure_hour", pa.int8()),
        pa.field("delay_mins",     pa.int16()),
        pa.field("recorded_at",    pa.timestamp("us", tz="UTC")),
    ])

    # Cast to correct types before writing.
    df = df.copy()
    df["weekday"]        = df["weekday"].astype("int8")
    df["departure_hour"] = df["departure_hour"].astype("int8")
    df["delay_mins"]     = df["delay_mins"].astype("int16")
    df["uid"]            = df["uid"].astype("str")
    df["origin_crs"]     = df["origin_crs"].astype("str")

    table = pa.Table.from_pandas(df, schema=schema, preserve_index=False)
    pq.write_table(table, PARQUET_OUT, compression="snappy")

    size_mb = PARQUET_OUT.stat().st_size / 1_048_576
    print(f"  Exported {len(df):,} rows → {PARQUET_OUT.name}  ({size_mb:.1f} MB)")

    # Compute stats for the README.
    tier_fn = lambda x: (
        "on-time"  if x <= 0  else
        "slight"   if x <= 5  else
        "moderate" if x <= 30 else
        "severe"
    )
    tiers = df["delay_mins"].apply(tier_fn).value_counts()
    total = len(df)

    return {
        "rows":          f"{total:,}",
        "size_mb":       f"{size_mb:.1f}",
        "stations":      f"{df['origin_crs'].nunique():,}",
        "date_min":      df["recorded_at"].min().strftime("%Y-%m-%d"),
        "date_max":      df["recorded_at"].max().strftime("%Y-%m-%d"),
        "avg_delay":     f"{df['delay_mins'].mean():.1f}",
        "pct_ontime":    f"{100 * tiers.get('on-time',  0) / total:.1f}",
        "pct_slight":    f"{100 * tiers.get('slight',   0) / total:.1f}",
        "pct_moderate":  f"{100 * tiers.get('moderate', 0) / total:.1f}",
        "pct_severe":    f"{100 * tiers.get('severe',   0) / total:.1f}",
        "exported_at":   datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC"),
    }

# ---------------------------------------------------------------------------
# README
# ---------------------------------------------------------------------------

README_TEMPLATE = """\
---
language:
- en
license: other
license_name: network-rail-open-data-licence
license_link: https://www.networkrail.co.uk/who-we-are/transparency-and-ethics/transparency/open-data-feeds/network-rail-infrastructure-limited-data-feeds-licence/
task_categories:
- tabular-regression
tags:
- uk-rail
- train-delays
- national-rail
- darwin
- punctuality
pretty_name: UK Rail Departure Delays (Darwin Push Port)
size_categories:
- 1M<n<10M
---

# UK Rail Departure Delays — Darwin Push Port

Row-level departure delay records for every active UK rail station, collected
live from the **National Rail Darwin Push Port** STOMP feed.  Each row is one
observed departure: planned time vs. actual time, expressed as `delay_mins`
(positive = late, negative = early).

Built to train delay-prediction models for [RailPredict](https://github.com/miki-przygoda/RailPredict).

---

## Current snapshot

| Field | Value |
|---|---|
| **Rows** | {rows} |
| **Stations (CRS codes)** | {stations} |
| **Date range** | {date_min} → {date_max} |
| **Parquet size** | {size_mb} MB (Snappy compressed) |
| **Exported** | {exported_at} |

### Delay tier distribution

| Tier | Definition | Share |
|---|---|---|
| On-time | ≤ 0 min | {pct_ontime}% |
| Slight | 1–5 min | {pct_slight}% |
| Moderate | 6–30 min | {pct_moderate}% |
| Severe | > 30 min | {pct_severe}% |

---

## Known biases — read before training

**Severe delays are heavily overrepresented.**
The Darwin feed generates a training record every time a train's delay changes
by ≥ 2 minutes, but only every 5 minutes for on-time trains.  In a typical
collection period, severe delays (~2% of real-world departures) account for
~53% of rows; on-time departures (~85% real-world) account for ~10%.

**Short collection window.**
This snapshot covers {date_min} → {date_max}.  There is no seasonality signal,
no winter weather data, and limited route diversity.  Models trained on it will
not generalise to Christmas, summer timetable changes, or weather disruption.

**Correcting for the bias (recommended):**
Apply equal-tier sample weighting before training so each tier contributes 25%
of the total gradient weight:

```python
import pandas as pd

df = pd.read_parquet("delay_history.parquet")

tier_fn = lambda x: (
    "ontime"   if x <= 0  else
    "slight"   if x <= 5  else
    "moderate" if x <= 30 else
    "severe"
)
tier_series  = df["delay_mins"].apply(tier_fn)
tier_counts  = tier_series.value_counts()
tier_weight  = {{t: len(df) / (4 * tier_counts[t]) for t in tier_counts.index}}
sample_weight = tier_series.map(tier_weight).values
```

Pass `sample_weight=sample_weight` to your model's `.fit()` call.

---

## Schema

| Column | Type | Description |
|---|---|---|
| `uid` | string | Service identifier (date prefix stripped from Darwin RID) |
| `origin_crs` | string | 3-character CRS station code (e.g. `KGX`, `MAN`) |
| `weekday` | int8 | Day of week: 0 = Monday … 6 = Sunday |
| `departure_hour` | int8 | Planned departure hour (0–23, UTC) |
| `delay_mins` | int16 | Departure delay in minutes (negative = early) |
| `recorded_at` | timestamp[utc] | UTC timestamp of the planned departure |

---

## Usage

```python
import pandas as pd

df = pd.read_parquet("delay_history.parquet")
print(df.head())
print(df["delay_mins"].describe())
```

Or directly from the HuggingFace Hub:

```python
from datasets import load_dataset

ds = load_dataset("miki-przygoda/uk-rail-delays")
df = ds["train"].to_pandas()
```

---

## Source & licence

Data derived from the **National Rail Darwin Push Port** feed, provided by
Network Rail under the
[Network Rail Open Data Licence](https://www.networkrail.co.uk/who-we-are/transparency-and-ethics/transparency/open-data-feeds/network-rail-infrastructure-limited-data-feeds-licence/).
You must comply with that licence when redistributing or building products on
this data.

Collected and published by [@miki-przygoda](https://github.com/miki-przygoda)
as part of the [RailPredict](https://github.com/miki-przygoda/RailPredict) project.
"""

def write_readme(stats: dict):
    readme = README_TEMPLATE.format(**stats)
    README_OUT.write_text(readme)
    print(f"  README written → {README_OUT}")

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    load_dotenv(REPO_ROOT / "RailPredict" / ".env")
    database_url = os.environ.get("DATABASE_URL", "")
    if not database_url:
        sys.exit("DATABASE_URL not set")
    database_url = database_url.replace("@db:", "@localhost:")

    DATASET_DIR.mkdir(exist_ok=True)

    print("\n── Loading data ─────────────────────────────────────")
    df = load(database_url)

    print("\n── Exporting Parquet ────────────────────────────────")
    stats = export_parquet(df)

    print("\n── Writing README ───────────────────────────────────")
    write_readme(stats)

    print(f"""
── Done ─────────────────────────────────────────────
  {stats['rows']} rows  |  {stats['size_mb']} MB  |  {stats['stations']} stations
  {stats['date_min']} → {stats['date_max']}

  Tier split:  on-time {stats['pct_ontime']}%  slight {stats['pct_slight']}%  moderate {stats['pct_moderate']}%  severe {stats['pct_severe']}%

  Next steps:
    1. huggingface-cli login
    2. huggingface-cli upload miki-przygoda/uk-rail-delays dataset/delay_history.parquet
    3. huggingface-cli upload miki-przygoda/uk-rail-delays dataset/README.md
""")
