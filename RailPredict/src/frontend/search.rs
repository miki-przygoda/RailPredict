//! Search page and departure board fragment handler.

use axum::extract::{Query, State};
use maud::{Markup, html};
use serde::Deserialize;

use crate::api::types::DepartureBoardEntry;
use crate::api::AppState;

use super::components::{delay_badge, platform_chip};
use super::layout::base;

pub async fn search_page() -> Markup {
    base(
        "Search",
        html! {
            div .search-container {
                h1 { "Where are you going?" }
                form
                    hx-get="/ui/stations/departures"
                    hx-target="#results"
                    hx-trigger="submit"
                {
                    input
                        type="text"
                        name="crs"
                        placeholder="Station code (e.g. KGX)"
                        autocomplete="off"
                        maxlength="3";
                    button type="submit" { "Search" }
                }
                div #results {}
            }
        },
    )
}

#[derive(Deserialize)]
pub struct CrsQuery {
    pub crs: String,
}

pub async fn departures_fragment(
    Query(q): Query<CrsQuery>,
    State(state): State<AppState>,
) -> Markup {
    let crs_upper = q.crs.trim().to_uppercase();
    let arcs = state.registry.snapshot_all();
    let mut entries: Vec<DepartureBoardEntry> = Vec::new();

    for arc in arcs {
        let status = arc.read().await;
        if status.origin_crs.as_deref().map(str::to_uppercase).as_deref() != Some(crs_upper.as_str())
        {
            continue;
        }
        entries.push(DepartureBoardEntry {
            rid: status.id.to_string(),
            scheduled_departure: status.scheduled_departure.value.to_rfc3339(),
            estimated_departure: status
                .actual_estimated_departure
                .value
                .map(|dt| dt.to_rfc3339()),
            delay_mins: status.best_delay_mins(),
            platform: status.best_platform().map(str::to_string),
            is_cancelled: status.is_cancelled.value,
        });
    }
    entries.sort_by_key(|e| e.scheduled_departure.clone());

    departure_board_fragment(&crs_upper, &entries)
}

pub fn departure_board_fragment(crs: &str, entries: &[DepartureBoardEntry]) -> Markup {
    html! {
        @if entries.is_empty() {
            p .no-results { "No departures found for " (crs) "." }
        } @else {
            div .departure-board {
                h2 { "Departures from " (crs) }
                @for entry in entries {
                    a .train-card href={ "/trains/" (entry.rid) "/view" } {
                        div .train-card-left {
                            span .train-time {
                                (entry.scheduled_departure.get(11..16).unwrap_or("--:--"))
                            }
                            (delay_badge(entry.delay_mins, entry.is_cancelled))
                        }
                        div .train-card-right {
                            span .train-rid { (entry.rid) }
                            (platform_chip(entry.platform.as_deref()))
                        }
                    }
                }
            }
        }
    }
}
