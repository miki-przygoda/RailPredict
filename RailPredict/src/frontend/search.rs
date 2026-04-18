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
                    hx-trigger="submit, every 30s"
                    hx-swap="innerHTML transition:true"
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

        // Phase 1: compute how many seconds ago the most recently updated Stamped field was
        // written. We take the maximum (most recent) last_updated across the fields that change
        // from live sources so the staleness indicator reflects whether *any* live data arrived.
        let most_recent = [
            status.actual_estimated_departure.last_updated,
            status.reported_delay_mins.last_updated,
            status.actual_platform.last_updated,
            status.is_cancelled.last_updated,
        ]
        .into_iter()
        .max();
        let last_updated_secs_ago = most_recent.map(|ts| {
            let delta = chrono::Utc::now() - ts;
            delta.num_seconds().max(0) as u64
        });

        // Phase 4: destination station name from destination_crs on TrainStatus.
        // AppState does not yet expose db — add a TODO note and leave None for now.
        // TODO: wire AppState::db and call db::static_data::get_station to resolve destination_crs
        let destination_name: Option<String> = status
            .destination_crs
            .as_deref()
            .map(|crs| crs.to_string()); // fallback: render CRS directly until DB is wired

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
            last_updated_secs_ago,
            destination_name,
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
                    // Phase 1: mark cards whose data is older than 120 seconds as stale.
                    @let stale = entry.last_updated_secs_ago.is_some_and(|s| s > 120);
                    a .train-card
                      data-stale=[if stale { Some("true") } else { None::<&str> }]
                      href={ "/trains/" (entry.rid) "/view" }
                    {
                        div .train-card-left {
                            span .train-time {
                                (entry.scheduled_departure.get(11..16).unwrap_or("--:--"))
                            }
                            (delay_badge(entry.delay_mins, entry.is_cancelled))
                            // Phase 4: destination station name.
                            @if let Some(dest) = &entry.destination_name {
                                span .train-destination { "→ " (dest) }
                            }
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
