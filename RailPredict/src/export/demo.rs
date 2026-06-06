//! Self-contained exec demo site (`export-demo` CLI subcommand).
//!
//! Bakes real KPIs and a DB-reconstructed predicted→actual replay into one
//! offline HTML file for a CEO sales demo. The replay frames match the
//! `static/board.js` renderer contract exactly so the same renderer (inlined,
//! light-themed) drives them. Real data is shown as real; the "with your
//! ticketing data" figures are clearly labelled projected.

use std::path::Path;

use chrono::Utc;
use serde::Serialize;

use crate::db::Db;

/// A train still being tracked — shows the prediction only (no actual yet).
/// Field names mirror `static/board.js` `trackCard()`.
#[derive(Serialize, Clone, Debug)]
pub struct TrackCard {
    pub rid: String,
    pub operator: String,
    pub brand: String,
    pub label: String,
    pub origin: String,
    pub dest: String,
    pub scheduled: String,
    pub predicted: i32,
}

/// A settled train — shows predicted → actual and the absolute error.
/// Field names mirror `static/board.js` `settledCard()`.
#[derive(Serialize, Clone, Debug)]
pub struct SettledCard {
    pub rid: String,
    pub operator: String,
    pub brand: String,
    pub label: String,
    pub origin: String,
    pub dest: String,
    pub predicted: i32,
    pub actual: i32,
    pub delta: i32,
}

/// One board snapshot — the unit the renderer consumes.
#[derive(Serialize, Clone, Debug)]
pub struct Frame {
    pub tracking: Vec<TrackCard>,
    pub settled: Vec<SettledCard>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_card_has_no_actual_key() {
        let c = TrackCard {
            rid: "r1".into(), operator: "GWR".into(), brand: "#0a493e".into(),
            label: "1A23".into(), origin: "PAD".into(), dest: "BRI".into(),
            scheduled: "09:15".into(), predicted: 4,
        };
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("predicted").is_some());
        assert!(v.get("actual").is_none(), "tracking card must not expose an actual");
    }

    #[test]
    fn settled_card_exposes_predicted_actual_delta() {
        let c = SettledCard {
            rid: "r1".into(), operator: "GWR".into(), brand: "#0a493e".into(),
            label: "1A23".into(), origin: "PAD".into(), dest: "BRI".into(),
            predicted: 4, actual: 9, delta: 5,
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["predicted"], 4);
        assert_eq!(v["actual"], 9);
        assert_eq!(v["delta"], 5);
    }
}
