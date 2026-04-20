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
                div .search-tabs {
                    button .tab-btn.active type="button"
                        onclick="showTab('departures', event)" { "Departures" }
                    button .tab-btn type="button"
                        onclick="showTab('journeys', event)" { "Journey" }
                }
                // Departures tab
                div id="tab-departures" .tab-pane {
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
                }
                // Journey tab
                div id="tab-journeys" .tab-pane.hidden {
                    form
                        hx-get="/ui/journeys"
                        hx-target="#results"
                        hx-trigger="submit"
                        hx-swap="innerHTML transition:true"
                    {
                        div style="display:flex;flex-direction:column;gap:0.75rem;margin-bottom:1rem;" {
                            div style="position:relative" {
                                input
                                    type="text"
                                    name="from-q"
                                    id="journey-from-q"
                                    placeholder="From station"
                                    autocomplete="off"
                                    hx-get="/ui/stations/search"
                                    hx-trigger="input changed delay:300ms"
                                    hx-target="#journey-from-suggestions"
                                    hx-vals="js:{q: document.getElementById('journey-from-q').value, crs_input_id: 'journey-from-crs', q_input_id: 'journey-from-q'}";
                                input
                                    type="hidden"
                                    name="from"
                                    id="journey-from-crs"
                                    value="";
                                div id="journey-from-suggestions" {}
                            }
                            div style="position:relative" {
                                input
                                    type="text"
                                    name="to-q"
                                    id="journey-to-q"
                                    placeholder="To station"
                                    autocomplete="off"
                                    hx-get="/ui/stations/search"
                                    hx-trigger="input changed delay:300ms"
                                    hx-target="#journey-to-suggestions"
                                    hx-vals="js:{q: document.getElementById('journey-to-q').value, crs_input_id: 'journey-to-crs', q_input_id: 'journey-to-q'}";
                                input
                                    type="hidden"
                                    name="to"
                                    id="journey-to-crs"
                                    value="";
                                div id="journey-to-suggestions" {}
                            }
                            button type="submit" { "Find journey" }
                        }
                    }
                }
                div #results {}
                script {
                    (maud::PreEscaped(r#"
function showTab(name, event) {
    document.querySelectorAll('.tab-pane').forEach(function(el) { el.classList.add('hidden'); });
    document.getElementById('tab-' + name).classList.remove('hidden');
    document.querySelectorAll('.tab-btn').forEach(function(el) { el.classList.remove('active'); });
    event.target.classList.add('active');
}
"#))
                }
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

/// `GET /ui/stations/search?q=<term>[&crs_input_id=...&q_input_id=...]`
///
/// Returns an HTML `<ul>` of matching station suggestions for the autocomplete
/// dropdown. Each `<li>` sets the hidden CRS input and the visible text input
/// on click, then clears the suggestions div.
///
/// `crs_input_id` and `q_input_id` allow the journey form to route selections
/// into the correct from/to hidden inputs. Defaults to `crs-hidden`/`station-q`
/// (the departures form IDs) when not provided.
pub async fn station_suggestions_fragment(
    Query(params): Query<StationSearchQuery>,
    State(state): State<AppState>,
) -> Markup {
    let q = params.q.trim().to_string();
    if q.len() < 2 {
        return html! {};
    }

    let crs_id = params
        .crs_input_id
        .as_deref()
        .unwrap_or("crs-hidden")
        .to_string();
    let q_id = params
        .q_input_id
        .as_deref()
        .unwrap_or("station-q")
        .to_string();

    // Derive the suggestions container ID to clear on selection.
    // Convention: suggestions div id = q_input_id with "-q" → "-suggestions",
    // or fall back to "station-suggestions".
    let suggestions_id = if q_id == "station-q" {
        "station-suggestions".to_string()
    } else {
        q_id.replace("-q", "-suggestions")
    };

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
                        "document.getElementById('" (crs_id) "').value='"
                        (crs_val)
                        "';"
                        "document.getElementById('" (q_id) "').value='"
                        (name_val)
                        "';"
                        "document.getElementById('" (suggestions_id) "').innerHTML='';"
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

// ---------------------------------------------------------------------------
// Item 5.1 — Journey search fragment
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct JourneyQuery {
    pub from: String,
    pub to: String,
    pub date: Option<String>,
}

/// `GET /ui/journeys?from=XXX&to=YYY[&date=YYYY-MM-DD]`
///
/// HTML fragment: direct services calling both `from` and `to` in order.
/// Used as an htmx swap target from the journey search form.
pub async fn journeys_fragment(
    Query(q): Query<JourneyQuery>,
    State(state): State<AppState>,
) -> Markup {
    let from = q.from.trim().to_uppercase();
    let to = q.to.trim().to_uppercase();

    if from == to || from.len() != 3 || to.len() != 3 {
        return html! {
            p .no-results { "Please enter valid origin and destination station codes." }
        };
    }

    let date = q
        .date
        .as_deref()
        .and_then(|s| s.parse::<chrono::NaiveDate>().ok())
        .unwrap_or_else(|| chrono::Utc::now().date_naive());

    let rows = sqlx::query_as::<_, (String, chrono::NaiveTime, Option<String>)>(
        "SELECT tc_from.uid, \
                tc_from.scheduled_departure, \
                tc_from.platform \
         FROM timetable_calls tc_from \
         JOIN timetable_calls tc_to \
             ON tc_to.uid            = tc_from.uid \
            AND tc_to.operating_date = tc_from.operating_date \
            AND tc_to.location_crs   = $2 \
            AND tc_to.call_order     > tc_from.call_order \
         WHERE tc_from.location_crs  = $1 \
           AND tc_from.operating_date = $3 \
         ORDER BY tc_from.scheduled_departure",
    )
    .bind(&from)
    .bind(&to)
    .bind(date)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let entries: Vec<crate::api::types::DepartureBoardEntry> = rows
        .into_iter()
        .map(|(uid, dep_time, platform)| {
            let scheduled_dt = chrono::NaiveDateTime::new(date, dep_time).and_utc();
            crate::api::types::DepartureBoardEntry {
                rid: uid.trim().to_string(),
                scheduled_departure: scheduled_dt.to_rfc3339(),
                estimated_departure: None,
                delay_mins: None,
                platform,
                is_cancelled: None,
                last_updated_secs_ago: None,
                destination_name: Some(to.clone()),
            }
        })
        .collect();

    if entries.is_empty() {
        return html! {
            p .no-results { "No direct services found from " (from) " to " (to) "." }
        };
    }

    departure_board_fragment(&format!("{from} \u{2192} {to}"), &entries)
}
