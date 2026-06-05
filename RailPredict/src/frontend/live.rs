//! Live board (`/live`) + its JSON snapshot (`/ui/live/snapshot`).
//!
//! The board is rendered client-side by `static/board.js` from the snapshot JSON,
//! so the *same* renderer drives the live page and the server-free `replay.html`.
//! `/live` is a thin maud shell; `/ui/live/snapshot` is the polled data feed —
//! `tracking` from the in-memory registry, `settled` from finalised outcomes.

use axum::Json;
use axum::extract::State;
use maud::{Markup, html};
use serde::Serialize;

use crate::api::AppState;
use crate::cache::location_names;
use crate::db::predictions;

use super::layout::{NavPage, base};

const NEUTRAL_BRAND: &str = "#9aa7b4";

/// A tracking-zone card: a train currently active, with its live prediction.
#[derive(Serialize)]
pub struct TrackCard {
    rid: String,
    label: String,
    operator: String,
    brand: String,
    origin: String,
    dest: String,
    scheduled: String,
    predicted: i32,
}

/// A settled-zone card: a train that has arrived — predicted vs actual.
#[derive(Serialize)]
pub struct SettledCard {
    rid: String,
    label: String,
    operator: String,
    brand: String,
    origin: String,
    dest: String,
    predicted: i32,
    actual: i32,
    delta: i32,
}

#[derive(Serialize)]
pub struct Snapshot {
    t: i64,
    tracking: Vec<TrackCard>,
    settled: Vec<SettledCard>,
}

/// `GET /live` — the board shell. All board content is rendered by `board.js`.
pub async fn live_page() -> Markup {
    let body = html! {
        div .live {
            div .lvh {
                span .lvh-title { "Live Network" }
                span .lvh-live { span .dot {} span # "live-count" { "—" } " tracking" }
                span .lvh-spacer {}
                div .chips #filters {}
                button .rec #record type="button" { span .rdot {} span # "record-label" { "Record" } }
                a .rec-dl.hidden #download href="#" download="railpredict-capture.json" { "Download" }
            }
            div #board .zones {
                p .panel-empty { "Connecting to the live feed…" }
            }
            p .live-foot {
                "A train enters " b { "Tracking" } " with its prediction, then moves to "
                b { "Just settled" } " with predicted vs actual. " b { "Record" }
                " captures the stream → " b { "Download" } " a JSON → open "
                code { "replay.html" } " to replay it with no server."
            }
        }
        script src="/static/board.js" {}
    };
    base("Live", NavPage::Live, body)
}

/// `GET /ui/live/snapshot` — current board state as JSON (polled by `board.js`).
pub async fn live_snapshot(State(state): State<AppState>) -> Json<Snapshot> {
    let tracking_raw = state.registry.tracking_board(24).await;
    let settled_raw = predictions::recent_settled(&state.db, 12).await.unwrap_or_default();

    let tracking = tracking_raw
        .into_iter()
        .map(|t| TrackCard {
            label: t.uid.clone().unwrap_or_else(|| t.rid.clone()),
            rid: t.rid,
            operator: "—".to_string(),
            brand: NEUTRAL_BRAND.to_string(),
            origin: t.origin_crs.as_deref().map(location_names::name_or_code).unwrap_or("???").to_string(),
            dest: t.destination_crs.as_deref().map(location_names::name_or_code).unwrap_or("—").to_string(),
            scheduled: t.scheduled_departure.format("%H:%M").to_string(),
            predicted: t.predicted_delay_mins,
        })
        .collect();

    let settled = settled_raw
        .into_iter()
        .map(|o| SettledCard {
            label: o.uid,
            rid: o.rid,
            operator: o.operator.unwrap_or_else(|| "—".to_string()),
            brand: o.brand_color.unwrap_or_else(|| NEUTRAL_BRAND.to_string()),
            origin: location_names::name_or_code(&o.origin_crs).to_string(),
            dest: o.destination_crs.as_deref().map(location_names::name_or_code).unwrap_or("—").to_string(),
            predicted: o.predicted_delay_mins,
            actual: o.final_delay_mins,
            delta: (o.final_delay_mins - o.predicted_delay_mins).abs(),
        })
        .collect();

    Json(Snapshot { t: chrono::Utc::now().timestamp(), tracking, settled })
}
