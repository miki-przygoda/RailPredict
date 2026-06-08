//! `TrainState` — the urgency vocabulary for a tracked train.
//!
//! `TrainState` classifies how close to departure (and how volatile) a service is,
//! which determines how aggressively it should be polled. States are set **inline by
//! the ingestion pipeline** today (see `ingestion/mod.rs`); a poll scheduler that acts
//! on them will be (re)built when Tier C live polling is wired — see `docs/tech-debt.md`.
//!
//! | State       | Meaning                                   | Cadence        |
//! |-------------|-------------------------------------------|----------------|
//! | `Dormant`   | Departure > 2h away — Tier A static only  | no polling     |
//! | `Monitored` | 30–120 min to departure                   | ~10 min        |
//! | `Active`    | 0–30 min to departure                     | ~30–60 s       |
//! | `Critical`  | < 5 min OR volatility-triggered           | ~10 s / stream |
//! | `Terminal`  | Departed or cancelled — pending eviction  | no polling     |

use crate::types::TrainId;

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

/// Broadcast whenever a train changes state — consumed by the API/SSE layer to push
/// live updates to the UI. Emitted today by the ingestion pipeline on emergency
/// (cancelled/delayed) promotions and on deactivation.
#[derive(Debug, Clone)]
pub struct StateChangeEvent {
    pub train_id: TrainId,
    pub old_state: TrainState,
    pub new_state: TrainState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_all_states() {
        assert_eq!(TrainState::Dormant.to_string(), "Dormant");
        assert_eq!(TrainState::Monitored.to_string(), "Monitored");
        assert_eq!(TrainState::Active.to_string(), "Active");
        assert_eq!(TrainState::Critical.to_string(), "Critical");
        assert_eq!(TrainState::Terminal.to_string(), "Terminal");
    }
}
