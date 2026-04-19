//! Search page and departure board fragment handler.

use axum::extract::{Query, State};
use maud::{Markup, html};
use serde::Deserialize;

use crate::api::handlers::{StationResult, StationSearchQuery};
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
                    hx-include="[name='crs']"
                {
                    div style="position:relative" {
                        // Visible text input — triggers autocomplete
                        input
                            type="text"
                            name="q"
                            id="station-q"
                            placeholder="Station name (e.g. London Kings Cross)"
                            autocomplete="off"
                            hx-get="/ui/stations/search"
                            hx-trigger="input changed delay:300ms"
                            hx-target="#station-suggestions"
                            hx-include="[name='q']";
                        // Hidden input carrying the actual CRS code for the form submit
                        input
                            type="hidden"
                            name="crs"
                            id="crs-hidden"
                            value="";
                        // Autocomplete suggestion list target
                        div #station-suggestions {}
                    }
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
    // Item 2.3r: merged DB + registry departure board.
    let entries = crate::api::handlers::build_departure_board(&state, &crs_upper).await;
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

/// `GET /ui/stations/search?q=<term>`
///
/// Returns an HTML `<ul>` of matching station suggestions for the autocomplete
/// dropdown. Each `<li>` sets the hidden CRS input and the visible text input
/// on click, then clears the suggestions div.
pub async fn station_suggestions_fragment(
    Query(params): Query<StationSearchQuery>,
    State(state): State<AppState>,
) -> Markup {
    let q = params.q.trim().to_string();
    if q.len() < 2 {
        return html! {};
    }

    let results: Vec<StationResult> = sqlx::query_as::<_, (String, String)>(
        "SELECT crs, name FROM stations \
         WHERE to_tsvector('english', name) @@ plainto_tsquery('english', $1) \
         ORDER BY name LIMIT 10",
    )
    .bind(&q)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(crs, name)| StationResult { crs, name })
    .collect();

    if results.is_empty() {
        return html! {};
    }

    html! {
        ul .station-suggestions {
            @for result in &results {
                @let crs_val = result.crs.clone();
                @let name_val = result.name.clone();
                li
                    style="cursor:pointer"
                    hx-on:click={
                        "document.getElementById('crs-hidden').value='"
                        (crs_val)
                        "';"
                        "document.getElementById('station-q').value='"
                        (name_val)
                        "';"
                        "document.getElementById('station-suggestions').innerHTML='';"
                    }
                {
                    span .suggestion-name { (result.name) }
                    " "
                    span .suggestion-crs { "(" (result.crs) ")" }
                }
            }
        }
    }
}
