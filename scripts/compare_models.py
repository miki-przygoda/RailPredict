"""
Compare LightGBM model variants and export the best as production ONNX models.

Previous results (from session 2026-05-26, synthetic seed + early Darwin data):
    v1 (synthetic only)          day-ahead MAE ≈ 35 min on 26 May test set
    v2 (synthetic + real day-1)  day-ahead MAE ≈ 21 min on 26 May test set  (−41%)

Current comparison (real Darwin data only — synthetic seed no longer in DB):
    v_early — trained on 22–23 May Darwin data only
    v_full  — trained on all real Darwin data before today (22–23 + 25 May)
              → exported as production models (day_ahead.onnx, realtime.onnx)

Test set: today's Darwin data (27 May 2026, growing while the server runs).

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
from lightgbm import LGBMRegressor
from onnxmltools import convert_lightgbm
from onnxmltools.convert.common.data_types import FloatTensorType
from sklearn.metrics import mean_absolute_error, root_mean_squared_error
from sqlalchemy import create_engine, text

warnings.filterwarnings("ignore", category=UserWarning)

MODELS_DIR = Path(__file__).parent.parent / "models"

# Dates — skipping 21 May (first-connection startup noise, avg delay −87 min)
EARLY_END  = pd.Timestamp("2026-05-24", tz="UTC")   # v_early trains on 22–23 May
FULL_END   = pd.Timestamp("2026-05-28", tz="UTC")   # v_full trains on 22–23 + 25 + 27 May
TEST_DATE  = "2026-05-28"                            # today's data is the test set

FEATURE_COLS_DAY = [
    "weekday", "departure_hour", "month", "is_peak",
    "origin_crs_enc", "uid_prefix_enc",
    "rolling_mean_7d", "rolling_std_7d", "rolling_ontime_7d", "sample_count_log",
]

FEATURE_COLS_RT = FEATURE_COLS_DAY + [
    "current_delay_mins", "preceding_delay_mins",
    "wind_mph", "volatility_score", "mins_until_departure",
]

LGBM_PARAMS = dict(
    n_estimators=500, learning_rate=0.05, num_leaves=63,
    min_child_samples=20, subsample=0.8, colsample_bytree=0.8,
    n_jobs=-1, verbose=-1, random_state=42,
)


# ---------------------------------------------------------------------------
# Data loading
# ---------------------------------------------------------------------------

def load_data(database_url: str) -> pd.DataFrame:
    engine = create_engine(database_url)
    sql = text("""
        SELECT uid, weekday, origin_crs, departure_hour,
               delay_mins, recorded_at
        FROM delay_history
        WHERE delay_mins BETWEEN -60 AND 300
          AND recorded_at >= '2026-05-22'
        ORDER BY uid, weekday, origin_crs, departure_hour, recorded_at
    """)
    with engine.connect() as conn:
        df = pd.read_sql(sql, conn, parse_dates=["recorded_at"])
    if df["recorded_at"].dt.tz is None:
        df["recorded_at"] = df["recorded_at"].dt.tz_localize("UTC")
    print(f"  Loaded {len(df):,} observations")
    return df


# ---------------------------------------------------------------------------
# Rolling features — O(n) per group via cumulative sums
# ---------------------------------------------------------------------------

def add_rolling_features(df: pd.DataFrame) -> pd.DataFrame:
    print("  Computing rolling features …", end="", flush=True)
    n = len(df)
    rolling_mean   = np.zeros(n, dtype=np.float32)
    rolling_std    = np.zeros(n, dtype=np.float32)
    rolling_ontime = np.zeros(n, dtype=np.float32)
    rolling_count  = np.zeros(n, dtype=np.int32)

    seven_days_ns = np.timedelta64(7, "D")

    for _, group in df.groupby(
        ["uid", "weekday", "origin_crs", "departure_hour"], sort=False
    ):
        idx    = group.index.values
        delays = group["delay_mins"].values.astype(np.float64)
        times  = group["recorded_at"].values

        cum_sum    = np.concatenate([[0.0], np.cumsum(delays)])
        cum_sq     = np.concatenate([[0.0], np.cumsum(delays ** 2)])
        cum_ontime = np.concatenate([[0.0], np.cumsum(delays <= 0).astype(float)])
        left_bounds = np.searchsorted(times, times - seven_days_ns, side="left")

        for j in range(len(idx)):
            l = left_bounds[j]
            k = j - l
            if k > 0:
                s  = cum_sum[j]    - cum_sum[l]
                s2 = cum_sq[j]     - cum_sq[l]
                so = cum_ontime[j] - cum_ontime[l]
                m  = s / k
                rolling_mean[idx[j]]   = np.float32(m)
                rolling_std[idx[j]]    = np.float32(np.sqrt(max(0.0, s2 / k - m * m)))
                rolling_ontime[idx[j]] = np.float32((so / k) * 100.0)
            rolling_count[idx[j]] = k

    df["rolling_mean_7d"]   = rolling_mean
    df["rolling_std_7d"]    = rolling_std
    df["rolling_ontime_7d"] = rolling_ontime
    df["sample_count_log"]  = np.log1p(rolling_count).astype(np.float32)
    print(" done")
    return df


# ---------------------------------------------------------------------------
# Feature engineering
# ---------------------------------------------------------------------------

def engineer_features(df: pd.DataFrame) -> tuple[pd.DataFrame, dict]:
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

    rng = np.random.default_rng(42)
    noise = rng.uniform(0.7, 1.3, size=len(df))
    df["current_delay_mins"]   = (df["delay_mins"].values * noise).astype(np.float32)
    df["preceding_delay_mins"] = np.float32(0)
    df["wind_mph"]             = np.float32(0)
    df["volatility_score"]     = np.float32(0)
    df["mins_until_departure"] = np.float32(0)

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
    export_onnx: bool = False,
    model_name: str = "",
) -> dict:
    X_tr = train[feature_cols].astype(np.float32).values
    y_tr = train["delay_mins"].values
    X_te = test[feature_cols].astype(np.float32).values
    y_te = test["delay_mins"].values

    m = LGBMRegressor(**LGBM_PARAMS)
    m.fit(X_tr, y_tr)
    preds = m.predict(X_te)

    mae  = mean_absolute_error(y_te, preds)
    rmse = root_mean_squared_error(y_te, preds)
    w2   = np.mean(np.abs(preds - y_te) <= 2)  * 100
    w5   = np.mean(np.abs(preds - y_te) <= 5)  * 100
    w10  = np.mean(np.abs(preds - y_te) <= 10) * 100
    bias = np.mean(preds - y_te)

    if export_onnx and model_name:
        MODELS_DIR.mkdir(exist_ok=True)
        n_feat = len(feature_cols)
        init_types = [("float_input", FloatTensorType([None, n_feat]))]
        onnx_model = convert_lightgbm(m.booster_, initial_types=init_types, target_opset=12)
        out_path = MODELS_DIR / f"{model_name}.onnx"
        with open(out_path, "wb") as f:
            f.write(onnx_model.SerializeToString())
        print(f"    Exported → {out_path}  ({out_path.stat().st_size / 1024:.0f} KB)")

    return dict(label=label, n_train=len(train), n_test=len(test),
                mae=mae, rmse=rmse, bias=bias, w2=w2, w5=w5, w10=w10, model=m)


# ---------------------------------------------------------------------------
# Feature importance
# ---------------------------------------------------------------------------

def print_importance(model: LGBMRegressor, cols: list[str], label: str) -> None:
    pairs = sorted(zip(cols, model.feature_importances_), key=lambda x: x[1], reverse=True)
    top_score = max(s for _, s in pairs)
    print(f"\n  [{label}] Feature importance (top 10):")
    for name, score in pairs[:10]:
        bar = "█" * int(score / top_score * 24)
        print(f"    {name:<24} {score:6.0f}  {bar}")


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
    print("\n  Data by day:")
    for d, row in by_day.iterrows():
        print(f"    {d}  {row['rows']:>8,} rows  avg={row['avg_delay']:+.1f} min")

    print("\n══ Rolling features ══════════════════════════════════════════")
    df = add_rolling_features(df)

    print("\n══ Feature engineering ═══════════════════════════════════════")
    df, meta = engineer_features(df)

    # Fixed test set: today's real Darwin data
    test = df[df["recorded_at"].dt.date == pd.Timestamp(TEST_DATE).date()].copy()
    print(f"\n  Test set ({TEST_DATE} — today): {len(test):,} rows")
    if len(test) < 50:
        sys.exit(f"Too little test data for {TEST_DATE} ({len(test)} rows — wait for Darwin to collect more)")

    # v_early: 22–23 May only (first clean real Darwin days)
    v_early_train = df[
        (df["recorded_at"] >= pd.Timestamp("2026-05-22", tz="UTC")) &
        (df["recorded_at"] < EARLY_END)
    ].copy()

    # v_full: all real Darwin data before today (22–23 + 25 May)
    v_full_train = df[df["recorded_at"] < FULL_END].copy()

    print(f"  v_early train (22–23 May):              {len(v_early_train):>8,} rows")
    print(f"  v_full  train (22–23 + 25 + 27 May):   {len(v_full_train):>8,} rows")

    # Save feature metadata
    meta_path = MODELS_DIR / "feature_meta.json"
    MODELS_DIR.mkdir(exist_ok=True)
    with open(meta_path, "w") as f:
        json.dump(meta, f)
    print(f"\n  Feature metadata → {meta_path}")
    print(f"  ({len(meta['crs'])} CRS/TIPLOC encodings, {len(meta['uid_prefix'])} UID prefixes)")

    # ---------------------------------------------------------------------------
    # Baselines
    # ---------------------------------------------------------------------------
    print("\n══ Trimmed-mean baselines ════════════════════════════════════")
    bl_early = trimmed_mean_baseline(v_early_train, test)
    bl_full  = trimmed_mean_baseline(v_full_train,  test)
    print(f"  v_early baseline MAE: {bl_early:.2f} min")
    print(f"  v_full  baseline MAE: {bl_full:.2f} min")

    # ---------------------------------------------------------------------------
    # Day-ahead models
    # ---------------------------------------------------------------------------
    results = []

    print("\n══ Day-ahead models ══════════════════════════════════════════")
    print("  Training v_early (22–23 May only) …")
    r = evaluate_variant("v_early day-ahead (22–23 May)",
                         v_early_train, test, FEATURE_COLS_DAY)
    results.append(r)
    print(f"    MAE={r['mae']:.2f}  RMSE={r['rmse']:.2f}  bias={r['bias']:+.2f}")

    print("  Training v_full (22–23 + 25 + 27 May) …")
    r = evaluate_variant("v_full day-ahead (all pre-today)",
                         v_full_train, test, FEATURE_COLS_DAY,
                         export_onnx=True, model_name="day_ahead")
    results.append(r)
    print(f"    MAE={r['mae']:.2f}  RMSE={r['rmse']:.2f}  bias={r['bias']:+.2f}")

    # ---------------------------------------------------------------------------
    # Real-time models
    # ---------------------------------------------------------------------------
    print("\n══ Real-time models ══════════════════════════════════════════")
    print("  Training v_early real-time …")
    r = evaluate_variant("v_early real-time (22–23 May)",
                         v_early_train, test, FEATURE_COLS_RT)
    results.append(r)
    print(f"    MAE={r['mae']:.2f}  RMSE={r['rmse']:.2f}  bias={r['bias']:+.2f}")

    print("  Training v_full real-time (22–23 + 25 + 27 May) …")
    r = evaluate_variant("v_full real-time (all pre-today)",
                         v_full_train, test, FEATURE_COLS_RT,
                         export_onnx=True, model_name="realtime")
    results.append(r)
    print(f"    MAE={r['mae']:.2f}  RMSE={r['rmse']:.2f}  bias={r['bias']:+.2f}")

    # ---------------------------------------------------------------------------
    # Comparison table
    # ---------------------------------------------------------------------------
    print("\n" + "═" * 76)
    print("  COMPARISON — test set: real Darwin", TEST_DATE, f"({len(test):,} rows)")
    print("═" * 76)
    print(f"  {'Model':<40} {'MAE':>6} {'RMSE':>7} {'Bias':>6} {'±2m':>5} {'±5m':>5} {'±10m':>6}")
    print("  " + "─" * 74)
    print(f"  {'v_early baseline (trimmed-mean)':<40} {bl_early:>6.2f}   {'—':>6}  {'—':>5}  {'—':>5}  {'—':>5}  {'—':>6}")
    print(f"  {'v_full  baseline (trimmed-mean)':<40} {bl_full:>6.2f}   {'—':>6}  {'—':>5}  {'—':>5}  {'—':>5}  {'—':>6}")
    for r in results:
        print(f"  {r['label']:<40} {r['mae']:>6.2f} {r['rmse']:>7.2f} {r['bias']:>+6.2f} "
              f"{r['w2']:>4.1f}% {r['w5']:>4.1f}% {r['w10']:>5.1f}%")
    print("═" * 76)

    # ---------------------------------------------------------------------------
    # Delta: more training data
    # ---------------------------------------------------------------------------
    print("\n══ Delta: adding 25 May data ═════════════════════════════════")
    for model_type in ("day-ahead", "real-time"):
        early_r = next(r for r in results if "v_early" in r["label"] and model_type in r["label"])
        full_r  = next(r for r in results if "v_full"  in r["label"] and model_type in r["label"])
        delta_mae  = full_r["mae"]  - early_r["mae"]
        delta_rmse = full_r["rmse"] - early_r["rmse"]
        delta_w5   = full_r["w5"]   - early_r["w5"]
        verdict = "better" if delta_mae < 0 else "worse"
        print(f"  {model_type:<12}  ΔMAE={delta_mae:+.2f}  ΔRMSE={delta_rmse:+.2f}"
              f"  Δ±5min={delta_w5:+.1f}%  → {verdict}")

    # ---------------------------------------------------------------------------
    # Historical reference (previous sessions)
    # ---------------------------------------------------------------------------
    print("\n══ Historical reference (synthetic-seed era, 26 May test set) ═")
    print("  v1 day-ahead (synthetic only)          MAE ≈ 35.0 min")
    print("  v2 day-ahead (synthetic + real day-1)  MAE ≈ 21.0 min  (−41%)")
    print("  Real Darwin data replaced synthetic seed — current models use real data only.")

    # Feature importance for exported models
    day_full = next(r for r in results if "v_full day-ahead" in r["label"])
    rt_full  = next(r for r in results if "v_full real-time" in r["label"])
    print_importance(day_full["model"], FEATURE_COLS_DAY, "v_full day-ahead")
    print_importance(rt_full["model"],  FEATURE_COLS_RT,  "v_full real-time")

    print("\n  v_full models exported to models/  — restart the Rust server to activate.")


if __name__ == "__main__":
    main()
