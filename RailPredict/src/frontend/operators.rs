//! Operator league (`/operators`) and per-operator drill-down (`/operators/:toc`).
//!
//! Both pages read the per-operator aggregates in `db::operators` (delay_history
//! JOINed to services by TOC) and re-scope by the global time-range picker. They
//! render branded empty states until `services.toc` is populated by the timetable
//! ingest — no operator is mis-attributed.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use maud::{Markup, html};
use serde::Deserialize;

use crate::api::AppState;
use crate::cache::location_names;
use crate::db::operators::{self, OperatorLeagueRow};
use crate::frontend::charts::{self, KpiTone};
use crate::frontend::components::{self, compact_count, normalize_range, range_label, range_to_hours};

use super::layout::{base, NavPage};

#[derive(Debug, Deserialize)]
pub struct RangeParams {
    #[serde(default)]
    pub range: Option<String>,
}

// ---------------------------------------------------------------------------
// Formatting + threshold helpers
// ---------------------------------------------------------------------------

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

/// Text-colour class for an on-time percentage (green ≥85, amber ≥75, else red).
fn ot_text(pct: f64) -> &'static str {
    if pct >= 85.0 { "val-ok" } else if pct >= 75.0 { "val-warn" } else { "val-bad" }
}

/// Bar-fill class for an on-time percentage.
fn ot_fill(pct: f64) -> &'static str {
    if pct >= 85.0 { "ot-good" } else if pct >= 75.0 { "ot-mid" } else { "ot-bad" }
}

/// Validate a TOC path segment: 2–4 ASCII alphanumerics, upper-cased.
fn clean_toc(raw: &str) -> Option<String> {
    let toc = raw.to_ascii_uppercase();
    if (2..=4).contains(&toc.len()) && toc.chars().all(|c| c.is_ascii_alphanumeric()) {
        Some(toc)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// /operators — league table
// ---------------------------------------------------------------------------

pub async fn operators_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<RangeParams>,
) -> Markup {
    let range = normalize_range(params.range.as_deref());
    let hours = range_to_hours(range);
    let league = operators::operator_league(&state.db, hours, 5, 30).await.unwrap_or_default();

    let body = render_league(range, &league);
    if headers.contains_key("hx-request") {
        body
    } else {
        base("Operators", NavPage::Operators, body)
    }
}

fn render_league(range: &str, league: &[OperatorLeagueRow]) -> Markup {
    html! {
        div .operators {
            div .dash-header {
                div {
                    h1 .dash-title { "Operators" }
                    p .dash-sub { "Punctuality league · ranked by on-time % · " (range_label(range)) }
                }
                div .dash-header-right { (components::time_range_picker("/operators", range)) }
            }

            @if league.is_empty() {
                div .panel { div .panel-body {
                    p .panel-empty {
                        "No operator data yet. Operators appear as services are captured from "
                        "the Darwin schedule feed and complete their journeys."
                    }
                } }
            } @else {
                div .league {
                    table {
                        thead { tr {
                            th .rank-h {}
                            th { "Operator" }
                            th .r { "On-time" }
                            th .r { "Avg delay" }
                            th .r { "Pred MAE" }
                            th .r { "Services" }
                        } }
                        tbody {
                            @for (i, row) in league.iter().enumerate() {
                                @let ot = row.on_time_pct.unwrap_or(0.0);
                                tr class=(if i == 0 { "top" } else { "" }) {
                                    td .rank { (i + 1) }
                                    td {
                                        div .op-cell {
                                            span .op-brand style=(format!("background:{}", row.brand_color)) {}
                                            a .op-link href=(format!("/operators/{}", row.toc)) { (row.name) }
                                            span .op-code { (row.toc) }
                                        }
                                    }
                                    td .r {
                                        div .ot-cell {
                                            span class=(format!("num {}", ot_text(ot))) { (fmt_pct(row.on_time_pct)) }
                                            span .ot-bar { span class=(format!("ot-fill {}", ot_fill(ot)))
                                                style=(format!("width:{}%", ot.clamp(0.0, 100.0).round() as i64)) {} }
                                        }
                                    }
                                    td .r.num { (fmt1(row.avg_delay_mins)) span .muted { " min" } }
                                    td .r.num.muted { (fmt1(row.mae_mins)) }
                                    td .r.num.muted { (compact_count(row.sample_count, 0)) }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// /operators/:toc — drill-down
// ---------------------------------------------------------------------------

pub async fn operator_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(toc): Path<String>,
    Query(params): Query<RangeParams>,
) -> Response {
    let Some(toc) = clean_toc(&toc) else {
        return (StatusCode::NOT_FOUND, "unknown operator").into_response();
    };
    let range = normalize_range(params.range.as_deref());
    let hours = range_to_hours(range);

    let (detail, series, dist, routes) = tokio::join!(
        operators::operator_detail(&state.db, &toc, hours),
        operators::operator_daily_series(&state.db, &toc, hours),
        operators::operator_delay_distribution(&state.db, &toc, hours),
        operators::operator_routes(&state.db, &toc, hours, 10),
    );

    let detail = detail.ok().flatten();
    let title = detail.as_ref().map(|d| d.name.clone()).unwrap_or_else(|| toc.clone());
    let body = render_operator(
        &toc,
        range,
        detail,
        series.unwrap_or_default(),
        dist.unwrap_or_default(),
        routes.unwrap_or_default(),
    );

    if headers.contains_key("hx-request") {
        body.into_response()
    } else {
        base(&title, NavPage::Operators, body).into_response()
    }
}

/// Map a delay-band sentinel (`lower_bound_mins`) to its display label.
fn band_label(lower: i32) -> &'static str {
    match lower {
        0 => "≤0",
        1 => "1–5",
        6 => "6–15",
        16 => "16–30",
        31 => "31–60",
        61 => "60+",
        _ => "?", // unreachable — BANDS is the exhaustive sentinel set
    }
}

fn render_operator(
    toc: &str,
    range: &str,
    detail: Option<operators::OperatorDetail>,
    series: Vec<operators::OperatorDailyPoint>,
    dist: Vec<operators::DelayBucket>,
    routes: Vec<operators::RouteRow>,
) -> Markup {
    let (name, brand) = detail
        .as_ref()
        .map(|d| (d.name.as_str(), d.brand_color.as_str()))
        .unwrap_or((toc, "#9aa7b4"));

    html! {
        div .operators {
            div .op-hero {
                span .op-hero-bar style=(format!("background:{brand}")) {}
                div {
                    h1 .op-hero-title { (name) }
                    p .dash-sub {
                        "ATOC code " (toc)
                        @if let Some(d) = &detail { " · " (compact_count(d.sample_count, 0)) " services scored" }
                        " · " (range_label(range))
                    }
                }
                div .dash-header-right { (components::time_range_picker(&format!("/operators/{toc}"), range)) }
            }

            @match &detail {
                None => {
                    div .panel { div .panel-body {
                        p .panel-empty { "No data for " (toc) " in this window." }
                    } }
                }
                Some(d) => {
                    @let ontime = d.on_time_pct.unwrap_or(0.0);
                    div .kpi-strip {
                        (charts::kpi_card("On-time", &fmt_pct(d.on_time_pct), None, None, None, None, KpiTone::Ok))
                        (charts::kpi_card("Avg delay", &fmt1(d.avg_delay_mins), Some("min"), None, None, None, KpiTone::Warn))
                        (charts::kpi_card("Prediction MAE", &fmt1(d.mae_mins), Some("min"), None, None, None, KpiTone::Info))
                        (charts::kpi_card("Services", &compact_count(d.sample_count, 0), None, None, None, None, KpiTone::Neutral))
                    }

                    div .cockpit-grid {
                        section .panel {
                            div .panel-head { h2 { "Punctuality trend" } span .panel-meta { "on-time %" } }
                            div .panel-body {
                                @let trend: Vec<f64> = series.iter().filter_map(|p| p.on_time_pct).collect();
                                @if trend.len() >= 2 {
                                    div class=(format!("acc-chart {}", ot_chart(ontime))) {
                                        (charts::area_spark(&trend, "op-trend"))
                                    }
                                    span .acc-cap {
                                        (fmt_pct(trend.first().copied())) " → " (fmt_pct(trend.last().copied()))
                                        " over " (range_label(range))
                                    }
                                } @else {
                                    p .panel-empty { "Not enough days in range for a trend." }
                                }
                            }
                        }
                        section .panel {
                            div .panel-head { h2 { "Delay distribution" } span .panel-meta { "services by band" } }
                            div .panel-body { (render_histogram(&dist)) }
                        }
                    }

                    section .panel {
                        div .panel-head { h2 { "Routes" } span .panel-meta { "by avg delay · worst first" } }
                        div .panel-body {
                            @if routes.is_empty() {
                                p .panel-empty { "No route data in range." }
                            } @else {
                                table .rt-table {
                                    thead { tr { th { "Route" } th .r { "On-time" } th .r { "Avg delay" } th .r { "Services" } } }
                                    tbody {
                                        @for r in &routes {
                                            @let ot = r.on_time_pct.unwrap_or(0.0);
                                            tr {
                                                td {
                                                    span { (location_names::name_or_code(&r.origin_crs)) }
                                                    span .arrow { "→" }
                                                    span { (location_names::name_or_code(&r.destination_crs)) }
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
}

/// Chart tone class for the trend area-spark, keyed on the operator's on-time %.
fn ot_chart(pct: f64) -> &'static str {
    if pct >= 85.0 { "chart-ok" } else if pct >= 75.0 { "chart-warn" } else { "chart-bad" }
}

/// Fixed 6-band histogram; bands absent from `dist` render as empty columns so the
/// axis stays stable. Heights scale to the busiest band.
fn render_histogram(dist: &[operators::DelayBucket]) -> Markup {
    const BANDS: [i32; 6] = [0, 1, 6, 16, 31, 61];
    let count = |lower: i32| dist.iter().find(|b| b.lower_bound_mins == lower).map(|b| b.sample_count).unwrap_or(0);
    let max = BANDS.iter().map(|&b| count(b)).max().unwrap_or(0).max(1);
    html! {
        @if dist.is_empty() {
            p .panel-empty { "No delay data in range." }
        } @else {
            div .histo {
                @for band in BANDS {
                    @let c = count(band);
                    @let h = (c as f64 / max as f64 * 100.0).round() as i64;
                    div .col {
                        div .bar style=(format!("height:{}%", h.max(2))) {}
                        div .lbl { (band_label(band)) }
                    }
                }
            }
        }
    }
}
