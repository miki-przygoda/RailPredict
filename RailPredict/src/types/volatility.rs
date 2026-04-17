//! `VolatilityContext` — environmental and historical disruption metadata for a train service.
//!
//! Live weather feed integration is deferred. Fields that require an external data source
//! are `Option<T>` so the struct can be constructed and used before those feeds exist.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Environmental and historical context used by the state machine to decide volatility promotions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolatilityContext {
    /// Wind speed in mph at the route's most exposed point. `None` until weather feed is wired up.
    pub wind_speed_mph: Option<f32>,

    /// Whether a major incident affecting this corridor has been flagged (news/social scraper).
    pub incident_flagged: bool,

    /// Historical on-time rate for this service [0.0, 1.0]. `None` if no history is available.
    /// 1.0 = always on time; 0.0 = never on time.
    pub historical_reliability: Option<f32>,

    /// When this context was last refreshed from its data source.
    pub last_updated: DateTime<Utc>,
}

impl VolatilityContext {
    /// A neutral context with no live data — safe starting value for a newly-registered train.
    pub fn unknown() -> Self {
        Self {
            wind_speed_mph: None,
            incident_flagged: false,
            historical_reliability: None,
            last_updated: Utc::now(),
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
}
