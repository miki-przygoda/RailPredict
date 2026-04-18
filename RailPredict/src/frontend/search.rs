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
    let entries = state.registry.departure_snapshot(&crs_upper).await;
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
                            (delay_badge(entry.delay_mins, entry.is_cancelled.unwrap_or(false)))
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
