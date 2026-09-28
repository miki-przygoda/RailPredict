"""
Compare LightGBM model variants and export the best as production ONNX models.

v2 improvements (2026-05-29):
  1. Fix preceding_delay_mins — was hardcoded 0; now computed via merge_asof.
  2. Fix mins_until_departure — was hardcoded 0; now approximated from recorded_at.
  3. Target clipping to [-30, 240].
  4. Bad-day exclusion (21 May, 27 May — startup reconnect artifacts).
  5. Boosted hyperparameters; early stopping on last-10% chronological val set.

v3 improvements (2026-05-29):
  6. Random 15% stratified test split (replaces fixed date split) — every tier
     is proportionally represented in both halves so metrics are unbiased.
  7. Sample weighting — equal weight per delay tier (on-time, slight, moderate,
     severe) so the 52%+ severe rows no longer dominate gradient updates.
     On-time MAE was 20+ min without this; expected <8 min after.
  8. Updated hyperparameters — n_estimators 1500, learning_rate 0.04,
     min_child_samples 100; calibrated for the larger post-HSP dataset.

v4 improvements (2026-05-30):
  9. Timezone fix — bad-day filter now casts recorded_at to UTC before date
     comparison; BST (+01:00) edge case was leaking ~11K rows from 21/27 May
     into training despite those days being listed as excluded.
     Per-day cap was tested and reverted — it removed 44% of training data and
     hurt the day-ahead model (MAE 14.12 → 20.57). More data wins.

v5 improvements (2026-05-30):
  10. 14-day rolling stats (rolling_mean_14d, rolling_std_14d) — longer window
      captures multi-week route patterns; day-ahead: 10→12 features,
      real-time: 15→18 features.
  11. Station congestion (station_congestion_30m) — mean delay of all trains at
      the same origin CRS in the prior 30 min; real-time model only. Captures
      network-level disruption beyond the single preceding-service signal.
  12. Historical weather from Open-Meteo archive API — wind_mph and
      volatility_score are now non-zero in training (were always 0 in v1–v4).

v6 improvements (2026-05-30):
  13. Fleet turnround (predecessor_train_delay) — delay of the same physical
      train set on its previous trip, sourced from Darwin Association (NP) at
      inference time.  Training proxy = preceding_delay_mins (same origin CRS,
      different UID, within 20 min).  Real-time: 19→21 features (also adds
      schedule_margin_mins from prior session, baked in simultaneously).

v7 improvements (2026-05-30):
  14. Synthetic augmentation — 3 weeks of artificial training data to teach the
      model what normal and average operation looks like (all 8 real days are
      heavily disrupted, avg +28–47 min).  Service patterns cloned from the
      largest real day; delay + rolling features generated self-consistently per
      day type.  Synthetic rows receive 0.4× sample weight so real observations
      remain dominant.  Test set is real data only.
      NOTE: v7 slightly degraded vs v6 (synthetic overwhelmed real signal).
            USE_SYNTHETIC=False for v7.5 while the approach is refined.

v7.5 improvements (2026-05-30):
  15. weekday_operator_enc — encodes the (weekday, UID-prefix) pair as a single
      categorical integer.  Captures operator-specific day-of-week patterns (e.g.
      operator G being disproportionately delayed on Mondays vs operator Y).
  16. operator_relative_delay — rolling_mean_7d minus the operator's mean
      rolling delay across all its services in the training window.  Tells the
      model how this service sits within its own operator's distribution rather
      than relative to the global average.  Inference: per-operator baseline
      stored in feature_meta.json → looked up by UID prefix at predict time.

v8 evaluation fixes (2026-09-28) -- leak removal, temporal holdout:
  17. current_delay_mins is no longer fabricated from the label.  v2-v7.5 built
      it as delay_mins * U(0.7, 1.3), i.e. the target plus noise, so the
      real-time model learned to copy it.  The real-time model now predicts the
      journey's FINAL observed origin delay from a snapshot taken earlier in the
      same journey: current_delay_mins is that snapshot's own reading (what
      Darwin reports at prediction time) and the target is the last reading of
      the journey.  Journey-final rows are not training rows (nothing left to
      predict).  A persistence baseline (predict final = current) is reported.
  18. Random stratified split replaced by a date-ordered holdout: the last ~15%
      of service dates are the test set, the ~10% of dates before that are the
      early-stopping validation set.  A journey never straddles the split.
  19. Rolling features only use PRIOR service dates (the current run's earlier
      snapshots were leaking into its own history).  The Rust side matches this
      (`HistoricalStore::rolling_stats_7d` ignores records from the current UTC day).
  20. Categorical encodings and operator means are fitted on the training
      dates only (previously on the full frame, test included).
  21. Synthetic rows are opt-in (--synthetic) and, when enabled, feed the
      day-ahead model only; their current_delay_mins is also label-derived, so
      the real-time model never sees them.  Never in validation or test.
  22. --since (default 2026-06-05) drops delay_history rows recorded before the
      v1.15.3 origin-delay fix; older labels measured journey duration.

Withdrawn results: every MAE published before v8 (day-ahead 13.10-14.39,
real-time 3.87-4.41) came from the leaky, randomly split, pre-label-fix
pipeline and is not a valid accuracy estimate.  See docs/model-performance.md.

Usage:
    python compare_models.py                    # labels from 2026-06-05 onwards
    python compare_models.py --since 2026-07-01
    python compare_models.py --since all        # no date floor (only if the DB
                                                # holds no pre-fix labels)

Outputs (written to ../models/):
    day_ahead.onnx
    realtime.onnx
    feature_meta.json
"""

import argparse
import json
import os
import sys
import warnings
from pathlib import Path

import numpy as np
import pandas as pd
from dotenv import load_dotenv
from lightgbm import LGBMRegressor, early_stopping, log_evaluation
from onnxmltools import convert_lightgbm
from onnxmltools.convert.common.data_types import FloatTensorType
from sklearn.metrics import mean_absolute_error, root_mean_squared_error
from sqlalchemy import create_engine, text

warnings.filterwarnings("ignore", category=UserWarning)

MODELS_DIR = Path(__file__).parent.parent / "models"

# delay_history labels recorded before this date are invalid: until the v1.15.3
# fix, reported_delay_mins measured journey progress (duration), not origin delay.
LABEL_FIX_DATE = "2026-06-05"

# Temporal holdout: last TEST_FRAC of service dates = test, the VAL_FRAC before
# that = early-stopping validation.  Both rounded to whole days, minimum 1 day.
TEST_FRAC = 0.15
VAL_FRAC  = 0.10

# A journey = one run of a pattern on one service date.
PATTERN_KEYS = ["uid", "weekday", "origin_crs", "departure_hour"]
JOURNEY_KEYS = PATTERN_KEYS + ["service_date"]

# Synthetic augmentation config (opt-in via --synthetic since v8).
# Injects a small slice of good-day synthetic rows to teach the model what
# on-time operation looks like.  Motivated by the pre-v1.15.3 data, where every
# real day looked 9-20% on-time -- an artefact of the journey-duration label bug,
# not real disruption -- so it is off by default on post-fix labels.  Note that
# generate_synthetic.py WRITES delay_history_synthetic if the generation is absent.
# SYNTH_GOOD_ROWS: cap at 2 good-day batches worth (~500k) — 13% of training set,
# so real signal stays dominant. SYNTH_WEIGHT 0.15× (vs 0.4× in v7) keeps gradient
# influence at ~3% — enough to nudge on-time predictions without overwhelming.
USE_SYNTHETIC    = False  # default; --synthetic overrides
SYNTH_GENERATION = "synth-2026-05-30"
SYNTH_GOOD_ROWS  = 500_000
SYNTH_WEIGHT     = 0.15

# Bad days to exclude (startup reconnect artifacts — extreme negative avg delay).
# Remove once HSP historical data is loaded; sample weights will handle noise then.
BAD_DAYS = ("2026-05-21", "2026-05-27")

FEATURE_COLS_DAY = [
    "weekday", "departure_hour", "month", "is_peak",
    "origin_crs_enc", "uid_prefix_enc",
    "rolling_mean_7d", "rolling_std_7d", "rolling_ontime_7d", "sample_count_log",
    "rolling_mean_14d", "rolling_std_14d",
    # v7.5 operator interaction features
    "weekday_operator_enc",
    "operator_relative_delay",
]

FEATURE_COLS_RT = FEATURE_COLS_DAY + [
    "current_delay_mins", "preceding_delay_mins",
    "wind_mph", "volatility_score", "mins_until_departure",
    "station_congestion_30m",
    "operator_cascade_delay",
    "predecessor_train_delay",
]

# v3 hyperparameters — calibrated for larger post-HSP dataset
LGBM_PARAMS = dict(
    n_estimators=1500,
    learning_rate=0.04,
    num_leaves=127,
    min_child_samples=100,   # stricter leaf floor now that we have more data
    subsample=0.7,
    colsample_bytree=0.8,
    n_jobs=-1,
    verbose=-1,
    random_state=42,
)

# ---------------------------------------------------------------------------
# Data loading
# ---------------------------------------------------------------------------

def load_data(database_url: str, since: str | None) -> pd.DataFrame:
    engine = create_engine(database_url)
    bad_days_sql = ", ".join(f"'{d}'" for d in BAD_DAYS)
    since_sql = "AND recorded_at >= CAST(:since AS date)" if since else ""
    sql = text(f"""
        SELECT uid, weekday, origin_crs, departure_hour,
               delay_mins, recorded_at
        FROM delay_history
        WHERE delay_mins BETWEEN -30 AND 240
          AND (recorded_at AT TIME ZONE 'UTC')::date NOT IN ({bad_days_sql})
          {since_sql}
        ORDER BY uid, weekday, origin_crs, departure_hour, recorded_at
    """)
    params = {"since": since} if since else {}
    with engine.connect() as conn:
        df = pd.read_sql(sql, conn, params=params, parse_dates=["recorded_at"])
    if df["recorded_at"].dt.tz is None:
        df["recorded_at"] = df["recorded_at"].dt.tz_localize("UTC")
    else:
        df["recorded_at"] = df["recorded_at"].dt.tz_convert("UTC")
    floor = f"since {since}" if since else "no date floor"
    print(f"  Loaded {len(df):,} observations  ({floor}; excluded: {', '.join(BAD_DAYS)})")
    return df


def add_journey_columns(df: pd.DataFrame) -> pd.DataFrame:
    """
    Tag each observation with its journey (pattern + UTC service date) and the
    journey's final observed delay.

    delay_history holds several snapshots per journey (record_outcome writes on
    every >=2 min change or every 5 min).  The last snapshot is the best known
    origin delay for that run: `final_delay_mins` is the real-time target and
    `is_journey_final` marks rows with nothing left to predict.
    """
    df = df.sort_values(PATTERN_KEYS + ["recorded_at"]).reset_index(drop=True)
    df["service_date"] = df["recorded_at"].dt.tz_convert("UTC").dt.normalize()
    grp = df.groupby(JOURNEY_KEYS, sort=False)["delay_mins"]
    df["final_delay_mins"] = grp.transform("last").astype(np.float32)
    df["is_journey_final"] = grp.cumcount(ascending=False).eq(0)
    n_j = df.groupby(JOURNEY_KEYS, sort=False).ngroups
    multi = (~df["is_journey_final"]).sum()
    print(f"  {n_j:,} journeys; {multi:,} non-final snapshots usable as real-time rows")
    return df


# ---------------------------------------------------------------------------
# Rolling features — O(n) per group via cumulative sums
# ---------------------------------------------------------------------------

def add_rolling_features(df: pd.DataFrame) -> pd.DataFrame:
    """
    7-/14-day rolling stats per pattern from PRIOR service dates only.

    Earlier snapshots of the same run are excluded: they are this journey's own
    (partial) outcome, not history.  Mirrors `HistoricalStore::rolling_stats_7d`,
    which ignores records from the current UTC day.  Requires add_journey_columns.
    """
    print("  Computing rolling features …", end="", flush=True)
    n = len(df)
    rolling_mean    = np.zeros(n, dtype=np.float32)
    rolling_std     = np.zeros(n, dtype=np.float32)
    rolling_ontime  = np.zeros(n, dtype=np.float32)
    rolling_count   = np.zeros(n, dtype=np.int32)
    rolling_mean14  = np.zeros(n, dtype=np.float32)
    rolling_std14   = np.zeros(n, dtype=np.float32)

    seven_days_ns    = np.timedelta64(7,  "D")
    fourteen_days_ns = np.timedelta64(14, "D")

    for _, group in df.groupby(PATTERN_KEYS, sort=False):
        idx    = group.index.values
        delays = group["delay_mins"].values.astype(np.float64)
        times  = group["recorded_at"].values
        days   = group["service_date"].values

        cum_sum    = np.concatenate([[0.0], np.cumsum(delays)])
        cum_sq     = np.concatenate([[0.0], np.cumsum(delays ** 2)])
        cum_ontime = np.concatenate([[0.0], np.cumsum(delays <= 0).astype(float)])
        left_7d    = np.searchsorted(times, times - seven_days_ns,    side="left")
        left_14d   = np.searchsorted(times, times - fourteen_days_ns, side="left")
        # Right edge: first observation of this row's own service date (exclusive).
        right      = np.searchsorted(times, days, side="left")

        for j in range(len(idx)):
            l7, l14, r = left_7d[j], left_14d[j], right[j]
            k7  = max(0, r - l7)
            k14 = max(0, r - l14)
            if k7 > 0:
                s  = cum_sum[r] - cum_sum[l7]
                s2 = cum_sq[r]  - cum_sq[l7]
                so = cum_ontime[r] - cum_ontime[l7]
                m  = s / k7
                rolling_mean[idx[j]]   = np.float32(m)
                rolling_std[idx[j]]    = np.float32(np.sqrt(max(0.0, s2 / k7 - m * m)))
                rolling_ontime[idx[j]] = np.float32((so / k7) * 100.0)
            rolling_count[idx[j]] = k7
            if k14 > 0:
                s14  = cum_sum[r] - cum_sum[l14]
                s2_14 = cum_sq[r] - cum_sq[l14]
                m14  = s14 / k14
                rolling_mean14[idx[j]] = np.float32(m14)
                rolling_std14[idx[j]]  = np.float32(np.sqrt(max(0.0, s2_14 / k14 - m14 * m14)))

    df["rolling_mean_7d"]   = rolling_mean
    df["rolling_std_7d"]    = rolling_std
    df["rolling_ontime_7d"] = rolling_ontime
    df["sample_count_log"]  = np.log1p(rolling_count).astype(np.float32)
    df["rolling_mean_14d"]  = rolling_mean14
    df["rolling_std_14d"]   = rolling_std14
    print(" done")
    return df


# ---------------------------------------------------------------------------
# Preceding delay feature — merge_asof within 20-min window
# ---------------------------------------------------------------------------

def compute_preceding_delay(df: pd.DataFrame) -> np.ndarray:
    """
    For each record find the most recent observation at the same TIPLOC by a
    *different* UID within the prior 20 minutes.  Returns a float32 array of
    the same length as df (0.0 where no predecessor exists).
    """
    print("  Computing preceding_delay_mins …", end="", flush=True)

    # merge_asof requires both sides sorted by the key column
    df_sorted = df[["uid", "origin_crs", "recorded_at", "delay_mins"]].copy()
    df_sorted = df_sorted.sort_values("recorded_at").reset_index(drop=False)
    # 'index' column now holds original df positions

    right = df_sorted[["uid", "origin_crs", "recorded_at", "delay_mins"]].rename(
        columns={
            "uid":        "uid_prev",
            "recorded_at": "rec_prev",
            "delay_mins":  "delay_prev",
        }
    )

    merged = pd.merge_asof(
        df_sorted[["index", "uid", "origin_crs", "recorded_at"]],
        right,
        left_on="recorded_at",
        right_on="rec_prev",
        by="origin_crs",
        tolerance=pd.Timedelta("20min"),
        allow_exact_matches=False,
    )

    # Zero out same-UID matches (merge_asof doesn't exclude them)
    same_uid = merged["uid"] == merged["uid_prev"]
    merged.loc[same_uid, "delay_prev"] = np.nan

    result = np.zeros(len(df), dtype=np.float32)
    orig_idx = merged["index"].values
    vals     = merged["delay_prev"].fillna(0.0).values.astype(np.float32)
    result[orig_idx] = vals

    pct_nonzero = (result != 0).mean() * 100
    print(f" done  ({pct_nonzero:.1f}% rows have a predecessor)")
    return result


# ---------------------------------------------------------------------------
# Station congestion feature — rolling 30-min mean delay at same origin CRS
# ---------------------------------------------------------------------------

def compute_station_congestion(df: pd.DataFrame) -> np.ndarray:
    """
    For each record: mean delay of ALL trains at the same origin_crs in the
    prior 30 minutes (inclusive of same UID — captures station-level chaos,
    not just a single preceding service).  Returns float32 array (0 = no signal).
    """
    print("  Computing station_congestion_30m …", end="", flush=True)

    df_sorted = df[["origin_crs", "recorded_at", "delay_mins"]].copy()
    df_sorted = df_sorted.sort_values("recorded_at").reset_index(drop=False)

    result = np.zeros(len(df), dtype=np.float32)
    thirty_min = np.timedelta64(30, "m")

    for _, group in df_sorted.groupby("origin_crs", sort=False):
        times     = group["recorded_at"].values
        delays    = group["delay_mins"].values.astype(np.float64)
        orig_idx  = group["index"].values
        cum_sum   = np.concatenate([[0.0], np.cumsum(delays)])
        left_bounds = np.searchsorted(times, times - thirty_min, side="left")
        for j in range(len(group)):
            l = left_bounds[j]
            k = j - l
            if k > 0:
                result[orig_idx[j]] = np.float32((cum_sum[j] - cum_sum[l]) / k)

    pct_nonzero = (result != 0).mean() * 100
    print(f" done  ({pct_nonzero:.1f}% rows have a congestion signal)")
    return result


# ---------------------------------------------------------------------------
# Operator cascade feature — rolling 60-min mean delay at same operator (UID prefix)
# ---------------------------------------------------------------------------

def compute_operator_cascade(df: pd.DataFrame) -> np.ndarray:
    """
    For each record: mean delay of ALL trains from the same operator (first char of UID)
    in the prior 60 minutes.  Returns 0.0 where fewer than 3 trains are in the window.

    Uses an hour-bucket approach (shift by 1 h to avoid leakage): the feature for a
    given row is the mean of all records in the *previous* complete hour for that
    operator prefix.  This is O(n) and leak-free at the cost of ~30 min of lag vs
    a true trailing window — acceptable for a coarse system-stress signal.
    """
    print("  Computing operator_cascade_delay …", end="", flush=True)

    df2 = df[["uid", "recorded_at", "delay_mins"]].copy()
    df2["op_prefix"]   = df2["uid"].str[0]
    df2["hour_bucket"] = df2["recorded_at"].dt.floor("1h")

    # Mean delay per (op_prefix, hour_bucket)
    hour_mean = (
        df2.groupby(["op_prefix", "hour_bucket"])["delay_mins"]
        .agg(["mean", "count"])
        .rename(columns={"mean": "op_mean", "count": "op_count"})
        .reset_index()
    )
    # Shift the bucket forward by 1 h so each row gets the *previous* hour's stats
    hour_mean["hour_bucket"] = hour_mean["hour_bucket"] + pd.Timedelta("1h")

    merged = df2[["op_prefix", "hour_bucket"]].merge(
        hour_mean, on=["op_prefix", "hour_bucket"], how="left"
    )
    # Zero out where fewer than 3 trains were in the window
    result = np.where(
        merged["op_count"].fillna(0) >= 3,
        merged["op_mean"].fillna(0.0),
        0.0,
    ).astype(np.float32)

    pct_nonzero = (result != 0).mean() * 100
    print(f" done  ({pct_nonzero:.1f}% rows have an operator cascade signal)")
    return result


# ---------------------------------------------------------------------------
# Historical weather — Open-Meteo archive API (Heathrow, UK proxy)
# ---------------------------------------------------------------------------

def fetch_training_weather(start_date: str, end_date: str) -> dict:
    """
    Fetch hourly wind speed (mph) from Open-Meteo archive API for the training
    period, anchored at London Heathrow (51.4775, -0.4614) as a UK-wide proxy.
    Returns a dict keyed by "YYYY-MM-DDTHH:00" → wind_mph float.
    Falls back to an empty dict on any network error.
    """
    import urllib.request as _req
    url = (
        "https://archive-api.open-meteo.com/v1/archive"
        f"?latitude=51.4775&longitude=-0.4614"
        f"&start_date={start_date}&end_date={end_date}"
        "&hourly=wind_speed_10m&wind_speed_unit=mph&timezone=UTC"
    )
    try:
        with _req.urlopen(url, timeout=30) as resp:
            data = json.loads(resp.read())
        times  = data["hourly"]["time"]
        speeds = data["hourly"]["wind_speed_10m"]
        lookup = {t: float(s) if s is not None else 0.0 for t, s in zip(times, speeds)}
        print(f"  Fetched {len(lookup)} hourly wind readings from Open-Meteo")
        return lookup
    except Exception as exc:
        print(f"  Warning: weather fetch failed ({exc}) — wind features will be 0")
        return {}


def assign_weather_features(df: pd.DataFrame, weather_lookup: dict) -> pd.DataFrame:
    """Assign wind_mph and volatility_score from the hourly lookup dict."""
    if not weather_lookup:
        df["wind_mph"]         = np.float32(0)
        df["volatility_score"] = np.float32(0)
        return df

    def _hour_key(ts):
        return ts.strftime("%Y-%m-%dT%H:00")

    wind = df["recorded_at"].apply(_hour_key).map(weather_lookup).fillna(0.0)
    df["wind_mph"] = wind.astype(np.float32)
    df["volatility_score"] = pd.cut(
        df["wind_mph"],
        bins=[-1, 20, 35, 50, 9999],
        labels=[0, 1, 2, 3],
    ).astype(np.float32)
    nonzero = (df["wind_mph"] > 0).mean() * 100
    print(f"  Weather assigned: mean wind={df['wind_mph'].mean():.1f} mph  "
          f"non-zero={nonzero:.1f}%")
    return df


# ---------------------------------------------------------------------------
# Feature engineering
# ---------------------------------------------------------------------------

def engineer_features(df: pd.DataFrame) -> pd.DataFrame:
    """
    Row-level features that need no fitting.  Every live signal is computed from
    observations strictly before (or at) the row's own recorded_at, i.e. what the
    engine could see at prediction time.  Encodings are fitted separately on the
    training dates (fit_encodings / apply_encodings).
    """
    df = df.copy()

    df["month"]   = df["recorded_at"].dt.month.astype(np.int32)
    df["is_peak"] = (
        df["departure_hour"].isin([7, 8, 16, 17, 18]) & (df["weekday"] <= 4)
    ).astype(np.float32)

    # current_delay_mins: the Darwin reading observed AT this snapshot.  At
    # inference it is status.reported_delay_mins at prediction time.  The
    # real-time target is final_delay_mins (the journey's last reading), so this
    # is a genuine earlier observation, not the label plus noise (v2-v7.5 leak).
    df["current_delay_mins"] = df["delay_mins"].astype(np.float32)

    # preceding_delay_mins: same origin, different UID, prior 20 min
    df["preceding_delay_mins"] = compute_preceding_delay(df)

    # mins_until_departure -- approximated from departure_hour vs recorded_at hour
    # Accurate to within ~30 min (we don't have departure minute in delay_history)
    rec_hour = df["recorded_at"].dt.hour.values + df["recorded_at"].dt.minute.values / 60.0
    dep_hour = df["departure_hour"].values.astype(float)
    mins_raw = (dep_hour - rec_hour) * 60.0
    # Overnight wrap: if result < −12 h the departure is on the next day
    mins_raw = np.where(mins_raw < -720.0, mins_raw + 1440.0, mins_raw)
    df["mins_until_departure"] = mins_raw.clip(-360.0, 360.0).astype(np.float32)

    # Station congestion: mean delay at origin CRS in prior 30 min
    df["station_congestion_30m"] = compute_station_congestion(df)

    # Operator cascade: mean delay of same operator (UID prefix) in previous hour
    df["operator_cascade_delay"] = compute_operator_cascade(df)

    # predecessor_train_delay: proxy using preceding_delay_mins (same origin, different
    # UID, within 20 min).  Training has no Association turnround data; at inference
    # the value comes from Darwin NP associations.  Known train/serve skew, not a leak.
    df["predecessor_train_delay"] = df["preceding_delay_mins"].values.copy()

    return df


def fit_encodings(train: pd.DataFrame) -> dict:
    """Categorical maps + per-operator mean, fitted on training dates only."""
    pfx = train["uid"].str[0]
    crs_map = {c: i + 1 for i, c in enumerate(sorted(train["origin_crs"].unique()))}
    pfx_map = {p: i + 1 for i, p in enumerate(sorted(pfx.unique()))}
    wd_op   = sorted((train["weekday"].astype(str) + "_" + pfx).unique())
    wd_op_map = {k: i + 1 for i, k in enumerate(wd_op)}
    op_mean = train.groupby(pfx)["rolling_mean_7d"].mean().to_dict()
    return {
        "crs":                 crs_map,
        "uid_prefix":          pfx_map,
        "weekday_operator":    wd_op_map,
        "operator_mean_delay": {k: float(v) for k, v in op_mean.items()},
    }


def apply_encodings(df: pd.DataFrame, meta: dict) -> pd.DataFrame:
    """Apply fitted encodings; unseen values map to 0 (OOV), as in onnx_engine.rs."""
    df = df.copy()
    pfx = df["uid"].str[0]
    df["origin_crs_enc"] = df["origin_crs"].map(meta["crs"]).fillna(0).astype(np.int32)
    df["uid_prefix_enc"] = pfx.map(meta["uid_prefix"]).fillna(0).astype(np.int32)
    wd_op = df["weekday"].astype(str) + "_" + pfx
    df["weekday_operator_enc"] = wd_op.map(meta["weekday_operator"]).fillna(0).astype(np.int32)
    df["operator_relative_delay"] = (
        df["rolling_mean_7d"].values
        - pfx.map(meta["operator_mean_delay"]).fillna(0.0).values
    ).astype(np.float32)
    return df


def temporal_split(df: pd.DataFrame, frac: float) -> tuple[pd.DataFrame, pd.DataFrame]:
    """
    Split by service date: the last `frac` of dates (min 1) form the later part.
    Whole days, so no journey straddles the boundary.
    """
    dates = np.sort(df["service_date"].unique())
    if len(dates) < 2:
        sys.exit(f"Need at least 2 service dates for a temporal split, have {len(dates)}")
    n_later = min(len(dates) - 1, max(1, int(round(len(dates) * frac))))
    cutoff = dates[-n_later]
    earlier = df[df["service_date"] < cutoff].copy()
    later   = df[df["service_date"] >= cutoff].copy()
    return earlier, later


def date_span(df: pd.DataFrame) -> str:
    d = df["service_date"]
    return f"{d.min():%Y-%m-%d} .. {d.max():%Y-%m-%d} ({d.nunique()} days)"


def tier_weights(target: pd.Series) -> np.ndarray:
    """Equal total weight per delay tier (on-time / slight / moderate / severe)."""
    tiers = pd.cut(target, bins=[-9999, 0, 5, 30, 9999],
                   labels=["ontime", "slight", "moderate", "severe"])
    counts = tiers.value_counts()
    counts = counts[counts > 0]
    w = {t: len(target) / (len(counts) * c) for t, c in counts.items()}
    for t, c in counts.items():
        print(f"    {t:<12}  count={c:>8,}  weight={w[t]:.4f}")
    return np.array(tiers.map(w).astype(np.float32), dtype=np.float32)


# ---------------------------------------------------------------------------
# Trimmed-mean baseline
# ---------------------------------------------------------------------------

def trimmed_mean_baseline(train: pd.DataFrame, test: pd.DataFrame) -> float:
    def _tmean(s):
        n = len(s)
        trim = max(1, int(n * 0.1))
        s_s = sorted(s)
        trimmed = s_s[trim: n - trim] if n - 2 * trim > 0 else s_s
        return float(np.mean(trimmed)) if trimmed else float(np.mean(s))

    agg = (
        train.groupby(["uid", "weekday", "origin_crs", "departure_hour"])["delay_mins"]
        .apply(_tmean).rename("pred_base").reset_index()
    )
    merged = test.merge(agg, on=["uid", "weekday", "origin_crs", "departure_hour"], how="left")
    merged["pred_base"] = merged["pred_base"].fillna(train["delay_mins"].mean())
    return mean_absolute_error(merged["delay_mins"], merged["pred_base"])


# ---------------------------------------------------------------------------
# Train + evaluate one variant
# ---------------------------------------------------------------------------

def evaluate_variant(
    label: str,
    fit: pd.DataFrame,
    val: pd.DataFrame,
    test: pd.DataFrame,
    feature_cols: list[str],
    target_col: str,
    sample_weight: np.ndarray | None = None,
    export_onnx: bool = False,
    model_name: str = "",
    params: dict | None = None,
) -> dict:
    """
    Fit on `fit`, early-stop on `val` (the dates just before the test window),
    score on `test` (the latest dates).  All three are disjoint in time.
    """
    params = dict(params or {})
    patience = params.pop("_early_stopping_patience", 50)
    effective_params = {**LGBM_PARAMS, **params}
    X_fit = fit[feature_cols].astype(np.float32).values
    y_fit = fit[target_col].values.clip(-30, 240)
    X_val = val[feature_cols].astype(np.float32).values
    y_val = val[target_col].values.clip(-30, 240)
    X_te  = test[feature_cols].astype(np.float32).values
    y_te  = test[target_col].values

    m = LGBMRegressor(**effective_params)
    m.fit(
        X_fit, y_fit,
        sample_weight=sample_weight,
        eval_set=[(X_val, y_val)],
        callbacks=[early_stopping(patience, verbose=False), log_evaluation(0)],
    )
    best_iter = m.best_iteration_ if m.best_iteration_ else effective_params["n_estimators"]

    preds = m.predict(X_te)

    mae  = mean_absolute_error(y_te, preds)
    rmse = root_mean_squared_error(y_te, preds)
    w2   = np.mean(np.abs(preds - y_te) <= 2)  * 100
    w5   = np.mean(np.abs(preds - y_te) <= 5)  * 100
    w10  = np.mean(np.abs(preds - y_te) <= 10) * 100
    bias = np.mean(preds - y_te)

    print(f"    best_iter={best_iter}  MAE={mae:.2f}  RMSE={rmse:.2f}  bias={bias:+.2f}"
          f"  ±2m={w2:.1f}%  ±5m={w5:.1f}%  ±10m={w10:.1f}%")

    if export_onnx and model_name:
        MODELS_DIR.mkdir(exist_ok=True)
        n_feat = len(feature_cols)
        init_types = [("float_input", FloatTensorType([None, n_feat]))]
        onnx_model = convert_lightgbm(m.booster_, initial_types=init_types, target_opset=12)
        out_path = MODELS_DIR / f"{model_name}.onnx"
        with open(out_path, "wb") as f:
            f.write(onnx_model.SerializeToString())
        print(f"    Exported → {out_path}  ({out_path.stat().st_size / 1024:.0f} KB)")

    return dict(label=label, n_train=len(fit), n_test=len(test), best_iter=best_iter,
                mae=mae, rmse=rmse, bias=bias, w2=w2, w5=w5, w10=w10, model=m)


# ---------------------------------------------------------------------------
# Feature importance
# ---------------------------------------------------------------------------

def print_importance(model: LGBMRegressor, cols: list[str], label: str) -> None:
    pairs = sorted(zip(cols, model.feature_importances_), key=lambda x: x[1], reverse=True)
    top_score = max(s for _, s in pairs)
    print(f"\n  [{label}] Feature importance:")
    for name, score in pairs:
        bar = "█" * int(score / top_score * 28)
        print(f"    {name:<26} {score:6.0f}  {bar}")


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(description="Train + evaluate RailPredict delay models")
    parser.add_argument(
        "--since", default=LABEL_FIX_DATE,
        help=f"Only use delay_history rows recorded on/after this date "
             f"(default {LABEL_FIX_DATE}, the v1.15.3 label fix). 'all' disables the floor.",
    )
    parser.add_argument(
        "--synthetic", action="store_true", default=USE_SYNTHETIC,
        help="Append synthetic good-day rows to the day-ahead fit set (writes "
             "delay_history_synthetic if the generation is missing).",
    )
    args = parser.parse_args()
    since = None if args.since == "all" else args.since
    use_synthetic = args.synthetic

    load_dotenv(Path(__file__).parent.parent / ".env")
    raw_url = os.environ.get("DATABASE_URL", "")
    if not raw_url:
        sys.exit("DATABASE_URL not set")
    database_url = raw_url.replace("@db:", "@localhost:").replace(
        "postgres://", "postgresql+psycopg2://", 1
    )

    print("\n══ Loading data ══════════════════════════════════════════════")
    df = load_data(database_url, since)
    if df.empty:
        sys.exit("No usable delay_history rows in range")

    by_day = df.groupby(df["recorded_at"].dt.date).agg(
        rows=("delay_mins", "count"), avg_delay=("delay_mins", "mean")
    )
    print("\n  Data by day (after exclusions and filter):")
    for d, row in by_day.iterrows():
        print(f"    {d}  {row['rows']:>8,} rows  avg={row['avg_delay']:+.1f} min")

    # Fetch historical weather for the training period (Open-Meteo archive, UTC)
    print("\n══ Weather features ══════════════════════════════════════════")
    dates = df["recorded_at"].dt.date
    weather = fetch_training_weather(str(dates.min()), str(dates.max()))

    print("\n══ Journeys + rolling features ═══════════════════════════════")
    df = add_journey_columns(df)
    df = add_rolling_features(df)

    print("\n══ Feature engineering ═══════════════════════════════════════")
    df = engineer_features(df)
    df = assign_weather_features(df, weather)

    # ---------------------------------------------------------------------------
    # Temporal holdout: [ fit dates | val dates | test dates ]
    # ---------------------------------------------------------------------------
    train_all, test = temporal_split(df, TEST_FRAC)
    fit_real, val   = temporal_split(train_all, VAL_FRAC / (1.0 - TEST_FRAC))
    print("\n  Temporal split (by service date):")
    print(f"    fit   {date_span(fit_real)}  {len(fit_real):>9,} rows")
    print(f"    val   {date_span(val)}  {len(val):>9,} rows  (early stopping)")
    print(f"    test  {date_span(test)}  {len(test):>9,} rows")

    # Encodings from fit dates only; OOV -> 0 exactly as at inference.
    meta = fit_encodings(fit_real)
    fit_real = apply_encodings(fit_real, meta)
    val      = apply_encodings(val, meta)
    test     = apply_encodings(test, meta)

    # ---------------------------------------------------------------------------
    # Day-ahead fit set (optionally + synthetic good-day rows)
    # ---------------------------------------------------------------------------
    fit_day = fit_real
    is_synth_mask = np.zeros(len(fit_day), dtype=bool)
    if use_synthetic:
        sys.path.insert(0, str(Path(__file__).parent))
        from generate_synthetic import load_or_generate as _load_synth  # noqa: E402

        engine = create_engine(database_url)
        print("\n══ Synthetic augmentation (day-ahead only) ═══════════════════")
        synth_all = _load_synth(engine, SYNTH_GENERATION)
        good_pool = synth_all[synth_all["day_type"] == "good"]
        synth_raw = good_pool.sample(
            n=min(SYNTH_GOOD_ROWS, len(good_pool)), random_state=42
        ).copy()
        synth_raw["month"] = 4
        if "sample_count_log" not in synth_raw:
            synth_raw["sample_count_log"] = np.float32(np.log1p(20))
        synth_raw = apply_encodings(synth_raw, meta)
        print(f"  {len(synth_raw):,} synthetic rows appended to the day-ahead fit set "
              f"(weight {SYNTH_WEIGHT}x; never in val/test, never in the real-time model)")
        fit_day = pd.concat([fit_real, synth_raw], ignore_index=True)
        is_synth_mask = np.r_[np.zeros(len(fit_real), bool), np.ones(len(synth_raw), bool)]

    print("\n  Day-ahead sample weights (equal-tier):")
    w_day = tier_weights(fit_day["delay_mins"])
    w_day[is_synth_mask] *= SYNTH_WEIGHT

    # ---------------------------------------------------------------------------
    # Real-time sets: non-final snapshots, target = journey's final delay
    # ---------------------------------------------------------------------------
    rt_fit  = fit_real[~fit_real["is_journey_final"]]
    rt_val  = val[~val["is_journey_final"]]
    rt_test = test[~test["is_journey_final"]]
    rt_ok = min(len(rt_fit), len(rt_val), len(rt_test)) > 0
    if rt_ok:
        print("\n  Real-time sample weights (equal-tier on final delay):")
        w_rt = tier_weights(rt_fit["final_delay_mins"])
    else:
        print("\n  Real-time model skipped: need multi-snapshot journeys in fit, val and test")
        print("  WARNING: models/realtime.onnx (if present) was NOT retrained and may be a "
              "pre-v8 leaky model -- delete it to fall back to the day-ahead model.")

    # Save feature metadata
    meta_path = MODELS_DIR / "feature_meta.json"
    MODELS_DIR.mkdir(exist_ok=True)
    with open(meta_path, "w") as f:
        json.dump(meta, f)
    print(f"\n  Feature metadata → {meta_path}")
    print(f"  ({len(meta['crs'])} TIPLOC encodings, {len(meta['uid_prefix'])} UID prefixes)")

    # ---------------------------------------------------------------------------
    # Baselines
    # ---------------------------------------------------------------------------
    print("\n══ Baselines ═════════════════════════════════════════════════")
    bl = trimmed_mean_baseline(train_all, test)
    print(f"  Day-ahead trimmed-mean baseline MAE: {bl:.2f} min")
    persist = None
    if rt_ok:
        persist = mean_absolute_error(rt_test["final_delay_mins"], rt_test["current_delay_mins"])
        print(f"  Real-time persistence baseline MAE (final = current): {persist:.2f} min")

    # High-convergence hyperparameters — 3k tree budget, finer lr, more patience.
    HC_PARAMS = dict(
        n_estimators=3000,
        learning_rate=0.015,
        num_leaves=255,
        min_child_samples=100,
        _early_stopping_patience=100,
    )

    results = []
    label_sfx = "-synth" if use_synthetic else ""

    print(f"\n══ Day-ahead model ({len(FEATURE_COLS_DAY)} features) ═══════════════════════════")
    results.append(evaluate_variant(
        f"v8{label_sfx} day-ahead", fit_day, val, test, FEATURE_COLS_DAY, "delay_mins",
        sample_weight=w_day, export_onnx=True, model_name="day_ahead",
    ))

    print(f"\n══ Day-ahead HC ({len(FEATURE_COLS_DAY)} features, 3k trees) ══════════════")
    results.append(evaluate_variant(
        f"v8{label_sfx}-HC day-ahead", fit_day, val, test, FEATURE_COLS_DAY, "delay_mins",
        sample_weight=w_day, export_onnx=True, model_name="day_ahead_hc", params=HC_PARAMS,
    ))

    if rt_ok:
        print(f"\n══ Real-time model ({len(FEATURE_COLS_RT)} features) ════════════════════════════")
        results.append(evaluate_variant(
            "v8 real-time", rt_fit, rt_val, rt_test, FEATURE_COLS_RT, "final_delay_mins",
            sample_weight=w_rt, export_onnx=True, model_name="realtime",
        ))

        print(f"\n══ Real-time HC ({len(FEATURE_COLS_RT)} features, 3k trees) ═══════════════════")
        results.append(evaluate_variant(
            "v8-HC real-time", rt_fit, rt_val, rt_test, FEATURE_COLS_RT, "final_delay_mins",
            sample_weight=w_rt, export_onnx=True, model_name="realtime_hc", params=HC_PARAMS,
        ))

    # ---------------------------------------------------------------------------
    # Comparison table
    # ---------------------------------------------------------------------------
    print("\n" + "═" * 80)
    print(f"  COMPARISON -- temporal holdout, test dates {date_span(test)}")
    print("═" * 80)
    print(f"  {'Model':<40} {'best_iter':>9} {'MAE':>7} {'RMSE':>8} "
          f"{'Bias':>6} {'±2m':>5} {'±5m':>5} {'±10m':>6}")
    print("  " + "─" * 80)
    print(f"  {'day-ahead trimmed-mean baseline':<40} {'-':>9} {bl:>7.2f}")
    if persist is not None:
        print(f"  {'real-time persistence baseline':<40} {'-':>9} {persist:>7.2f}")
    print("  " + "─" * 80)
    for r in results:
        print(f"  {r['label']:<40} {r['best_iter']:>9,} {r['mae']:>7.2f} {r['rmse']:>8.2f} "
              f"{r['bias']:>+6.2f} {r['w2']:>4.1f}% {r['w5']:>4.1f}% {r['w10']:>5.1f}%")
    print("═" * 80)

    day_r = next(r for r in results if "day-ahead" in r["label"] and "HC" not in r["label"])
    print_importance(day_r["model"], FEATURE_COLS_DAY, day_r["label"])
    rt_r = next((r for r in results if "real-time" in r["label"] and "HC" not in r["label"]), None)
    if rt_r is not None:
        print_importance(rt_r["model"], FEATURE_COLS_RT, rt_r["label"])

    print("\n  Models exported to models/ -- restart the Rust server to activate.\n")

    # Write benchmarks.json — picked up by the export module for the index page.
    from datetime import datetime as _dt

    def _metrics(r: dict) -> dict:
        return {
            "mae":          round(float(r["mae"]),  2),
            "rmse":         round(float(r["rmse"]), 2),
            "bias":         round(float(r["bias"]), 2),
            "within_2min":  round(float(r["w2"]),   1),
            "within_5min":  round(float(r["w5"]),   1),
            "within_10min": round(float(r["w10"]),  1),
        }

    benchmarks = {
        "status":       "valid",
        "evaluation":   "temporal holdout by service date (no shuffling); "
                        "real-time target = journey final delay from an earlier snapshot",
        "trained_at":   _dt.now().strftime("%Y-%m-%d"),
        "labels_since": since or "all",
        "fit_dates":    date_span(fit_real),
        "val_dates":    date_span(val),
        "test_dates":   date_span(test),
        "train_rows":   int(len(fit_day)),
        "test_rows":    int(len(test)),
        "baseline_mae": round(float(bl), 2),
        "day_ahead":    _metrics(day_r),
    }
    if rt_r is not None:
        benchmarks["realtime"] = _metrics(rt_r)
        benchmarks["realtime"]["test_rows"] = int(len(rt_test))
        benchmarks["realtime_persistence_mae"] = round(float(persist), 2)
    benchmarks_path = MODELS_DIR / "benchmarks.json"
    with open(benchmarks_path, "w") as f:
        json.dump(benchmarks, f, indent=2)
    print(f"  Benchmarks → {benchmarks_path}\n")


if __name__ == "__main__":
    main()
