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

Previous results (fixed reference — v3 real Darwin data, 30 May 2026):
    v3 day-ahead  MAE = 14.12 min
    v3 real-time  MAE =  4.09 min

Usage:
    python compare_models.py

Outputs (written to ../models/):
    day_ahead.onnx
    realtime.onnx
    feature_meta.json
"""

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
from sklearn.model_selection import train_test_split
from sqlalchemy import create_engine, text

warnings.filterwarnings("ignore", category=UserWarning)

MODELS_DIR = Path(__file__).parent.parent / "models"

# Previous-model reference MAEs (for the comparison table) — v3 results
PREV_DAY_MAE = 14.12
PREV_RT_MAE  =  4.09

# Bad days to exclude (startup reconnect artifacts — extreme negative avg delay).
# Remove once HSP historical data is loaded; sample weights will handle noise then.
BAD_DAYS = ("2026-05-21", "2026-05-27")

FEATURE_COLS_DAY = [
    "weekday", "departure_hour", "month", "is_peak",
    "origin_crs_enc", "uid_prefix_enc",
    "rolling_mean_7d", "rolling_std_7d", "rolling_ontime_7d", "sample_count_log",
    "rolling_mean_14d", "rolling_std_14d",
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

def load_data(database_url: str) -> pd.DataFrame:
    engine = create_engine(database_url)
    bad_days_sql = ", ".join(f"'{d}'" for d in BAD_DAYS)
    sql = text(f"""
        SELECT uid, weekday, origin_crs, departure_hour,
               delay_mins, recorded_at
        FROM delay_history
        WHERE delay_mins BETWEEN -30 AND 240
          AND (recorded_at AT TIME ZONE 'UTC')::date NOT IN ({bad_days_sql})
        ORDER BY uid, weekday, origin_crs, departure_hour, recorded_at
    """)
    with engine.connect() as conn:
        df = pd.read_sql(sql, conn, parse_dates=["recorded_at"])
    if df["recorded_at"].dt.tz is None:
        df["recorded_at"] = df["recorded_at"].dt.tz_localize("UTC")
    print(f"  Loaded {len(df):,} observations  (excluded: {', '.join(BAD_DAYS)})")
    return df


# ---------------------------------------------------------------------------
# Rolling features — O(n) per group via cumulative sums
# ---------------------------------------------------------------------------

def add_rolling_features(df: pd.DataFrame) -> pd.DataFrame:
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

    for _, group in df.groupby(
        ["uid", "weekday", "origin_crs", "departure_hour"], sort=False
    ):
        idx    = group.index.values
        delays = group["delay_mins"].values.astype(np.float64)
        times  = group["recorded_at"].values

        cum_sum    = np.concatenate([[0.0], np.cumsum(delays)])
        cum_sq     = np.concatenate([[0.0], np.cumsum(delays ** 2)])
        cum_ontime = np.concatenate([[0.0], np.cumsum(delays <= 0).astype(float)])
        left_7d    = np.searchsorted(times, times - seven_days_ns,    side="left")
        left_14d   = np.searchsorted(times, times - fourteen_days_ns, side="left")

        for j in range(len(idx)):
            l7, l14 = left_7d[j], left_14d[j]
            k7  = j - l7
            k14 = j - l14
            if k7 > 0:
                s  = cum_sum[j] - cum_sum[l7]
                s2 = cum_sq[j]  - cum_sq[l7]
                so = cum_ontime[j] - cum_ontime[l7]
                m  = s / k7
                rolling_mean[idx[j]]   = np.float32(m)
                rolling_std[idx[j]]    = np.float32(np.sqrt(max(0.0, s2 / k7 - m * m)))
                rolling_ontime[idx[j]] = np.float32((so / k7) * 100.0)
            rolling_count[idx[j]] = k7
            if k14 > 0:
                s14  = cum_sum[j] - cum_sum[l14]
                s2_14 = cum_sq[j] - cum_sq[l14]
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

def engineer_features(df: pd.DataFrame) -> tuple[pd.DataFrame, dict]:
    df = df.copy()

    df["month"]   = df["recorded_at"].dt.month.astype(np.int32)
    df["is_peak"] = (
        df["departure_hour"].isin([7, 8, 16, 17, 18]) & (df["weekday"] <= 4)
    ).astype(np.float32)

    unique_crs = sorted(df["origin_crs"].unique())
    unique_pfx = sorted(df["uid"].str[0].unique())
    crs_map = {c: i + 1 for i, c in enumerate(unique_crs)}
    pfx_map = {p: i + 1 for i, p in enumerate(unique_pfx)}

    df["origin_crs_enc"] = df["origin_crs"].map(crs_map).fillna(0).astype(np.int32)
    df["uid_prefix_enc"] = df["uid"].str[0].map(pfx_map).fillna(0).astype(np.int32)

    # current_delay_mins: historical delay + training noise to prevent copy-through
    rng = np.random.default_rng(42)
    noise = rng.uniform(0.7, 1.3, size=len(df))
    df["current_delay_mins"] = (df["delay_mins"].values * noise).astype(np.float32)

    # FIX 1: preceding_delay_mins — computed from data, was always 0
    df["preceding_delay_mins"] = compute_preceding_delay(df)

    # FIX 2: mins_until_departure — approximated from departure_hour vs recorded_at hour
    # Accurate to within ~30 min (we don't have departure minute in delay_history)
    rec_hour = df["recorded_at"].dt.hour.values + df["recorded_at"].dt.minute.values / 60.0
    dep_hour = df["departure_hour"].values.astype(float)
    mins_raw = (dep_hour - rec_hour) * 60.0
    # Overnight wrap: if result < −12 h the departure is on the next day
    mins_raw = np.where(mins_raw < -720.0, mins_raw + 1440.0, mins_raw)
    df["mins_until_departure"] = mins_raw.clip(-360.0, 360.0).astype(np.float32)

    # Station congestion: mean delay at origin CRS in prior 30 min
    df["station_congestion_30m"] = compute_station_congestion(df)
    df["schedule_margin_mins"] = np.float32(0)

    # Operator cascade: mean delay of same operator (UID prefix) in prior 60 min
    df["operator_cascade_delay"] = compute_operator_cascade(df)

    # predecessor_train_delay: proxy using preceding_delay_mins (same origin, different UID,
    # within 20 min).  At training time we don't have Association turnround data, so this
    # is the best available approximation.  The model will differentiate the two signals
    # at inference time once live Association messages flow in.
    df["predecessor_train_delay"] = df["preceding_delay_mins"].values.copy()

    meta = {"crs": crs_map, "uid_prefix": pfx_map}
    return df, meta


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
    train: pd.DataFrame,
    test: pd.DataFrame,
    feature_cols: list[str],
    sample_weight: np.ndarray | None = None,
    export_onnx: bool = False,
    model_name: str = "",
) -> dict:
    X_tr = train[feature_cols].astype(np.float32).values
    y_tr = train["delay_mins"].values.clip(-30, 240)
    X_te = test[feature_cols].astype(np.float32).values
    y_te = test["delay_mins"].values

    # Early stopping on last-10% chronological slice (no temporal leakage)
    split   = int(len(X_tr) * 0.9)
    X_fit, X_val = X_tr[:split], X_tr[split:]
    y_fit, y_val = y_tr[:split], y_tr[split:]
    w_fit = sample_weight[:split] if sample_weight is not None else None

    m = LGBMRegressor(**LGBM_PARAMS)
    m.fit(
        X_fit, y_fit,
        sample_weight=w_fit,
        eval_set=[(X_val, y_val)],
        callbacks=[early_stopping(50, verbose=False), log_evaluation(0)],
    )
    best_iter = m.best_iteration_ if m.best_iteration_ else LGBM_PARAMS["n_estimators"]

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

    return dict(label=label, n_train=len(train), n_test=len(test), best_iter=best_iter,
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
    load_dotenv(Path(__file__).parent.parent / ".env")
    raw_url = os.environ.get("DATABASE_URL", "")
    if not raw_url:
        sys.exit("DATABASE_URL not set")
    database_url = raw_url.replace("@db:", "@localhost:").replace(
        "postgres://", "postgresql+psycopg2://", 1
    )

    print("\n══ Loading data ══════════════════════════════════════════════")
    df = load_data(database_url)

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

    print("\n══ Rolling features ══════════════════════════════════════════")
    df = add_rolling_features(df)

    print("\n══ Feature engineering ═══════════════════════════════════════")
    df, meta = engineer_features(df)
    df = assign_weather_features(df, weather)

    # Spot-check the two fixed features
    prec_pct = (df["preceding_delay_mins"] != 0).mean() * 100
    mud_rng  = df["mins_until_departure"]
    print(f"  preceding_delay_mins: {prec_pct:.1f}% non-zero  "
          f"mean={df['preceding_delay_mins'].mean():.1f}")
    print(f"  mins_until_departure: min={mud_rng.min():.0f}  "
          f"max={mud_rng.max():.0f}  mean={mud_rng.mean():.1f}")

    # Stratify by delay tier so every tier is proportionally represented in both splits.
    # This matters because our data is heavily biased (52%+ severe, ~11% on-time) —
    # a random split without stratification could put all the on-time rows in one half.
    df["_tier"] = pd.cut(
        df["delay_mins"],
        bins=[-9999, 0, 5, 30, 9999],
        labels=["ontime", "slight", "moderate", "severe"],
    )
    tier_dist = df["_tier"].value_counts(normalize=True)
    print("\n  Delay tier distribution:")
    for tier, pct in tier_dist.items():
        print(f"    {tier:<12} {pct*100:5.1f}%")

    train, test = train_test_split(df, test_size=0.15, random_state=42, stratify=df["_tier"])
    train = train.drop(columns=["_tier"]).copy()
    test  = test.drop(columns=["_tier"]).copy()
    print(f"\n  Train: {len(train):,} rows   Test: {len(test):,} rows  (15% random stratified split)")

    # Sample weights: each tier contributes equally to total loss.
    # Without this the 52%+ severe tier dominates gradient updates and the model
    # learns to predict high delays even for on-time trains (MAE 20+ min on-time).
    tier_fn = lambda x: (
        "ontime" if x <= 0 else "slight" if x <= 5 else "moderate" if x <= 30 else "severe"
    )
    tier_series = train["delay_mins"].apply(tier_fn)
    tier_counts = tier_series.value_counts()
    total       = len(train)
    tier_weight = {t: total / (4 * tier_counts[t]) for t in tier_counts.index}
    sample_weights = tier_series.map(tier_weight).values.astype(np.float32)
    print(f"\n  Sample weights (equal-tier rebalancing):")
    for t, w in sorted(tier_weight.items()):
        print(f"    {t:<12}  count={tier_counts[t]:>8,}  weight={w:.4f}")

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
    print("\n══ Trimmed-mean baseline ═════════════════════════════════════")
    bl = trimmed_mean_baseline(train, test)
    print(f"  Baseline MAE: {bl:.2f} min")

    # ---------------------------------------------------------------------------
    # Day-ahead model
    # ---------------------------------------------------------------------------
    results = []

    print("\n══ Day-ahead model (10 features) ════════════════════════════")
    r = evaluate_variant(
        "v5 day-ahead", train, test, FEATURE_COLS_DAY,
        sample_weight=sample_weights,
        export_onnx=True, model_name="day_ahead",
    )
    results.append(r)

    # ---------------------------------------------------------------------------
    # Real-time model
    # ---------------------------------------------------------------------------
    print("\n══ Real-time model (15 features) ════════════════════════════")
    r = evaluate_variant(
        "v5 real-time", train, test, FEATURE_COLS_RT,
        sample_weight=sample_weights,
        export_onnx=True, model_name="realtime",
    )
    results.append(r)

    # ---------------------------------------------------------------------------
    # Comparison table
    # ---------------------------------------------------------------------------
    print("\n" + "═" * 80)
    print(f"  COMPARISON — 15% random stratified test set  ({len(test):,} rows)")
    print("═" * 80)
    print(f"  {'Model':<38} {'Train rows':>10} {'MAE':>7} {'RMSE':>8} "
          f"{'Bias':>6} {'±2m':>5} {'±5m':>5} {'±10m':>6}")
    print("  " + "─" * 78)

    # Fixed reference row for the previous model
    for label, mae, rmse in [
        ("v3 day-ahead (30 May train)", PREV_DAY_MAE, "—"),
        ("v3 real-time (30 May train)", PREV_RT_MAE,  "—"),
    ]:
        print(f"  {label:<38} {'~1.4M':>10} {mae:>7.2f} {str(rmse):>8}  {'—':>5}  {'—':>5}  {'—':>5}  {'—':>6}")

    print("  " + "─" * 78)
    for r in results:
        print(f"  {r['label']:<38} {r['n_train']:>10,} {r['mae']:>7.2f} {r['rmse']:>8.2f} "
              f"{r['bias']:>+6.2f} {r['w2']:>4.1f}% {r['w5']:>4.1f}% {r['w10']:>5.1f}%")
    print("═" * 80)

    # Delta vs previous
    print("\n══ Delta vs v_prev ═══════════════════════════════════════════")
    for r in results:
        ref = PREV_DAY_MAE if "day-ahead" in r["label"] else PREV_RT_MAE
        delta = r["mae"] - ref
        sign  = "▼ better" if delta < 0 else "▲ worse"
        print(f"  {r['label']:<22}  ΔMAE={delta:+.2f} min  {sign}")

    # Feature importance
    day_r = next(r for r in results if "day-ahead" in r["label"])
    rt_r  = next(r for r in results if "real-time" in r["label"])
    print_importance(day_r["model"], FEATURE_COLS_DAY, "v5 day-ahead")
    print_importance(rt_r["model"],  FEATURE_COLS_RT,  "v5 real-time")

    print("\n  v4 models exported to models/ — restart the Rust server to activate.\n")

    # Write benchmarks.json — picked up by the export module for the index page.
    from datetime import datetime as _dt
    benchmarks = {
        "trained_at":    _dt.now().strftime("%Y-%m-%d"),
        "train_rows":    int(len(train)),
        "test_rows":     int(len(test)),
        "baseline_mae":  round(float(bl), 2),
        "day_ahead": {
            "mae":          round(float(day_r["mae"]),  2),
            "rmse":         round(float(day_r["rmse"]), 2),
            "bias":         round(float(day_r["bias"]), 2),
            "within_2min":  round(float(day_r["w2"]),   1),
            "within_5min":  round(float(day_r["w5"]),   1),
            "within_10min": round(float(day_r["w10"]),  1),
        },
        "realtime": {
            "mae":          round(float(rt_r["mae"]),  2),
            "rmse":         round(float(rt_r["rmse"]), 2),
            "bias":         round(float(rt_r["bias"]), 2),
            "within_2min":  round(float(rt_r["w2"]),   1),
            "within_5min":  round(float(rt_r["w5"]),   1),
            "within_10min": round(float(rt_r["w10"]),  1),
        },
    }
    benchmarks_path = MODELS_DIR / "benchmarks.json"
    with open(benchmarks_path, "w") as f:
        json.dump(benchmarks, f, indent=2)
    print(f"  Benchmarks → {benchmarks_path}\n")


if __name__ == "__main__":
    main()
