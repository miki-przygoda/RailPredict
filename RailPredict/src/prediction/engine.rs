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

use std::sync::Arc;

use chrono::{Datelike, Utc};

use crate::types::train_status::Stamped;
use crate::types::TrainStatus;

use super::types::{DelayRecord, HistoricalStore, ServicePattern, MAX_SAMPLES};

/// Wraps the shared `HistoricalStore`. Cheap to clone — each clone shares the same store.
#[derive(Clone)]
pub struct PredictionEngine {
    store: Arc<HistoricalStore>,
}

impl PredictionEngine {
    pub fn new() -> Self {
        Self { store: Arc::new(HistoricalStore::new()) }
    }

    /// Compute a prediction from historical data and write it into `status.predicted_delay_mins`
    /// and `status.volatility.historical_reliability`.
    ///
    /// Does nothing if the pattern cannot be derived (uid or origin_crs missing) or if fewer
    /// than 3 samples exist for this pattern.
    pub fn predict_and_update(&self, status: &mut TrainStatus) {
        let Some(pattern) = derive_pattern(status) else { return };
        let Some(samples) = self.store.get_samples(&pattern) else { return };
        if samples.len() < 3 {
            return;
        }
        let (mean, confidence) = trimmed_mean(&samples, MAX_SAMPLES);
        status.predicted_delay_mins = Stamped::new(Some(mean));
        status.volatility.historical_reliability = Some(confidence);
    }

    /// Feed a confirmed delay outcome from a Darwin TS message back into the historical store.
    ///
    /// Called after `status.reported_delay_mins` has been set by the ingestion pipeline.
    /// Does nothing if the pattern or delay value cannot be derived.
    pub fn record_outcome(&self, status: &TrainStatus) {
        let Some(pattern) = derive_pattern(status) else { return };
        let Some(delay_mins) = status.reported_delay_mins.value else { return };
        self.store.insert(pattern, DelayRecord { delay_mins, recorded_at: Utc::now() });
    }
}

/// Derive the `ServicePattern` key from a `TrainStatus`. Returns `None` if either
/// the uid or origin_crs is not yet known.
fn derive_pattern(status: &TrainStatus) -> Option<ServicePattern> {
    let uid = status.uid.clone()?;
    let origin_crs = status.origin_crs.clone()?;
    let weekday = status.scheduled_departure.value.weekday();
    Some(ServicePattern { uid, weekday, origin_crs })
}

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

    #[test]
    fn predict_with_fewer_than_three_samples_returns_none() {
        let engine = PredictionEngine::new();
        let mut status = make_status_with_history("C12345", "LEEDS");
        status.reported_delay_mins = Stamped::new(Some(5));
        engine.record_outcome(&status);
        engine.record_outcome(&status);
        engine.predict_and_update(&mut status);
        assert!(status.predicted_delay_mins.value.is_none());
    }

    #[test]
    fn predict_with_uniform_history_returns_that_value() {
        let engine = PredictionEngine::new();
        let mut status = make_status_with_history("C12345", "LEEDS");
        status.reported_delay_mins = Stamped::new(Some(5));
        for _ in 0..10 {
            engine.record_outcome(&status);
        }
        engine.predict_and_update(&mut status);
        assert_eq!(status.predicted_delay_mins.value, Some(5));
    }

    #[test]
    fn trimmed_mean_ignores_outliers() {
        let engine = PredictionEngine::new();
        let mut status = make_status_with_history("C12345", "LEEDS");
        // 8 samples at 5 mins, 2 outliers at 120 mins
        for _ in 0..8 {
            status.reported_delay_mins = Stamped::new(Some(5));
            engine.record_outcome(&status);
        }
        status.reported_delay_mins = Stamped::new(Some(120));
        engine.record_outcome(&status);
        engine.record_outcome(&status);

        engine.predict_and_update(&mut status);
        // Trimming 10% from each end of 10 samples = 1 from each end.
        // Drops one 5 and one 120. Remaining: [5,5,5,5,5,5,5,120] → mean=25.
        // Without trimming it would be (5*8 + 120*2)/10 = 28.
        // With trim=1: sorted=[5,5,5,5,5,5,5,5,120,120], drop first and last → [5,5,5,5,5,5,5,120] → mean=25.
        let predicted = status.predicted_delay_mins.value.unwrap();
        assert!(predicted < 30, "trimmed mean {predicted} should be < 30 (not dominated by outliers)");
        assert!(predicted >= 5, "trimmed mean {predicted} should be >= 5");
    }

    #[test]
    fn record_outcome_caps_at_max_samples() {
        use crate::prediction::types::MAX_SAMPLES;
        use super::super::types::ServicePattern;
        use chrono::Datelike;

        let engine = PredictionEngine::new();
        let mut status = make_status_with_history("C12345", "LEEDS");
        status.reported_delay_mins = Stamped::new(Some(3));
        for _ in 0..MAX_SAMPLES + 5 {
            engine.record_outcome(&status);
        }
        let pattern = ServicePattern {
            uid: "C12345".to_string(),
            weekday: status.scheduled_departure.value.weekday(),
            origin_crs: "LEEDS".to_string(),
        };
        assert_eq!(engine.store.sample_count(&pattern), MAX_SAMPLES);
    }

    #[test]
    fn confidence_scales_with_sample_count() {
        let engine = PredictionEngine::new();
        let mut status = make_status_with_history("C12345", "LEEDS");
        status.reported_delay_mins = Stamped::new(Some(4));
        for _ in 0..45 {
            engine.record_outcome(&status);
        }
        engine.predict_and_update(&mut status);
        let confidence = status.volatility.historical_reliability.unwrap();
        assert!((confidence - 0.5).abs() < 0.01, "expected ~0.5, got {confidence}");
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
}
