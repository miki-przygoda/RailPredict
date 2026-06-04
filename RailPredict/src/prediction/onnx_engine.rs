//! ONNX-backed ML prediction engine for day-ahead and real-time delay forecasting.
//!
//! Wraps two ONNX Runtime sessions loaded from `models/`:
//!   - `day_ahead.onnx`  — pattern + 7-day rolling history (12 features)
//!   - `realtime.onnx`   — above + live Darwin signals (20 features)
//!
//! Both models are optional: if a file is absent the corresponding method returns `None`
//! and `PredictionEngine` falls back to the trimmed-mean statistical engine.
//!
//! Feature vector layout is **index-ordered** and must exactly match the Python training
//! script (`scripts/compare_models.py`). Any change to column order in either place
//! requires a corresponding change in the other.
//!
//! ## Day-ahead (14 features)
//! [0] weekday 0–6   [1] departure_hour 0–23   [2] month 1–12   [3] is_peak 0/1
//! [4] origin_crs_enc   [5] uid_prefix_enc
//! [6] rolling_mean_7d  [7] rolling_std_7d  [8] rolling_ontime_7d  [9] sample_count_log
//! [10] rolling_mean_14d  [11] rolling_std_14d
//! [12] weekday_operator_enc  [13] operator_relative_delay
//!
//! ## Real-time (22 features = day-ahead + 8)
//! [14] current_delay_mins  [15] preceding_delay_mins  [16] wind_mph
//! [17] volatility_score    [18] mins_until_departure  [19] station_congestion_30m
//! [20] operator_cascade_delay  [21] predecessor_train_delay

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Utc};
use ndarray::Array2;
use ort::inputs;
use ort::session::Session;
use ort::value::TensorRef;
use serde_json::Value as JsonValue;

use super::types::{LiveFeatures, RollingStats, ServicePattern};

const N_DAY_FEATURES: usize = 14;
const N_RT_FEATURES:  usize = 22;

const DAY_FEATURE_NAMES: [&str; N_DAY_FEATURES] = [
    "weekday", "departure_hour", "month", "is_peak",
    "origin_crs_enc", "uid_prefix_enc",
    "rolling_mean_7d", "rolling_std_7d", "rolling_ontime_7d", "sample_count_log",
    "rolling_mean_14d", "rolling_std_14d",
    "weekday_operator_enc", "operator_relative_delay",
];
const RT_EXTRA_NAMES: [&str; 8] = [
    "current_delay_mins", "preceding_delay_mins", "wind_mph", "volatility_score",
    "mins_until_departure", "station_congestion_30m", "operator_cascade_delay",
    "predecessor_train_delay",
];

// ---------------------------------------------------------------------------
// OnnxEngine
// ---------------------------------------------------------------------------

/// Holds the two optional ONNX sessions and the categorical encoding maps.
///
/// Sessions are wrapped in `Mutex` because `Session::run` requires `&mut self`
/// while `OnnxEngine` is shared across threads via `Arc`. The mutex is always
/// uncontended in normal operation (one inference per Darwin message, not concurrent).
///
/// Construct via `OnnxEngine::load(models_dir)` or `OnnxEngine::default()` for
/// a no-op fallback (all predictions return `None`).
#[derive(Default)]
pub struct OnnxEngine {
    day_ahead: Option<Mutex<Session>>,
    realtime:  Option<Mutex<Session>>,
    /// CRS code → integer label (1-indexed; 0 reserved for out-of-vocabulary).
    crs_map: HashMap<String, i32>,
    /// First character of UID → integer label (encodes train operating company).
    uid_prefix_map: HashMap<String, i32>,
    /// "weekday_prefix" (e.g. "0_G") → integer label for the combined interaction.
    weekday_operator_map: HashMap<String, i32>,
    /// UID prefix → mean rolling_mean_7d across all services of that operator in training.
    operator_mean_delay: HashMap<String, f32>,
}


impl OnnxEngine {
    /// Load both models and the feature encoding metadata from `models_dir`.
    ///
    /// Missing model files are silently ignored (returns `Ok` with `None` sessions).
    /// A missing `feature_meta.json` is a soft warning — predictions will return `None`
    /// for unknown encodings, triggering the statistical fallback.
    pub fn load(models_dir: &Path) -> Result<Self> {
        let meta_path = models_dir.join("feature_meta.json");
        let (crs_map, uid_prefix_map, weekday_operator_map, operator_mean_delay) =
            if meta_path.exists() {
                load_meta(&meta_path)?
            } else {
                tracing::warn!(
                    path = %meta_path.display(),
                    "feature_meta.json not found — run `make train` to enable ML predictions"
                );
                (HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new())
            };

        let day_ahead = load_session(models_dir, "day_ahead.onnx")?;
        let realtime  = load_session(models_dir, "realtime.onnx")?;

        match (day_ahead.is_some(), realtime.is_some()) {
            (true, true)  => tracing::info!("ONNX day-ahead and real-time models loaded"),
            (true, false) => tracing::info!("ONNX day-ahead model loaded (realtime.onnx not found)"),
            (false, true) => tracing::info!("ONNX real-time model loaded (day_ahead.onnx not found)"),
            (false, false) => tracing::info!(
                "No ONNX models in {:?} — run `make train` to enable ML predictions",
                models_dir
            ),
        }

        Ok(Self { day_ahead, realtime, crs_map, uid_prefix_map, weekday_operator_map, operator_mean_delay })
    }

    /// Day-ahead prediction (14 features). Returns `None` when the model isn't loaded
    /// or the CRS code is unknown. Returns `(predicted_minutes, feature_json)`.
    pub fn predict_day_ahead(
        &self,
        pattern: &ServicePattern,
        rolling: &RollingStats,
        scheduled_departure: &DateTime<Utc>,
    ) -> Option<(i32, JsonValue)> {
        let session = self.day_ahead.as_ref()?;
        let feats = self.day_ahead_features(pattern, rolling, scheduled_departure)?;
        let json  = feats_to_json(&feats, &DAY_FEATURE_NAMES);
        // Day-ahead models carry a persistent ~−6 min bias from the disrupted training
        // distribution. Correct at inference until a debiased training run closes the gap.
        run_session(session, feats, N_DAY_FEATURES).map(|v| (v + 6, json))
    }

    /// Real-time prediction (22 features). Returns `None` when the model isn't loaded
    /// or the CRS code is unknown. Returns `(predicted_minutes, feature_json)`.
    pub fn predict_realtime(
        &self,
        pattern: &ServicePattern,
        rolling: &RollingStats,
        scheduled_departure: &DateTime<Utc>,
        live: &LiveFeatures,
    ) -> Option<(i32, JsonValue)> {
        let session = self.realtime.as_ref()?;
        let mut feats = self.day_ahead_features(pattern, rolling, scheduled_departure)?;
        feats.push(live.current_delay_mins);
        feats.push(live.preceding_delay_mins);
        feats.push(live.wind_mph);
        feats.push(live.volatility_score);
        feats.push(live.mins_until_departure);
        feats.push(live.station_congestion_30m);
        feats.push(live.operator_cascade_delay);
        feats.push(live.predecessor_train_delay);
        let all_names: Vec<&str> = DAY_FEATURE_NAMES.iter().chain(RT_EXTRA_NAMES.iter()).copied().collect();
        let json = feats_to_json(&feats, &all_names);
        run_session(session, feats, N_RT_FEATURES).map(|v| (v, json))
    }
    // (sessions are Mutex<Session> so run_session can acquire &mut Session)

    // -----------------------------------------------------------------------
    // Feature construction
    // -----------------------------------------------------------------------

    fn day_ahead_features(
        &self,
        pattern: &ServicePattern,
        rolling: &RollingStats,
        scheduled_departure: &DateTime<Utc>,
    ) -> Option<Vec<f32>> {
        let crs_enc = *self.crs_map.get(&pattern.origin_crs).unwrap_or(&0) as f32;
        let uid_prefix = pattern.uid.chars().next().unwrap_or('_').to_string();
        let pfx_enc    = *self.uid_prefix_map.get(&uid_prefix).unwrap_or(&0) as f32;

        let weekday_idx = pattern.weekday.num_days_from_monday();
        let wd_op_key   = format!("{}_{}", weekday_idx, uid_prefix);
        let wd_op_enc   = *self.weekday_operator_map.get(&wd_op_key).unwrap_or(&0) as f32;

        let op_mean              = *self.operator_mean_delay.get(&uid_prefix).unwrap_or(&0.0);
        let operator_rel_delay   = rolling.mean_delay - op_mean;

        let weekday = weekday_idx as f32;
        let hour    = pattern.departure_hour as f32;
        let month   = scheduled_departure.month() as f32;
        let is_peak = if [7u8, 8, 16, 17, 18].contains(&pattern.departure_hour)
            && weekday_idx < 5
        { 1.0_f32 } else { 0.0_f32 };

        Some(vec![
            weekday,
            hour,
            month,
            is_peak,
            crs_enc,
            pfx_enc,
            rolling.mean_delay,
            rolling.std_delay,
            rolling.on_time_pct,
            rolling.sample_count_log,
            rolling.mean_delay_14d,
            rolling.std_delay_14d,
            wd_op_enc,
            operator_rel_delay,
        ])
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_session(models_dir: &Path, filename: &str) -> Result<Option<Mutex<Session>>> {
    let path = models_dir.join(filename);
    if !path.exists() {
        return Ok(None);
    }
    let session = Session::builder()
        .context("failed to create ONNX session builder")?
        .commit_from_file(&path)
        .with_context(|| format!("failed to load ONNX model from {}", path.display()))?;
    Ok(Some(Mutex::new(session)))
}

/// Parsed `feature_meta.json` maps: three `String → i32` categorical encoders
/// plus the `String → f32` feature-scaling map.
type FeatureMeta = (
    HashMap<String, i32>,
    HashMap<String, i32>,
    HashMap<String, i32>,
    HashMap<String, f32>,
);

fn load_meta(path: &Path) -> Result<FeatureMeta> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&content)
        .context("feature_meta.json is not valid JSON")?;

    let parse_i32_map = |key: &str| -> Result<HashMap<String, i32>> {
        v[key]
            .as_object()
            .with_context(|| format!("feature_meta.json missing '{key}' object"))?
            .iter()
            .map(|(k, val)| {
                let n = val.as_i64().with_context(|| format!("non-integer value for '{k}'"))?;
                Ok((k.clone(), n as i32))
            })
            .collect()
    };

    // operator_mean_delay may be absent in older meta files — fall back to empty map.
    let op_mean_map: HashMap<String, f32> = v["operator_mean_delay"]
        .as_object()
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, val)| val.as_f64().map(|f| (k.clone(), f as f32)))
                .collect()
        })
        .unwrap_or_default();

    let weekday_op_map = if v["weekday_operator"].is_object() {
        parse_i32_map("weekday_operator")?
    } else {
        HashMap::new()
    };

    Ok((parse_i32_map("crs")?, parse_i32_map("uid_prefix")?, weekday_op_map, op_mean_map))
}

fn feats_to_json(feats: &[f32], names: &[&str]) -> JsonValue {
    let obj: serde_json::Map<String, JsonValue> = names
        .iter()
        .zip(feats.iter())
        .map(|(name, val)| (name.to_string(), JsonValue::from(*val as f64)))
        .collect();
    JsonValue::Object(obj)
}

/// Build a [1 × n_features] tensor, run the session, return the rounded clamped result.
fn run_session(session: &Mutex<Session>, feats: Vec<f32>, expected: usize) -> Option<i32> {
    debug_assert_eq!(feats.len(), expected, "feature vector length mismatch");
    let arr = Array2::<f32>::from_shape_vec((1, expected), feats).ok()?;
    let tensor = TensorRef::from_array_view(arr.view()).ok()?;
    let mut s = session.lock().ok()?;
    // Use positional input (index 0) — works for single-input models regardless of name.
    let outputs = s.run(inputs![tensor]).ok()?;
    // Extract first output as flat slice.
    let (_, data) = outputs[0].try_extract_tensor::<f32>().ok()?;
    let raw = *data.first()?;
    // Clamp to the same range used in the export SQL query, round to nearest minute.
    Some(raw.clamp(-120.0, 600.0).round() as i32)
}
