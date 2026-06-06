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

/// One real settled outcome, the raw material for the replay timeline.
#[derive(Clone, Debug)]
pub struct ReplayTrain {
    pub rid: String,
    pub operator: String,
    pub brand: String,
    pub label: String,
    pub origin: String,
    pub dest: String,
    pub scheduled: String,
    pub predicted: i32,
    pub actual: i32,
}

fn to_track(t: &ReplayTrain) -> TrackCard {
    TrackCard {
        rid: t.rid.clone(), operator: t.operator.clone(), brand: t.brand.clone(),
        label: t.label.clone(), origin: t.origin.clone(), dest: t.dest.clone(),
        scheduled: t.scheduled.clone(), predicted: t.predicted,
    }
}

fn to_settled(t: &ReplayTrain) -> SettledCard {
    SettledCard {
        rid: t.rid.clone(), operator: t.operator.clone(), brand: t.brand.clone(),
        label: t.label.clone(), origin: t.origin.clone(), dest: t.dest.clone(),
        predicted: t.predicted, actual: t.actual, delta: (t.actual - t.predicted).abs(),
    }
}

/// Slide a window across the trains: at step `i`, trains `[i .. i+tracking_window)`
/// are still tracking (prediction only) and the previous `settled_window` trains
/// are shown settled, most-recent first. Produces `trains.len()+1` candidate frames;
/// fully-empty frames are dropped.
pub fn build_frames(
    trains: &[ReplayTrain],
    tracking_window: usize,
    settled_window: usize,
) -> Vec<Frame> {
    let mut frames = Vec::new();
    for i in 0..=trains.len() {
        let settled_start = i.saturating_sub(settled_window);
        let settled: Vec<SettledCard> = trains[settled_start..i].iter().rev().map(to_settled).collect();
        let tracking_end = (i + tracking_window).min(trains.len());
        let tracking: Vec<TrackCard> = trains[i..tracking_end].iter().map(to_track).collect();
        if tracking.is_empty() && settled.is_empty() {
            continue;
        }
        frames.push(Frame { tracking, settled });
    }
    frames
}

const DEMO_TEMPLATE: &str = include_str!("demo_template.html");

#[derive(Serialize, Clone, Debug)]
pub struct OperatorHighlight {
    pub name: String,
    pub brand: String,
    pub on_time_pct: f64,
    pub journeys: i64,
}

/// Clearly-labelled illustrative figures for the "with your ticketing data" beat.
/// These are NOT measured — the template badges them PROJECTED and prints `note`.
#[derive(Serialize, Clone, Debug)]
pub struct ProjectedFigures {
    pub disrupted_tickets_pct: f64,
    pub recoverable_revenue: String,
    pub churn_reduction_pct: f64,
    pub note: String,
}

impl Default for ProjectedFigures {
    fn default() -> Self {
        ProjectedFigures {
            disrupted_tickets_pct: 6.0,
            recoverable_revenue: "£1.4M".into(),
            churn_reduction_pct: 22.0,
            note: "Illustrative — modelled on representative ticketing volumes, not measured."
                .into(),
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct DemoData {
    pub generated_at: String,
    pub hero_number: String,
    pub hero_observations: i64,
    pub services_count: i64,
    pub on_time_pct: Option<f64>,
    pub mae_mins: Option<f64>,
    pub within_5_pct: Option<f64>,
    pub operators: Vec<OperatorHighlight>,
    pub frames: Vec<Frame>,
    pub projected: ProjectedFigures,
}

/// Format a count as a compact headline string: 2_546_226 -> "2.5M+".
pub fn human_count(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M+", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}K+", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Inject the data as a JSON literal into the template. Pure — no DB.
/// Escapes `</` so a stray `</script>` in a string field can't break out of the
/// host <script> (mirrors `export::render_html`).
pub fn render_demo_html(data: &DemoData) -> anyhow::Result<String> {
    let json = serde_json::to_string(data)?.replace("</", "<\\/");
    Ok(DEMO_TEMPLATE.replace("__DEMO_DATA__", &json))
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

    fn sample(n: usize) -> Vec<ReplayTrain> {
        (0..n).map(|k| ReplayTrain {
            rid: format!("r{k}"), operator: "GWR".into(), brand: "#0a493e".into(),
            label: format!("1A0{k}"), origin: "PAD".into(), dest: "BRI".into(),
            scheduled: "09:15".into(), predicted: 3, actual: 8,
        }).collect()
    }

    #[test]
    fn frames_progress_from_tracking_to_settled() {
        let trains = sample(3);
        let frames = build_frames(&trains, 2, 2);
        assert_eq!(frames.len(), 4); // i = 0..=3, none empty
        // First frame: nothing settled, trains tracking.
        assert_eq!(frames[0].settled.len(), 0);
        assert_eq!(frames[0].tracking.len(), 2);
        // Last frame: nothing tracking, recent trains settled (most recent first).
        assert_eq!(frames[3].tracking.len(), 0);
        assert_eq!(frames[3].settled[0].rid, "r2");
        // r0 is tracking at frame 0 and settled by frame 1.
        assert!(frames[0].tracking.iter().any(|c| c.rid == "r0"));
        assert!(frames[1].settled.iter().any(|c| c.rid == "r0"));
    }

    #[test]
    fn settled_delta_is_absolute_error() {
        let frames = build_frames(&sample(1), 1, 1);
        let last = frames.last().unwrap();
        assert_eq!(last.settled[0].delta, 5); // |8 - 3|
    }

    #[test]
    fn empty_input_yields_no_frames() {
        assert!(build_frames(&[], 4, 4).is_empty());
    }

    fn demo_fixture() -> DemoData {
        DemoData {
            generated_at: "06 Jun 2026 10:00 UTC".into(),
            hero_number: "2.5M+".into(),
            hero_observations: 2_546_226,
            services_count: 74_000,
            on_time_pct: Some(91.4),
            mae_mins: Some(3.2),
            within_5_pct: Some(78.0),
            operators: vec![],
            frames: build_frames(&sample(2), 2, 2),
            projected: ProjectedFigures::default(),
        }
    }

    #[test]
    fn render_replaces_placeholder_and_keeps_anchors() {
        let html = render_demo_html(&demo_fixture()).unwrap();
        assert!(!html.contains("__DEMO_DATA__"), "placeholder must be replaced");
        assert!(html.contains(r#"id="beat-replay""#));
        assert!(html.contains("2.5M+") || html.contains("2546226"));
    }

    #[test]
    fn human_count_formats_compactly() {
        assert_eq!(human_count(2_546_226), "2.5M+");
        assert_eq!(human_count(74_000), "74K+");
        assert_eq!(human_count(512), "512");
    }
}
