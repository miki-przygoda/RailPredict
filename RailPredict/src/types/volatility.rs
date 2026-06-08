//! `VolatilityContext` — environmental and historical disruption metadata for a train service.
//!
//! Live weather feed integration is deferred. Fields that require an external data source
//! are `Option<T>` so the struct can be constructed and used before those feeds exist.
//!
//! ## Phase 1 (AdvancedAnalytics): `CorrelationSignal` added
//! Carries the preceding-service delay signal used to weight the Tier B prediction blend.
//! Populated by `PredictionEngine::predict_and_update` when a registry snapshot is provided.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use super::TrainId;

/// Auditable record of a preceding-service delay that influenced the Tier B prediction.
///
/// When a service departing from the same origin within the previous 20 minutes is
/// running more than 5 minutes late, the prediction engine blends its delay into the
/// historical trimmed mean. This struct records the signal so the UI can surface it:
/// "Prediction adjusted because the preceding service (RID …) is currently 15 mins late."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationSignal {
    /// The `TrainId` of the service whose delay influenced this prediction.
    pub preceding_rid: TrainId,
    /// The reported delay of the preceding service in minutes.
    pub preceding_delay_mins: i32,
    /// The weight applied to the preceding-service delay in the blended prediction
    /// (complement weight applied to the historical trimmed mean).
    /// Currently fixed at `PRECEDING_WEIGHT` (0.4) in `prediction/engine.rs`.
    pub weight: f32,
}

/// Environmental and historical context used by the state machine to decide volatility promotions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolatilityContext {
    /// Wind speed in mph at the route's most exposed point. `None` until weather feed is wired up.
    pub wind_speed_mph: Option<f32>,

    /// Whether a major incident affecting this corridor has been flagged (news/social scraper).
    /// Also set to `true` for trains affected by a TIPLOC cascade (Phase 2 AdvancedAnalytics).
    pub incident_flagged: bool,

    /// Historical on-time rate for this service [0.0, 1.0]. `None` if no history is available.
    /// 1.0 = always on time; 0.0 = never on time.
    /// Phase 4 (AdvancedAnalytics): this value has exponential decay applied when history is
    /// older than `STALENESS_THRESHOLD_DAYS` (21 days) — it approaches zero as history ages.
    pub historical_reliability: Option<f32>,

    /// When this context was last refreshed from its data source.
    pub last_updated: DateTime<Utc>,

    /// Preceding-service correlation signal used in the Phase 1 blended prediction.
    /// `None` when no qualifying preceding service was detected at prediction time, or
    /// when the prediction engine was called without a registry snapshot.
    pub correlation_signal: Option<CorrelationSignal>,

    /// Reported delay of the predecessor service (same physical train set, previous trip).
    /// Set from Darwin `Association` messages (category NP) via the ingestion pipeline.
    /// `None` until a turnround association is seen for this RID.
    pub predecessor_train_delay_mins: Option<i32>,

    /// Named ML feature vector used to produce the most recent ONNX prediction.
    /// Persisted to `prediction_outcomes.features` and `prediction_snapshots.features`
    /// so every stored prediction is replayable and usable as a training row.
    /// `None` when the statistical fallback (trimmed mean) was used instead of ONNX.
    pub prediction_features: Option<JsonValue>,
}

impl VolatilityContext {
    /// A neutral context with no live data — safe starting value for a newly-registered train.
    pub fn unknown() -> Self {
        Self {
            wind_speed_mph: None,
            incident_flagged: false,
            historical_reliability: None,
            last_updated: Utc::now(),
            correlation_signal: None,
            predecessor_train_delay_mins: None,
            prediction_features: None,
        }
    }

    /// Returns `true` if wind speed exceeds the threshold that forces Active state (50 mph).
    pub fn is_wind_critical(&self) -> bool {
        self.wind_speed_mph.is_some_and(|w| w > 50.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_context_is_not_wind_critical() {
        assert!(!VolatilityContext::unknown().is_wind_critical());
    }

    #[test]
    fn wind_above_threshold_is_critical() {
        let ctx = VolatilityContext {
            wind_speed_mph: Some(55.0),
            ..VolatilityContext::unknown()
        };
        assert!(ctx.is_wind_critical());
    }

    #[test]
    fn wind_at_threshold_is_not_critical() {
        let ctx = VolatilityContext {
            wind_speed_mph: Some(50.0),
            ..VolatilityContext::unknown()
        };
        assert!(!ctx.is_wind_critical());
    }

    #[test]
    fn unknown_context_has_no_correlation_signal() {
        assert!(VolatilityContext::unknown().correlation_signal.is_none());
    }

    #[test]
    fn correlation_signal_is_serialisable() {
        use crate::types::TrainId;
        let sig = CorrelationSignal {
            preceding_rid: TrainId::rid("202404170123456").unwrap(),
            preceding_delay_mins: 15,
            weight: 0.4,
        };
        let ctx = VolatilityContext {
            correlation_signal: Some(sig),
            ..VolatilityContext::unknown()
        };
        let json = serde_json::to_string(&ctx).expect("serialise");
        assert!(json.contains("preceding_delay_mins"));
    }
}
