//! `TrainState` enum and all transition logic.
//!
//! ## Complete state transition diagram
//!
//! ```text
//!                     departure > 2h
//!   [register] ──────────────────────────> Dormant
//!                                            │
//!                        120 > dep > 30 min  │  time-based promotion
//!                                            ▼
//!                                        Monitored
//!                                         │    ▲
//!               dep < 30 min  ────────────┘    │ dep pushed back > 30 min (demotion)
//!                                              │
//!                                            Active
//!                                         │    ▲
//!              dep < 5 min OR volatility  ┘    │ volatility resolves + dep > 5 min
//!                                              │
//!                                          Critical
//!                                              │
//!                         departed / cancelled │
//!                                              ▼
//!                                          Terminal (removed from registry)
//! ```
//!
//! ## Edge cases answered upfront
//!
//! - **Demotion Active → Monitored**: yes, if `actual_estimated_departure` is pushed back
//!   past the 30-minute threshold (e.g. major delay). The system must not over-poll a train
//!   whose departure has receded.
//!
//! - **Terminal state**: a departed or cancelled train enters `Terminal`. The poll manager
//!   removes it from the heap and the registry evicts it after a configurable buffer window.
//!   There is no polling in `Terminal`.
//!
//! - **Critical → Active on volatility resolution**: if the volatility event that caused a
//!   Critical promotion clears (wind drops, incident resolved) AND departure is > 5 min away,
//!   the train demotes back to `Active`. Time-based rules then apply normally.
//!
//! - **Emergency promotions bypass time thresholds**: any state can jump directly to `Critical`
//!   via `PromotionReason::VolatilityTriggered` or `PromotionReason::IncidentDetected`,
//!   regardless of departure time.

use chrono::{DateTime, Utc};

/// Why a state promotion was triggered. Carried on the `mpsc` notification so consumers
/// can distinguish routine time-based changes from emergency escalations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionReason {
    /// Departure time crossed a time-based threshold (routine).
    TimeBased,
    /// Wind speed on the route exceeded the critical threshold (>50 mph).
    VolatilityTriggered,
    /// A major incident on the corridor was detected by the news/social scraper.
    /// Full integration deferred to Epic 4 (ingestion pipeline).
    #[allow(dead_code)]
    IncidentDetected,
}

/// The urgency state of a single train service, which determines polling frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrainState {
    /// Departure > 2 hours away. No live calls; Tier A static data only.
    Dormant,
    /// 30–120 minutes to departure. Poll every 10 minutes.
    Monitored,
    /// 0–30 minutes to departure. Poll every 30–60 seconds.
    Active,
    /// < 5 minutes to departure OR volatility-triggered. Push-port stream or 10-second polling.
    Critical,
    /// Train has departed or been cancelled. No further polling; pending registry eviction.
    Terminal,
}

impl TrainState {
    /// Poll interval for this state. Returns `None` for states that do not poll.
    pub fn poll_interval(&self) -> Option<std::time::Duration> {
        match self {
            Self::Dormant => None,
            Self::Monitored => Some(std::time::Duration::from_secs(600)),  // 10 min
            Self::Active => Some(std::time::Duration::from_secs(45)),      // 30–60s midpoint
            Self::Critical => Some(std::time::Duration::from_secs(10)),
            Self::Terminal => None,
        }
    }

    /// Compute the correct state given the time until the best estimated departure and
    /// whether a volatility event is currently active.
    ///
    /// This is the single authoritative rule set; called both on registration and on
    /// each state re-evaluation tick.
    pub fn from_departure(
        best_departure: DateTime<Utc>,
        now: DateTime<Utc>,
        volatility_active: bool,
        is_terminated: bool,
    ) -> Self {
        if is_terminated {
            return Self::Terminal;
        }

        let mins_until = (best_departure - now).num_minutes();

        if volatility_active || mins_until < 5 {
            Self::Critical
        } else if mins_until < 30 {
            Self::Active
        } else if mins_until < 120 {
            Self::Monitored
        } else {
            Self::Dormant
        }
    }

    /// Apply an emergency promotion regardless of departure time.
    /// Returns the new state (always `Critical` for volatility/incident triggers).
    pub fn emergency_promote(&self, reason: &PromotionReason) -> Self {
        match reason {
            PromotionReason::TimeBased => *self,
            PromotionReason::VolatilityTriggered | PromotionReason::IncidentDetected => {
                Self::Critical
            }
        }
    }
}

impl std::fmt::Display for TrainState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dormant => write!(f, "Dormant"),
            Self::Monitored => write!(f, "Monitored"),
            Self::Active => write!(f, "Active"),
            Self::Critical => write!(f, "Critical"),
            Self::Terminal => write!(f, "Terminal"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn now_plus(mins: i64) -> DateTime<Utc> {
        Utc::now() + Duration::minutes(mins)
    }

    #[test]
    fn far_future_is_dormant() {
        let state = TrainState::from_departure(now_plus(180), Utc::now(), false, false);
        assert_eq!(state, TrainState::Dormant);
    }

    #[test]
    fn monitored_window() {
        let state = TrainState::from_departure(now_plus(60), Utc::now(), false, false);
        assert_eq!(state, TrainState::Monitored);
    }

    #[test]
    fn active_window() {
        let state = TrainState::from_departure(now_plus(15), Utc::now(), false, false);
        assert_eq!(state, TrainState::Active);
    }

    #[test]
    fn under_five_mins_is_critical() {
        let state = TrainState::from_departure(now_plus(3), Utc::now(), false, false);
        assert_eq!(state, TrainState::Critical);
    }

    #[test]
    fn volatility_forces_critical_regardless_of_time() {
        let state = TrainState::from_departure(now_plus(180), Utc::now(), true, false);
        assert_eq!(state, TrainState::Critical);
    }

    #[test]
    fn terminated_is_always_terminal() {
        let state = TrainState::from_departure(now_plus(180), Utc::now(), true, true);
        assert_eq!(state, TrainState::Terminal);
    }

    #[test]
    fn emergency_promote_overrides_dormant() {
        let new_state = TrainState::Dormant.emergency_promote(&PromotionReason::VolatilityTriggered);
        assert_eq!(new_state, TrainState::Critical);
    }

    #[test]
    fn time_based_reason_does_not_promote_via_emergency() {
        let new_state = TrainState::Dormant.emergency_promote(&PromotionReason::TimeBased);
        assert_eq!(new_state, TrainState::Dormant);
    }

    #[test]
    fn poll_intervals_match_spec() {
        assert_eq!(TrainState::Dormant.poll_interval(), None);
        assert_eq!(TrainState::Monitored.poll_interval(), Some(std::time::Duration::from_secs(600)));
        assert_eq!(TrainState::Active.poll_interval(), Some(std::time::Duration::from_secs(45)));
        assert_eq!(TrainState::Critical.poll_interval(), Some(std::time::Duration::from_secs(10)));
        assert_eq!(TrainState::Terminal.poll_interval(), None);
    }
}
