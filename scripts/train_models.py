"""
Train two LightGBM models and export them as ONNX for RailPredict.

Day-ahead  (10 features) — pattern + rolling history only
Real-time  (15 features) — same + current Darwin delay, preceding service, weather

Usage:
    python train_models.py [--days N]

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
from lightgbm import LGBMRegressor
from onnxmltools import convert_lightgbm
from onnxmltools.convert.common.data_types import FloatTensorType
from sklearn.metrics import mean_absolute_error, root_mean_squared_error
from sqlalchemy import create_engine, text

warnings.filterwarnings("ignore", category=UserWarning)

MODELS_DIR = Path(__file__).parent.parent / "models"

# Feature column order MUST match the Rust inference code exactly.
FEATURE_COLS_DAY = [
    "weekday",
    "departure_hour",
    "month",
    "is_peak",
    "origin_crs_enc",
    "uid_prefix_enc",
    "rolling_mean_7d",
    "rolling_std_7d",
    "rolling_ontime_7d",
    "sample_count_log",
]

FEATURE_COLS_RT = FEATURE_COLS_DAY + [
    "current_delay_mins",
    "preceding_delay_mins",
    "wind_mph",
    "volatility_score",
    "mins_until_departure",
]

LGBM_PARAMS = dict(
    n_estimators=500,
    learning_rate=0.05,
    num_leaves=63,
    min_child_samples=20,
    subsample=0.8,
    colsample_bytree=0.8,
    n_jobs=-1,
    verbose=-1,
    random_state=42,
)


# ---------------------------------------------------------------------------
# Data extraction
# ---------------------------------------------------------------------------

def load_data(database_url: str) -> pd.DataFrame:
    engine = create_engine(database_url)
    sql = text("""
        SELECT uid, weekday, origin_crs, departure_hour,
               delay_mins, recorded_at
        FROM delay_history
        WHERE delay_mins BETWEEN -120 AND 600
        ORDER BY recorded_at
    """)
    with engine.connect() as conn:
        df = pd.read_sql(sql, conn, parse_dates=["recorded_at"])
    print(f"  Loaded {len(df):,} observations")
    return df


# ---------------------------------------------------------------------------
# Feature engineering
# ---------------------------------------------------------------------------

def engineer_features(df: pd.DataFrame) -> tuple[pd.DataFrame, dict]:
    # Pre-sort so each pattern's observations are in time order.
    df = df.sort_values(
        ["uid", "weekday", "origin_crs", "departure_hour", "recorded_at"]
    ).reset_index(drop=True)

    # Rolling features (no-leak: strictly prior observations per pattern).
    # Use explicit index-based assignment to avoid groupby.apply column-drop issues.
    print("  Computing rolling features …")
    n = len(df)
    rolling_mean  = np.zeros(n, dtype=np.float32)
    rolling_std   = np.zeros(n, dtype=np.float32)
    rolling_ontime = np.zeros(n, dtype=np.float32)
    rolling_count = np.zeros(n, dtype=np.int32)

    seven_days_ns = np.timedelta64(7, "D")
    for _, group in df.groupby(
        ["uid", "weekday", "origin_crs", "departure_hour"], sort=False
    ):
        idx    = group.index.values
        delays = group["delay_mins"].values.astype(float)
        times  = group["recorded_at"].values

        for j in range(len(idx)):
            orig = idx[j]
            t    = times[j]
            mask = times[:j] >= (t - seven_days_ns)
            w    = delays[:j][mask]
            k    = len(w)
            if k > 0:
                rolling_mean[orig]   = float(np.mean(w))
                rolling_std[orig]    = float(np.std(w))
                rolling_ontime[orig] = float(np.mean(w <= 0) * 100)
            rolling_count[orig] = k

    df["rolling_mean_7d"]   = rolling_mean
    df["rolling_std_7d"]    = rolling_std
    df["rolling_ontime_7d"] = rolling_ontime
    df["sample_count"]      = rolling_count

    df["sample_count_log"] = np.log1p(df["sample_count"]).astype(np.float32)

    # Temporal features.
    df["month"]    = df["recorded_at"].dt.month.astype(np.int32)
    df["is_peak"]  = (
        df["departure_hour"].isin([7, 8, 16, 17, 18]) & (df["weekday"] <= 4)
    ).astype(np.float32)

    # Categorical encoding.
    unique_crs = sorted(df["origin_crs"].unique())
    unique_pfx = sorted(df["uid"].str[0].unique())

    crs_map = {c: i + 1 for i, c in enumerate(unique_crs)}   # 0 reserved for OOV
    pfx_map = {p: i + 1 for i, p in enumerate(unique_pfx)}

    df["origin_crs_enc"]  = df["origin_crs"].map(crs_map).fillna(0).astype(np.int32)
    df["uid_prefix_enc"]  = df["uid"].str[0].map(pfx_map).fillna(0).astype(np.int32)

    meta = {"crs": crs_map, "uid_prefix": pfx_map}

    # Real-time synthetic features (training simulation).
    # current_delay_mins: perturbed actual delay — simulates noisy Darwin reading.
    rng = np.random.default_rng(42)
    noise = rng.uniform(0.7, 1.3, size=len(df))
    df["current_delay_mins"]   = (df["delay_mins"].values * noise).astype(np.float32)
    # preceding / weather / volatility unknown at training time → 0 (neutral).
    df["preceding_delay_mins"] = np.float32(0)
    df["wind_mph"]             = np.float32(0)
    df["volatility_score"]     = np.float32(0)
    df["mins_until_departure"] = np.float32(0)

    return df, meta


# ---------------------------------------------------------------------------
# Split
# ---------------------------------------------------------------------------

def temporal_split(df: pd.DataFrame):
    cutoff = df["recorded_at"].max() - pd.Timedelta(days=1)
    train = df[df["recorded_at"] < cutoff]
    test  = df[df["recorded_at"] >= cutoff]
    print(f"  Train: {len(train):,} rows  /  Test: {len(test):,} rows")
    return train, test


# ---------------------------------------------------------------------------
# Baseline (trimmed mean per pattern)
# ---------------------------------------------------------------------------

def trimmed_mean_baseline(train: pd.DataFrame, test: pd.DataFrame) -> float:
    def _tmean(s):
        n = len(s)
        trim = max(1, int(n * 0.1))
        s_sorted = sorted(s)
        trimmed = s_sorted[trim: n - trim] if n - 2 * trim > 0 else s_sorted
        return float(np.mean(trimmed)) if trimmed else float(np.mean(s))

    agg = (
        train.groupby(["uid", "weekday", "origin_crs", "departure_hour"])["delay_mins"]
        .apply(_tmean)
        .rename("pred_baseline")
        .reset_index()
    )
    merged = test.merge(agg, on=["uid", "weekday", "origin_crs", "departure_hour"], how="left")
    merged["pred_baseline"] = merged["pred_baseline"].fillna(train["delay_mins"].mean())
    return mean_absolute_error(merged["delay_mins"], merged["pred_baseline"])


# ---------------------------------------------------------------------------
# Training and ONNX export
# ---------------------------------------------------------------------------

def train_and_export(
    train: pd.DataFrame,
    test: pd.DataFrame,
    feature_cols: list[str],
    target_col: str,
    model_name: str,
) -> LGBMRegressor:
    X_train = train[feature_cols].astype(np.float32).values
    y_train = train[target_col].values
    X_test  = test[feature_cols].astype(np.float32).values
    y_test  = test[target_col].values

    model = LGBMRegressor(**LGBM_PARAMS)
    model.fit(X_train, y_train)

    preds = model.predict(X_test)
    mae   = mean_absolute_error(y_test, preds)
    rmse  = root_mean_squared_error(y_test, preds)
    print(f"  [{model_name}]  MAE={mae:.2f} min  RMSE={rmse:.2f} min")

    # ONNX export.
    n_features = len(feature_cols)
    initial_type = [("float_input", FloatTensorType([None, n_features]))]
    onnx_model = convert_lightgbm(model.booster_, initial_types=initial_type, target_opset=12)

    # Print output names so the Rust side can be verified.
    output_names = [o.name for o in onnx_model.graph.output]
    print(f"  [{model_name}]  ONNX outputs: {output_names}")

    out_path = MODELS_DIR / f"{model_name}.onnx"
    with open(out_path, "wb") as f:
        f.write(onnx_model.SerializeToString())
    print(f"  [{model_name}]  → {out_path}  ({out_path.stat().st_size / 1024:.0f} KB)")

    return model


# ---------------------------------------------------------------------------
# Feature importance report
# ---------------------------------------------------------------------------

def print_importance(model: LGBMRegressor, feature_cols: list[str], label: str) -> None:
    importance = sorted(
        zip(feature_cols, model.feature_importances_),
        key=lambda x: x[1],
        reverse=True,
    )
    print(f"\n  [{label}] Feature importance:")
    for name, score in importance[:10]:
        bar = "█" * int(score / max(s for _, s in importance) * 20)
        print(f"    {name:<22} {score:6.0f}  {bar}")


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(description="Train RailPredict delay models")
    parser.add_argument("--days", type=int, default=0,
                        help="Lookback days (0 = all history)")
    args = parser.parse_args()

    # Load DATABASE_URL from .env (or override with env var).
    load_dotenv(Path(__file__).parent.parent / ".env")
    raw_url = os.environ.get("DATABASE_URL", "")
    if not raw_url:
        sys.exit("DATABASE_URL not set — add it to .env or set the env var")

    # Swap docker hostname for localhost when running on the host machine.
    database_url = raw_url.replace("@db:", "@localhost:").replace(
        "postgres://", "postgresql+psycopg2://", 1
    )

    MODELS_DIR.mkdir(exist_ok=True)

    print("\n── Loading data ─────────────────────────────────────────────")
    df = load_data(database_url)
    if len(df) < 100:
        sys.exit("Not enough data to train (need ≥ 100 observations)")

    print("\n── Engineering features ─────────────────────────────────────")
    df, meta = engineer_features(df)

    print("\n── Saving feature metadata ──────────────────────────────────")
    meta_path = MODELS_DIR / "feature_meta.json"
    with open(meta_path, "w") as f:
        json.dump(meta, f)
    print(f"  {len(meta['crs'])} CRS codes, {len(meta['uid_prefix'])} UID prefixes → {meta_path}")

    print("\n── Splitting data ───────────────────────────────────────────")
    train, test = temporal_split(df)
    if len(test) == 0:
        sys.exit("Test set is empty — need at least 2 days of data")

    print("\n── Baseline (trimmed mean) ───────────────────────────────────")
    baseline_mae = trimmed_mean_baseline(train, test)
    print(f"  Baseline MAE: {baseline_mae:.2f} min")

    print("\n── Training day-ahead model ─────────────────────────────────")
    day_model = train_and_export(train, test, FEATURE_COLS_DAY, "delay_mins", "day_ahead")

    print("\n── Training real-time model ─────────────────────────────────")
    rt_model = train_and_export(train, test, FEATURE_COLS_RT, "delay_mins", "realtime")

    print_importance(day_model, FEATURE_COLS_DAY, "day-ahead")
    print_importance(rt_model,  FEATURE_COLS_RT,  "real-time")

    print("\n── Done ─────────────────────────────────────────────────────")
    print("  Run  make train  again as more data accumulates to retrain.")
    print("  Restart the Rust server to pick up new models.\n")


if __name__ == "__main__":
    main()
