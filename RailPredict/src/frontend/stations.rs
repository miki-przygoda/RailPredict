//! Station explorer — `/stations` (busiest-station index) + `/stations/:code`.
//!
//! The explorer shows reliability KPIs, a delay-by-hour×weekday heatmap, and the
//! busiest services for one origin, all from `db::stations` (real `delay_history`,
//! no backfill needed). `delay_history` keys origins by TIPLOC-style location code
//! (e.g. `WATRLMN`), so the index browses the busiest of those codes; a friendly
//! CRS→name mapping is the same deferred data work as the operator backfill.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use maud::{Markup, PreEscaped, html};
use serde::Deserialize;

use crate::api::AppState;
use crate::db::stations::{self, HeatCell};
use crate::frontend::charts::KpiTone;
use crate::frontend::components::{self, compact_count, normalize_range, range_label, range_to_hours};

use super::layout::{NavPage, base};

const NEUTRAL_BRAND: &str = "#9aa7b4";
const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

#[derive(Debug, Deserialize)]
pub struct RangeParams {
    #[serde(default)]
    pub range: Option<String>,
}

fn fmt1(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{x:.1}"),
        None => "—".to_string(),
    }
}

fn fmt_pct(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{x:.1}%"),
        None => "—".to_string(),
    }
}

fn ot_text(pct: f64) -> &'static str {
    if pct >= 85.0 { "val-ok" } else if pct >= 75.0 { "val-warn" } else { "val-bad" }
}

/// Heatmap cell colour by average delay (deep green = on time → red = severe).
fn heat_colour(v: f64) -> &'static str {
    if v < 1.5 { "#1c3a2e" } else if v < 3.0 { "#2f7d5b" } else if v < 6.0 { "#34d399" }
    else if v < 10.0 { "#f2c14e" } else if v < 16.0 { "#e08a2c" } else { "#f04545" }
}

/// Validate a location-code path segment: 2–8 ASCII alphanumerics, upper-cased
/// (CRS *and* the longer TIPLOC-style codes `delay_history` actually stores).
fn clean_code(raw: &str) -> Option<String> {
    let code = raw.to_ascii_uppercase();
    ((2..=8).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphanumeric())).then_some(code)
}

// ---------------------------------------------------------------------------
// /stations — busiest-station index
// ---------------------------------------------------------------------------

pub async fn stations_page(State(state): State<AppState>) -> Markup {
    let origins = stations::busiest_origins(&state.db, 168, 60).await.unwrap_or_default();
    let body = html! {
        div .stations {
            div .dash-header {
                div {
                    h1 .dash-title { "Stations" }
                    p .dash-sub { "Busiest stations by departures (last 7 days) — pick one to explore its delay heatmap." }
                }
            }
            div .st-filter-wrap {
                input # "st-filter" .st-search type="text" placeholder="Filter by code…" autocomplete="off";
            }
            @if origins.is_empty() {
                p .panel-empty { "No station data yet." }
            } @else {
                div .station-list {
                    @for o in &origins {
                        a .station-row href=(format!("/stations/{}", o.code)) data-code=(o.code) {
                            span .sr-code { (o.code) }
                            span .sr-stats {
                                span class=(format!("sr-ot {}", o.on_time_pct.map(ot_text).unwrap_or("muted"))) { (fmt_pct(o.on_time_pct)) }
                                span .muted { (fmt1(o.avg_delay_mins)) "m avg" }
                                span .muted { (compact_count(o.sample_count, 0)) }
                            }
                        }
                    }
                }
            }
        }
        script { (PreEscaped(r#"
(function(){var i=document.getElementById('st-filter');if(!i)return;
i.addEventListener('input',function(e){var q=e.target.value.trim().toUpperCase();
document.querySelectorAll('.station-row').forEach(function(r){
r.style.display=(q===''||r.dataset.code.indexOf(q)===0)?'':'none';});});})();
"#)) }
    };
    base("Stations", NavPage::Stations, body)
}

// ---------------------------------------------------------------------------
// /stations/:code — the explorer
// ---------------------------------------------------------------------------

pub async fn station_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    Query(params): Query<RangeParams>,
) -> Response {
    let Some(code) = clean_code(&code) else {
        return (StatusCode::NOT_FOUND, "unknown station").into_response();
    };
    let range = normalize_range(params.range.as_deref());
    let hours = range_to_hours(range);

    let (summary, heat, busiest) = tokio::join!(
        stations::station_summary(&state.db, &code, hours),
        stations::station_heatmap(&state.db, &code, hours),
        stations::station_busiest_services(&state.db, &code, hours, 12),
    );

    let body = render_station(
        &code,
        range,
        summary.ok().flatten(),
        heat.unwrap_or_default(),
        busiest.unwrap_or_default(),
    );
    if headers.contains_key("hx-request") {
        body.into_response()
    } else {
        base(&code, NavPage::Stations, body).into_response()
    }
}

fn render_station(
    code: &str,
    range: &str,
    summary: Option<stations::StationSummary>,
    heat: Vec<HeatCell>,
    busiest: Vec<stations::ServiceRow>,
) -> Markup {
    let has_data = summary.as_ref().map(|s| s.sample_count > 0).unwrap_or(false);
    html! {
        div .stations {
            div .dash-header {
                div {
                    h1 .dash-title { (code) }
                    p .dash-sub { "Departures from this station · " (range_label(range)) }
                }
                div .dash-header-right {
                    a .panel-link href="/stations" { "← All stations" }
                    (components::time_range_picker(&format!("/stations/{code}"), range))
                }
            }

            @if !has_data {
                div .panel { div .panel-body {
                    p .panel-empty { "No departures recorded from " (code) " in this window." }
                } }
            } @else {
                @let s = summary.as_ref().unwrap();
                div .kpi-strip {
                    (kpi("On-time", &fmt_pct(s.on_time_pct), Some("%"), "departures from here", KpiTone::Ok))
                    (kpi("Avg delay", &fmt1(s.avg_delay_mins), Some("min"), "across sampled departures", KpiTone::Warn))
                    (kpi("Prediction MAE", &fmt1(s.mae_mins), Some("min"), "predicted vs actual", KpiTone::Info))
                    (kpi("Departures", &compact_count(s.sample_count, 0), None, "in window", KpiTone::Neutral))
                }

                section .panel {
                    div .panel-head {
                        h2 { "Delay by hour & weekday" }
                        span .panel-meta { "avg delay · darker green = on time" }
                    }
                    div .panel-body { (heatmap(&heat)) }
                }

                section .panel {
                    div .panel-head {
                        h2 { "Busiest services" }
                        span .panel-meta { "by departures" }
                    }
                    div .panel-body {
                        @if busiest.is_empty() {
                            p .panel-empty { "No services in range." }
                        } @else {
                            table .sv-table {
                                thead { tr {
                                    th { "Service" } th { "Operator" }
                                    th .r { "On-time" } th .r { "Avg delay" } th .r { "Departures" }
                                } }
                                tbody {
                                    @for r in &busiest {
                                        @let ot = r.on_time_pct.unwrap_or(0.0);
                                        tr {
                                            td {
                                                code { (code) } " → "
                                                code { (r.destination_crs.as_deref().unwrap_or("—")) }
                                            }
                                            td {
                                                span .sv-op {
                                                    span .sv-brand style=(format!("background:{NEUTRAL_BRAND}")) {}
                                                    (r.toc.as_deref().unwrap_or("—"))
                                                }
                                            }
                                            td class=(format!("r {}", ot_text(ot))) { (fmt_pct(r.on_time_pct)) }
                                            td .r { (fmt1(r.avg_delay_mins)) " min" }
                                            td .r.muted { (compact_count(r.sample_count, 0)) }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn kpi(label: &str, value: &str, unit: Option<&str>, caption: &str, tone: KpiTone) -> Markup {
    crate::frontend::charts::kpi_card(label, value, unit, Some(caption), None, None, tone)
}

/// Dense 7×24 heatmap from the sparse non-empty cells.
fn heatmap(cells: &[HeatCell]) -> Markup {
    let map: HashMap<(i16, i16), &HeatCell> =
        cells.iter().map(|c| ((c.weekday, c.departure_hour), c)).collect();
    html! {
        div .heatwrap {
            div .heat {
                @for (d, dname) in DAYS.iter().enumerate() {
                    div .hrow-label { (dname) }
                    @for h in 0..24_i16 {
                        @match map.get(&(d as i16, h)) {
                            Some(c) => {
                                @let v = c.avg_delay_mins.unwrap_or(0.0);
                                div .hcell
                                    style=(format!("background:{}", heat_colour(v)))
                                    title=(format!("{} {:02}:00 · {:+.1}m · {} obs", dname, h, v, c.sample_count)) {}
                            }
                            None => { div .hcell.hcell-empty {} }
                        }
                    }
                }
            }
            div .hx-axis {
                div .sp {}
                @for h in (0..24).step_by(3) { div .hx { (format!("{:02}", h)) } }
            }
            div .heat-legend {
                "on time"
                span .sw style="background:#1c3a2e" {}
                span .sw style="background:#2f7d5b" {}
                span .sw style="background:#34d399" {}
                span .sw style="background:#f2c14e" {}
                span .sw style="background:#e08a2c" {}
                span .sw style="background:#f04545" {}
                "severe"
            }
        }
    }
}
