//! Tier B prediction engine.
//!
//! ## Algorithm
//! Trimmed mean over the last MAX_SAMPLES delay records for the matching ServicePattern.
//! The bottom and top 10% of sorted values are dropped before averaging — this resists
//! extreme outliers (e.g. a one-off 4-hour delay due to infrastructure failure) corrupting
//! the baseline prediction for an otherwise reliable service.
//!
//! Confidence = min(1.0, sample_count / MAX_SAMPLES). Starts at ~0.01 for a brand-new
//! pattern and approaches 1.0 after ~13 weeks of daily observations. Callers can use this
//! to decide whether to trust the prediction or present it with a caveat.
//!
//! ## No fabrication rule
//! Returns None for both predicted_delay_mins and historical_reliability until at least 3
//! samples exist. A single delayed Monday does not make a reliable predictor.
//!
//! ## AdvancedAnalytics phases implemented here
//!
//! ### Phase 1 — Preceding service correlation
//! `predict_and_update_with_correlation` accepts an optional slice of `TrainStatus`
//! snapshots from the registry. It scans for a service departing the same origin within
//! `[departure - 20 mins, departure - 1 min]`. If found with `reported_delay_mins > 5`,
//! the trimmed mean is blended:
//! `adjusted = (mean * HISTORY_WEIGHT) + (preceding_delay * PRECEDING_WEIGHT)`.
//! The signal is stored in `status.volatility.correlation_signal` for UI auditability.
//!
//! The original `predict_and_update` (no snapshot argument) is kept for backward
//! compatibility with `ingestion/mod.rs`. It calls `predict_and_update_with_correlation`
//! with `None`, disabling correlation. Once Improvements.md 2.1–2.2 are done, the
//! ingestion / poll consumer should switch to calling `predict_and_update_with_correlation`.
//!
//! ### Phase 4 — Confidence decay for stale history
//! After computing raw confidence, the most recent `recorded_at` is checked. If it is
//! older than `STALENESS_THRESHOLD_DAYS`, confidence is multiplied by
//! `exp(-days_since / STALENESS_THRESHOLD_DAYS)`, causing it to approach zero as history
//! ages. The decayed value is stored in `status.volatility.historical_reliability`.
//!
//! ### Phase 3 — Hour-of-day buckets
//! `derive_pattern` now includes `departure_hour` in the `ServicePattern` key, so
//! rush-hour and off-peak departures have independent history rings.
//!
//! ### Observability — prediction accuracy metric
//! After `predict_and_update`, if both `predicted_delay_mins` and `reported_delay_mins`
//! are known, the absolute error is recorded via `metrics::histogram!`. Requires the
//! `metrics` crate (added by the Observability agent in Cargo.toml).

use std::sync::Arc;

use chrono::{Datelike, Timelike, Utc};

use crate::types::train_status::Stamped;
use crate::types::volatility::CorrelationSignal;
use crate::types::TrainStatus;

use super::onnx_engine::OnnxEngine;
use super::types::{DelayRecord, HistoricalStore, LiveFeatures, ServicePattern, MAX_SAMPLES};

// ---------------------------------------------------------------------------
// Blend weights for Phase 1 (preceding service correlation)
// ---------------------------------------------------------------------------

/// Weight applied to the historical trimmed mean in the blended prediction.
/// Complement of `PRECEDING_WEIGHT`. Together they must sum to 1.0.
const HISTORY_WEIGHT: f64 = 0.6;

/// Weight applied to the preceding service's reported delay in the blended prediction.
const PRECEDING_WEIGHT: f64 = 0.4;

/// Minimum reported delay (minutes) for a preceding service to be treated as a
/// correlation signal. Services with ≤ this delay are considered on-time noise.
const PRECEDING_SIGNAL_THRESHOLD_MINS: i32 = 5;

/// Time window (minutes) before the target train's departure within which we look
/// for a preceding service at the same origin. The upper bound is 20 mins prior;
/// the lower bound is 1 min prior (to exclude the train itself).
const PRECEDING_WINDOW_MAX_MINS: i64 = 20;
const PRECEDING_WINDOW_MIN_MINS: i64 = 1;

// ---------------------------------------------------------------------------
// Phase 4: staleness decay
// ---------------------------------------------------------------------------

/// Number of days after which history is considered "stale". At this age the raw
/// confidence is multiplied by `e^(-1) ≈ 0.37`. At 2× the threshold it is `e^(-2) ≈ 0.14`.
const STALENESS_THRESHOLD_DAYS: f64 = 21.0;

// ---------------------------------------------------------------------------
// record_outcome throttle
// ---------------------------------------------------------------------------

/// Minimum change in delay (minutes) that triggers a new history record.
/// Updates smaller than this are skipped unless MIN_RECORD_INTERVAL_SECS has elapsed.
const MIN_DELAY_CHANGE_MINS: i32 = 2;

/// Minimum seconds between consecutive records for the same service pattern.
/// Guarantees a periodic snapshot even when the delay is stable.
const MIN_RECORD_INTERVAL_SECS: i64 = 300; // 5 minutes

// ---------------------------------------------------------------------------
// PredictionEngine
// ---------------------------------------------------------------------------

/// Wraps the shared `HistoricalStore` and the optional ONNX ML engine.
///
/// Cheap to clone — each clone shares the same `Arc`-backed store and engine.
/// Prediction priority: real-time ONNX → day-ahead ONNX → trimmed-mean statistical.
#[derive(Clone)]
pub struct PredictionEngine {
    store: Arc<HistoricalStore>,
    onnx:  Arc<OnnxEngine>,
}

impl Default for PredictionEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PredictionEngine {
    pub fn new() -> Self {
        Self {
            store: Arc::new(HistoricalStore::new()),
            onnx:  Arc::new(OnnxEngine::default()),
        }
    }

    /// Construct with an existing store (used when pre-loading from the DB on startup).
    /// Uses the default no-op ONNX engine — call `with_store_and_onnx` to enable ML.
    pub fn with_store(store: Arc<HistoricalStore>) -> Self {
        Self { store, onnx: Arc::new(OnnxEngine::default()) }
    }

    /// Construct with pre-loaded history AND an ONNX engine.
    /// This is the production path: `OnnxEngine::load("models")` at startup.
    pub fn with_store_and_onnx(store: Arc<HistoricalStore>, onnx: Arc<OnnxEngine>) -> Self {
        Self { store, onnx }
    }

    /// Returns a cheap `Arc` clone of the store — used by the background DB flush task.
    pub fn arc_store(&self) -> Arc<HistoricalStore> {
        Arc::clone(&self.store)
    }

    /// Compute a prediction from historical data and write it into `status.predicted_delay_mins`
    /// and `status.volatility.historical_reliability`.
    ///
    /// This is the original single-argument API retained for backward compatibility with
    /// `ingestion/mod.rs`. It calls `predict_and_update_with_correlation(status, None)`.
    ///
    /// For Phase 1 (preceding service correlation), use `predict_and_update_with_correlation`
    /// and pass a registry snapshot slice.
    pub fn predict_and_update(&self, status: &mut TrainStatus) {
        self.predict_and_update_with_correlation(status, None);
    }

    /// Full prediction update with optional preceding-service correlation (Phase 1).
    ///
    /// Does nothing if the pattern cannot be derived (uid or origin_crs missing) or if fewer
    /// than 3 samples exist for this pattern.
    ///
    /// ## Phase 1: preceding service correlation
    /// Pass `registry_snapshot` as a slice of all current `TrainStatus` values (read-only
    /// snapshot) to enable the correlation scan. If `None`, no correlation is attempted.
    ///
    /// ## Phase 4: confidence decay
    /// Confidence is automatically decayed based on the age of the most recent observation.
    ///
    /// ## Observability metric (requires `metrics` crate — added by Observability agent)
    /// If both `predicted_delay_mins` and `reported_delay_mins` are known after update,
    /// records `prediction_error_mins` histogram entry.
    ///
    /// TODO (caller wiring for Phase 1): the ingestion pipeline / poll consumer should call
    /// this variant (not `predict_and_update`) once it can supply a registry snapshot.
    /// Until Improvements.md 2.1–2.2 are done, passing `None` is safe — no correlation occurs.
    pub fn predict_and_update_with_correlation(
        &self,
        status: &mut TrainStatus,
        registry_snapshot: Option<&[TrainStatus]>,
    ) {
        let Some(pattern) = derive_pattern(status) else { return };

        // -----------------------------------------------------------------------
        // Rolling stats (shared by both ONNX models and the statistical baseline)
        // -----------------------------------------------------------------------
        let rolling = self.store.rolling_stats_7d(&pattern);

        // -----------------------------------------------------------------------
        // ML path: try real-time ONNX if a live Darwin reading is available,
        //          then day-ahead ONNX, then fall back to the statistical engine.
        // -----------------------------------------------------------------------
        let dep = &status.scheduled_departure.value;

        let ml_prediction: Option<i32> = if let Some(reported) = status.reported_delay_mins.value {
            // Real-time: we have a live Darwin delay signal — use the 15-feature model.
            let preceding = status
                .volatility
                .correlation_signal
                .as_ref()
                .map_or(0.0, |s| s.preceding_delay_mins as f32);
            let wind = status.volatility.wind_speed_mph.unwrap_or(0.0);
            let volatility_score = match (status.volatility.is_wind_critical(), status.volatility.incident_flagged) {
                (true,  true)  => 3.0_f32,
                (false, true)  => 2.0,
                (true,  false) => 1.0,
                (false, false) => 0.0,
            };
            let mins_until = (dep.timestamp() - Utc::now().timestamp()) as f32 / 60.0;
            let live = LiveFeatures {
                current_delay_mins:   reported as f32,
                preceding_delay_mins: preceding,
                wind_mph:             wind,
                volatility_score,
                mins_until_departure: mins_until,
            };
            self.onnx.predict_realtime(&pattern, &rolling, dep, &live)
                .or_else(|| self.onnx.predict_day_ahead(&pattern, &rolling, dep))
        } else {
            // Day-ahead: no live reading yet.
            self.onnx.predict_day_ahead(&pattern, &rolling, dep)
        };

        if let Some(pred) = ml_prediction {
            status.predicted_delay_mins = Stamped::new(Some(pred));
            // Confidence for ML path: use rolling sample coverage as proxy.
            status.volatility.historical_reliability = Some(
                (rolling.sample_count_log / 4.0_f32).min(1.0),
            );
            // Still run correlation scan to populate the signal for UI auditability.
            let (_, correlation_signal) = if let Some(snapshot) = registry_snapshot {
                apply_preceding_correlation(pred, status, snapshot)
            } else {
                (pred, None)
            };
            status.volatility.correlation_signal = correlation_signal;

            if let (Some(predicted), Some(reported)) = (
                status.predicted_delay_mins.value,
                status.reported_delay_mins.value,
            ) {
                metrics::histogram!("prediction_error_mins")
                    .record((predicted - reported).unsigned_abs() as f64);
            }
            return;
        }

        // -----------------------------------------------------------------------
        // Statistical fallback: trimmed mean (unchanged from original engine)
        // -----------------------------------------------------------------------
        let Some(samples) = self.store.get_samples(&pattern) else { return };
        if samples.len() < 3 {
            return;
        }

        let (mean, raw_confidence) = trimmed_mean(&samples, MAX_SAMPLES);

        // -----------------------------------------------------------------------
        // Phase 1: preceding service correlation
        // -----------------------------------------------------------------------
        let (blended_mean, correlation_signal) = if let Some(snapshot) = registry_snapshot {
            apply_preceding_correlation(mean, status, snapshot)
        } else {
            (mean, None)
        };

        status.volatility.correlation_signal = correlation_signal;

        // -----------------------------------------------------------------------
        // Phase 4: confidence decay for stale history
        // -----------------------------------------------------------------------
        let decayed_confidence = if let Some(most_recent) = self.store.most_recent_recorded_at(&pattern) {
            let days_since = (Utc::now() - most_recent).num_seconds() as f64 / 86_400.0;
            if days_since > STALENESS_THRESHOLD_DAYS {
                let decay = (-days_since / STALENESS_THRESHOLD_DAYS).exp() as f32;
                tracing::trace!(
                    uid = %pattern.uid,
                    days_since = days_since,
                    raw_confidence = raw_confidence,
                    decayed_confidence = raw_confidence * decay,
                    "Applying staleness decay to confidence"
                );
                raw_confidence * decay
            } else {
                raw_confidence
            }
        } else {
            raw_confidence
        };

        status.predicted_delay_mins = Stamped::new(Some(blended_mean));
        status.volatility.historical_reliability = Some(decayed_confidence);

        // -----------------------------------------------------------------------
        // Observability: prediction accuracy metric
        // Requires `metrics` crate in Cargo.toml (added by Observability agent).
        // -----------------------------------------------------------------------
        if let (Some(predicted), Some(reported)) = (
            status.predicted_delay_mins.value,
            status.reported_delay_mins.value,
        ) {
            let error_mins = (predicted - reported).abs() as f64;
            metrics::histogram!("prediction_error_mins").record(error_mins);
        }
    }

    /// Feed a confirmed delay outcome from a Darwin TS message back into the historical store.
    ///
    /// Called after `status.reported_delay_mins` has been set by the ingestion pipeline.
    /// Does nothing if the pattern or delay value cannot be derived.
    ///
    /// ## Write throttle
    /// Only records if the delay has changed by ≥ MIN_DELAY_CHANGE_MINS since the last
    /// stored observation OR at least MIN_RECORD_INTERVAL_SECS have elapsed. This prevents
    /// Darwin's high-frequency TS updates (dozens per minute per active train) from flooding
    /// the history store with identical readings while still capturing meaningful changes.
    pub fn record_outcome(&self, status: &TrainStatus) {
        let Some(pattern) = derive_pattern(status) else { return };
        let Some(delay_mins) = status.reported_delay_mins.value else { return };
        let now = Utc::now();
        if let Some((last_delay, last_at)) = self.store.last_record(&pattern) {
            let change = (delay_mins - last_delay).abs();
            let elapsed = (now - last_at).num_seconds();
            if change < MIN_DELAY_CHANGE_MINS && elapsed < MIN_RECORD_INTERVAL_SECS {
                return;
            }
        }
        // Capture the prediction that was active before this observation arrived.
        let predicted_delay_mins = status.predicted_delay_mins.value;
        self.store.insert(pattern, DelayRecord { delay_mins, predicted_delay_mins, recorded_at: now });
    }
}

// ---------------------------------------------------------------------------
// Phase 1 helpers
// ---------------------------------------------------------------------------

/// Scan `registry_snapshot` for a preceding service at the same origin and, if found
/// with a significant delay, return a blended mean and the correlation signal.
///
/// Returns `(original_mean, None)` if no qualifying preceding service is found.
fn apply_preceding_correlation(
    mean: i32,
    status: &TrainStatus,
    snapshot: &[TrainStatus],
) -> (i32, Option<CorrelationSignal>) {
    let Some(ref origin_crs) = status.origin_crs else {
        return (mean, None);
    };

    let target_departure = status.scheduled_departure.value;
    let window_start = target_departure - chrono::Duration::minutes(PRECEDING_WINDOW_MAX_MINS);
    let window_end   = target_departure - chrono::Duration::minutes(PRECEDING_WINDOW_MIN_MINS);

    // Find the service with the latest scheduled_departure within the window
    // at the same origin that also has a significant reported delay.
    let mut best: Option<(&TrainStatus, i32)> = None;

    for candidate in snapshot {
        // Skip the train itself.
        if candidate.id == status.id {
            continue;
        }
        // Must share the same origin.
        if candidate.origin_crs.as_deref() != Some(origin_crs.as_str()) {
            continue;
        }
        let dep = candidate.scheduled_departure.value;
        if dep < window_start || dep > window_end {
            continue;
        }
        let Some(delay) = candidate.reported_delay_mins.value else {
            continue;
        };
        if delay <= PRECEDING_SIGNAL_THRESHOLD_MINS {
            continue;
        }
        // Keep the one with the latest departure (closest predecessor).
        if best.is_none_or(|(prev, _)| dep > prev.scheduled_departure.value) {
            best = Some((candidate, delay));
        }
    }

    let Some((preceding, preceding_delay)) = best else {
        return (mean, None);
    };

    let blended = (mean as f64 * HISTORY_WEIGHT + preceding_delay as f64 * PRECEDING_WEIGHT)
        .round() as i32;

    let signal = CorrelationSignal {
        preceding_rid: preceding.id.clone(),
        preceding_delay_mins: preceding_delay,
        weight: PRECEDING_WEIGHT as f32,
    };

    (blended, Some(signal))
}

// ---------------------------------------------------------------------------
// Pattern derivation
// ---------------------------------------------------------------------------

/// Derive the `ServicePattern` key from a `TrainStatus`. Returns `None` if either
/// the uid or origin_crs is not yet known.
///
/// Phase 3: `departure_hour` is now part of the key.
fn derive_pattern(status: &TrainStatus) -> Option<ServicePattern> {
    let uid = status.uid.clone()?;
    let origin_crs = status.origin_crs.clone()?;
    let weekday = status.scheduled_departure.value.weekday();
    let departure_hour = status.scheduled_departure.value.hour() as u8;
    Some(ServicePattern { uid, weekday, origin_crs, departure_hour })
}

// ---------------------------------------------------------------------------
// Trimmed mean
// ---------------------------------------------------------------------------

/// Trimmed mean: sort values, drop the bottom and top 10% (keeping at least 1 element),
/// return the integer mean of the remaining values and a confidence in [0.0, 1.0].
fn trimmed_mean(samples: &[i32], max_samples: usize) -> (i32, f32) {
    let n = samples.len();
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();

    // Clamp trim so we never remove more than floor((n-1)/2) from each end,
    // guaranteeing at least one element remains after trimming.
    let trim = ((n as f32 * 0.1) as usize).min(n.saturating_sub(1) / 2);
    let trimmed = &sorted[trim..n - trim];

    let mean =
        (trimmed.iter().map(|&x| x as i64).sum::<i64>() / trimmed.len() as i64) as i32;
    let confidence = (n as f32 / max_samples as f32).min(1.0);
    (mean, confidence)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    use crate::types::{TrainId, TrainStatus};
    use crate::types::train_status::Stamped;

    fn make_status_with_history(uid: &str, origin_crs: &str) -> TrainStatus {
        let now = Utc::now();
        let mut s = TrainStatus::new(
            TrainId::rid("202404170123456").unwrap(),
            now,
            now,
        );
        s.uid = Some(uid.to_string());
        s.origin_crs = Some(origin_crs.to_string());
        s
    }

    #[test]
    fn predict_with_no_history_returns_none() {
        let engine = PredictionEngine::new();
        let mut status = make_status_with_history("C12345", "LEEDS");
        engine.predict_and_update(&mut status);
        assert!(status.predicted_delay_mins.value.is_none());
        assert!(status.volatility.historical_reliability.is_none());
    }

    // Helper: insert `n` records with timestamps spaced 10 min apart, bypassing the throttle.
    // Tests that need a specific sample count should use this instead of calling record_outcome
    // in a tight loop, since the throttle would suppress duplicate rapid inserts.
    fn seed_store(engine: &PredictionEngine, status: &TrainStatus, delays: &[i32]) {
        use crate::prediction::types::{DelayRecord, ServicePattern};
        use chrono::{Datelike, Duration, Timelike};
        let pattern = ServicePattern {
            uid: status.uid.clone().unwrap_or_default(),
            weekday: status.scheduled_departure.value.weekday(),
            origin_crs: status.origin_crs.clone().unwrap_or_default(),
            departure_hour: status.scheduled_departure.value.hour() as u8,
        };
        let n = delays.len();
        for (i, &d) in delays.iter().enumerate() {
            engine.store.insert(pattern.clone(), DelayRecord {
                delay_mins: d,
                predicted_delay_mins: None,
                // Oldest first; spaced 10 min apart so rolling_stats_7d sees them all.
                recorded_at: Utc::now() - Duration::minutes((n - i) as i64 * 10),
            });
        }
    }

    #[test]
    fn predict_with_fewer_than_three_samples_returns_none() {
        let engine = PredictionEngine::new();
        let mut status = make_status_with_history("C12345", "LEEDS");
        status.reported_delay_mins = Stamped::new(Some(5));
        seed_store(&engine, &status, &[5, 5]);
        engine.predict_and_update(&mut status);
        assert!(status.predicted_delay_mins.value.is_none());
    }

    #[test]
    fn predict_with_uniform_history_returns_that_value() {
        let engine = PredictionEngine::new();
        let mut status = make_status_with_history("C12345", "LEEDS");
        status.reported_delay_mins = Stamped::new(Some(5));
        seed_store(&engine, &status, &[5; 10]);
        engine.predict_and_update(&mut status);
        assert_eq!(status.predicted_delay_mins.value, Some(5));
    }

    #[test]
    fn trimmed_mean_ignores_outliers() {
        let engine = PredictionEngine::new();
        let mut status = make_status_with_history("C12345", "LEEDS");
        // 8 samples at 5 mins, 2 outliers at 120 mins
        let mut delays = vec![5i32; 8];
        delays.extend_from_slice(&[120, 120]);
        seed_store(&engine, &status, &delays);

        status.reported_delay_mins = Stamped::new(Some(5));
        engine.predict_and_update(&mut status);
        // Trimming 10% from each end of 10 samples = 1 from each end.
        // sorted=[5,5,5,5,5,5,5,5,120,120], drop first and last → [5,5,5,5,5,5,5,120] → mean=25.
        let predicted = status.predicted_delay_mins.value.unwrap();
        assert!(predicted < 30, "trimmed mean {predicted} should be < 30 (not dominated by outliers)");
        assert!(predicted >= 5, "trimmed mean {predicted} should be >= 5");
    }

    #[test]
    fn store_caps_at_max_samples() {
        use crate::prediction::types::{DelayRecord, MAX_SAMPLES, ServicePattern};
        use chrono::{Datelike, Duration, Timelike};

        let engine = PredictionEngine::new();
        let status = make_status_with_history("C12345", "LEEDS");
        let pattern = ServicePattern {
            uid: "C12345".to_string(),
            weekday: status.scheduled_departure.value.weekday(),
            origin_crs: "LEEDS".to_string(),
            departure_hour: status.scheduled_departure.value.hour() as u8,
        };
        for i in 0..MAX_SAMPLES + 5 {
            engine.store.insert(pattern.clone(), DelayRecord {
                delay_mins: (i % 10) as i32,
                predicted_delay_mins: None,
                recorded_at: Utc::now() - Duration::minutes((MAX_SAMPLES + 5 - i) as i64),
            });
        }
        assert_eq!(engine.store.sample_count(&pattern), MAX_SAMPLES);
    }

    #[test]
    fn confidence_scales_with_sample_count() {
        let engine = PredictionEngine::new();
        let mut status = make_status_with_history("C12345", "LEEDS");
        status.reported_delay_mins = Stamped::new(Some(4));
        seed_store(&engine, &status, &[4i32; 45]);
        engine.predict_and_update(&mut status);
        let confidence = status.volatility.historical_reliability.unwrap();
        assert!((confidence - 0.5).abs() < 0.01, "expected ~0.5, got {confidence}");
    }

    #[test]
    fn record_outcome_throttle_skips_unchanged_rapid_updates() {
        let engine = PredictionEngine::new();
        let mut status = make_status_with_history("C12345", "LEEDS");
        status.reported_delay_mins = Stamped::new(Some(10));
        // First call always inserts.
        engine.record_outcome(&status);
        // Second call immediately after with same delay should be suppressed.
        engine.record_outcome(&status);
        engine.record_outcome(&status);
        use crate::prediction::types::ServicePattern;
        use chrono::{Datelike, Timelike};
        let pattern = ServicePattern {
            uid: "C12345".to_string(),
            weekday: status.scheduled_departure.value.weekday(),
            origin_crs: "LEEDS".to_string(),
            departure_hour: status.scheduled_departure.value.hour() as u8,
        };
        assert_eq!(engine.store.sample_count(&pattern), 1, "throttle should suppress duplicate rapid inserts");
    }

    #[test]
    fn record_outcome_records_significant_delay_change() {
        let engine = PredictionEngine::new();
        let mut status = make_status_with_history("C12345", "LEEDS");
        status.reported_delay_mins = Stamped::new(Some(5));
        engine.record_outcome(&status);
        // A change of ≥ MIN_DELAY_CHANGE_MINS (2) should bypass the time throttle.
        status.reported_delay_mins = Stamped::new(Some(10));
        engine.record_outcome(&status);
        use crate::prediction::types::ServicePattern;
        use chrono::{Datelike, Timelike};
        let pattern = ServicePattern {
            uid: "C12345".to_string(),
            weekday: status.scheduled_departure.value.weekday(),
            origin_crs: "LEEDS".to_string(),
            departure_hour: status.scheduled_departure.value.hour() as u8,
        };
        assert_eq!(engine.store.sample_count(&pattern), 2, "significant delay change should always be recorded");
    }

    #[test]
    fn missing_uid_produces_no_prediction() {
        let engine = PredictionEngine::new();
        let now = Utc::now();
        let mut status = TrainStatus::new(
            TrainId::rid("202404170123456").unwrap(),
            now, now,
        );
        // uid is None — pattern cannot be derived
        status.origin_crs = Some("LEEDS".to_string());
        status.reported_delay_mins = Stamped::new(Some(5));
        engine.record_outcome(&status); // should be a no-op
        engine.predict_and_update(&mut status);
        assert!(status.predicted_delay_mins.value.is_none());
    }

    // -----------------------------------------------------------------------
    // Phase 1: preceding service correlation tests
    // -----------------------------------------------------------------------

    #[test]
    fn preceding_service_with_high_delay_blends_prediction() {
        use chrono::Duration;
        let engine = PredictionEngine::new();

        // Target train: departs at T
        let now = Utc::now();
        let mut target = make_status_with_history("C12345", "LDS");
        // Scheduled departure is "now"
        target.scheduled_departure = Stamped::new(now);
        target.reported_delay_mins = Stamped::new(Some(5));
        seed_store(&engine, &target, &[5i32; 10]);

        // Preceding train: same origin, departs 10 mins before target, 15 mins late
        let mut preceding = TrainStatus::new(
            TrainId::rid("202404170000001").unwrap(),
            now - Duration::minutes(10),
            now - Duration::minutes(10),
        );
        preceding.origin_crs = Some("LDS".to_string());
        preceding.reported_delay_mins = Stamped::new(Some(15));

        let snapshot = vec![preceding];
        engine.predict_and_update_with_correlation(&mut target, Some(&snapshot));

        // Historical mean = 5, preceding delay = 15
        // blended = round(5 * 0.6 + 15 * 0.4) = round(3 + 6) = 9
        assert_eq!(target.predicted_delay_mins.value, Some(9));
        assert!(target.volatility.correlation_signal.is_some());
        let sig = target.volatility.correlation_signal.as_ref().unwrap();
        assert_eq!(sig.preceding_delay_mins, 15);
    }

    #[test]
    fn preceding_service_below_threshold_no_blend() {
        use chrono::Duration;
        let engine = PredictionEngine::new();

        let now = Utc::now();
        let mut target = make_status_with_history("C12345", "LDS");
        target.scheduled_departure = Stamped::new(now);
        target.reported_delay_mins = Stamped::new(Some(5));
        seed_store(&engine, &target, &[5i32; 10]);

        // Preceding train: only 3 mins late — below threshold
        let mut preceding = TrainStatus::new(
            TrainId::rid("202404170000001").unwrap(),
            now - Duration::minutes(10),
            now - Duration::minutes(10),
        );
        preceding.origin_crs = Some("LDS".to_string());
        preceding.reported_delay_mins = Stamped::new(Some(3));

        let snapshot = vec![preceding];
        engine.predict_and_update_with_correlation(&mut target, Some(&snapshot));

        // No blend — prediction stays at historical mean (5)
        assert_eq!(target.predicted_delay_mins.value, Some(5));
        assert!(target.volatility.correlation_signal.is_none());
    }

    #[test]
    fn no_registry_snapshot_produces_no_correlation() {
        let engine = PredictionEngine::new();
        let mut status = make_status_with_history("C12345", "LEEDS");
        status.reported_delay_mins = Stamped::new(Some(5));
        seed_store(&engine, &status, &[5i32; 10]);
        engine.predict_and_update(&mut status);
        assert!(status.volatility.correlation_signal.is_none());
        assert_eq!(status.predicted_delay_mins.value, Some(5));
    }

    // -----------------------------------------------------------------------
    // Phase 4: confidence decay tests
    // -----------------------------------------------------------------------

    #[test]
    fn fresh_history_does_not_decay_confidence() {
        use crate::prediction::types::DelayRecord;
        use chrono::Datelike;
        use chrono::Timelike;

        let engine = PredictionEngine::new();
        let now = Utc::now();
        let mut status = make_status_with_history("C12345", "LEEDS");
        status.reported_delay_mins = Stamped::new(Some(5));

        // Insert 45 records with recent timestamps (today)
        let pattern = crate::prediction::types::ServicePattern {
            uid: "C12345".to_string(),
            weekday: status.scheduled_departure.value.weekday(),
            origin_crs: "LEEDS".to_string(),
            departure_hour: status.scheduled_departure.value.hour() as u8,
        };
        for _ in 0..45 {
            engine.store.insert(
                pattern.clone(),
                DelayRecord { delay_mins: 5, predicted_delay_mins: None, recorded_at: now },
            );
        }

        engine.predict_and_update(&mut status);
        let confidence = status.volatility.historical_reliability.unwrap();
        // 45/90 = 0.5 — no decay because history is fresh
        assert!((confidence - 0.5).abs() < 0.01, "expected ~0.5, got {confidence}");
    }

    #[test]
    fn stale_history_decays_confidence() {
        use crate::prediction::types::DelayRecord;
        use chrono::Datelike;
        use chrono::Duration;
        use chrono::Timelike;

        let engine = PredictionEngine::new();
        let now = Utc::now();
        let stale_time = now - Duration::days(42); // 2× staleness threshold

        let mut status = make_status_with_history("C12345", "LEEDS");
        let pattern = crate::prediction::types::ServicePattern {
            uid: "C12345".to_string(),
            weekday: status.scheduled_departure.value.weekday(),
            origin_crs: "LEEDS".to_string(),
            departure_hour: status.scheduled_departure.value.hour() as u8,
        };
        for _ in 0..90 {
            engine.store.insert(
                pattern.clone(),
                DelayRecord { delay_mins: 5, predicted_delay_mins: None, recorded_at: stale_time },
            );
        }

        engine.predict_and_update(&mut status);
        let confidence = status.volatility.historical_reliability.unwrap();
        // raw = 1.0, decay = exp(-42/21) = exp(-2) ≈ 0.135
        assert!(confidence < 0.2, "expected decayed confidence < 0.2, got {confidence}");
    }
}
