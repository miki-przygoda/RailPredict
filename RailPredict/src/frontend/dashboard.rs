//! Overview cockpit — served at `/`, outside rate limiting.
//!
//! A live Signal-Terminal metrics cockpit: KPI strip (on-time %, avg delay,
//! prediction MAE, trains tracked) with sparklines, a live network-state panel,
//! a mini operator league, and a data-coverage footer. Re-scopable by the global
//! time-range picker; htmx range swaps return just the cockpit fragment.

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use maud::{Markup, PreEscaped, html};
use serde::Deserialize;

use crate::api::AppState;
use crate::db::{operators, overview};
use crate::frontend::charts::{self, Polarity};
use crate::frontend::components;

use super::layout::{base, NavPage};

#[derive(Debug, Deserialize)]
pub struct DashParams {
    #[serde(default)]
    pub range: Option<String>,
}

/// Normalise an incoming range string to one of the four supported values.
fn normalize_range(raw: Option<&str>) -> &'static str {
    match raw {
        Some("24h") => "24h",
        Some("30d") => "30d",
        Some("all") => "all",
        _ => "7d",
    }
}

fn range_to_hours(range: &str) -> i32 {
    match range {
        "24h" => 24,
        "30d" => 720,
        "all" => 876_000,
        _ => 168,
    }
}

pub async fn dashboard_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<DashParams>,
) -> Markup {
    let range = normalize_range(params.range.as_deref());
    let hours = range_to_hours(range);

    let db_ok = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();

    let (headline, series, league, coverage) = tokio::join!(
        overview::headline_metrics(&state.db, hours),
        overview::daily_series(&state.db, hours),
        operators::operator_league(&state.db, hours, 20, 8),
        overview::coverage_counts(&state.db),
    );
    let net = state.registry.network_summary(6).await;

    let body = render_cockpit(
        range,
        db_ok,
        headline.unwrap_or_default(),
        series.unwrap_or_default(),
        league.unwrap_or_default(),
        coverage.unwrap_or_default(),
        net,
    );

    if headers.contains_key("hx-request") {
        body
    } else {
        base("Dashboard", NavPage::Dashboard, body)
    }
}

/// Extract a metric column from the series as an f64 vec (dropping NULL days).
fn col(series: &[overview::DailyPoint], f: impl Fn(&overview::DailyPoint) -> Option<f64>) -> Vec<f64> {
    series.iter().filter_map(f).collect()
}

/// Delta between the last and first non-null points of a series (None if <2 points).
fn delta(vals: &[f64]) -> Option<f64> {
    match (vals.first(), vals.last()) {
        (Some(a), Some(b)) if vals.len() >= 2 => Some(b - a),
        _ => None,
    }
}

/// Format an optional metric value, or an em dash when absent.
fn fmt_opt(v: Option<f64>, prec: usize) -> String {
    match v {
        Some(x) => format!("{x:.*}", prec),
        None => "—".to_string(),
    }
}

/// Compact human count: 1_284 → "1.3k", 6_700_000 → "6.7M".
fn fmt_big(n: i64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1_000_000.0) }
    else if n >= 1_000 { format!("{:.1}k", n as f64 / 1_000.0) }
    else { n.to_string() }
}

#[allow(clippy::too_many_arguments)]
fn render_cockpit(
    range: &str,
    db_ok: bool,
    headline: overview::HeadlineMetrics,
    series: Vec<overview::DailyPoint>,
    league: Vec<operators::OperatorLeagueRow>,
    coverage: overview::CoverageCounts,
    net: crate::cache::train_registry::NetworkSummary,
) -> Markup {
    let ontime_spark = col(&series, |p| p.on_time_pct);
    let delay_spark = col(&series, |p| p.avg_delay_mins);
    let mae_spark = col(&series, |p| p.mae_mins);

    html! {
        div .dashboard {
            div .dash-header {
                div {
                    h1 .dash-title { "Network Overview" }
                    p .dash-sub { "Live UK rail punctuality & prediction accuracy" }
                }
                (components::time_range_picker("/", range))
            }

            div .kpi-strip {
                (charts::kpi_card("On-time", &fmt_opt(headline.on_time_pct, 1), Some("%"),
                    delta(&ontime_spark).map(|d| (d, Polarity::HigherIsBetter)), Some(&ontime_spark)))
                (charts::kpi_card("Avg delay", &fmt_opt(headline.avg_delay_mins, 1), Some("min"),
                    delta(&delay_spark).map(|d| (d, Polarity::LowerIsBetter)), Some(&delay_spark)))
                (charts::kpi_card("Prediction MAE", &fmt_opt(headline.mae_mins, 2), Some("min"),
                    delta(&mae_spark).map(|d| (d, Polarity::LowerIsBetter)), Some(&mae_spark)))
                (charts::kpi_card("Trains tracked", &net.tracked.to_string(), None, None, None))
            }

            div .cockpit-grid {
                section .panel {
                    div .panel-head {
                        h2 { "Live network" }
                        span .panel-meta { @if db_ok { "Darwin feed" } @else { "DB offline" } }
                    }
                    div .net-counts {
                        (net_stat("Tracked", net.tracked, "net-neutral"))
                        (net_stat("On time", net.on_time, "net-ok"))
                        (net_stat("Delayed", net.delayed, "net-warn"))
                        (net_stat("Cancelled", net.cancelled, "net-bad"))
                    }
                    @if net.worst.is_empty() {
                        p .panel-empty { "No delayed trains right now." }
                    } @else {
                        table .mini-table {
                            thead { tr { th { "Service" } th .num { "Delay" } } }
                            tbody {
                                @for w in &net.worst {
                                    tr {
                                        td {
                                            code { (w.origin_crs.as_deref().unwrap_or("???")) }
                                            span .arrow { "→" }
                                            code { (w.destination_crs.as_deref().unwrap_or("???")) }
                                        }
                                        td .num { span .delay-bad { "+" (w.delay_mins) "m" } }
                                    }
                                }
                            }
                        }
                    }
                }

                section .panel {
                    div .panel-head {
                        h2 { "Operator league" }
                        a .panel-link href="/operators" { "All operators →" }
                    }
                    @if league.is_empty() {
                        p .panel-empty { "No operator data yet. Run the GTFS ingest to populate operators." }
                    } @else {
                        table .mini-table .league-table {
                            thead { tr { th { "Operator" } th .num { "On-time" } th .num { "Avg" } } }
                            tbody {
                                @for row in &league {
                                    tr {
                                        td {
                                            span .op-chip style=(format!("background:{}", row.brand_color)) {}
                                            (row.name)
                                        }
                                        td .num { (fmt_opt(row.on_time_pct, 0)) "%" }
                                        td .num { (fmt_opt(row.avg_delay_mins, 1)) }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            section .dash-section {
                p .dash-section-label { "Explore" }
                div .dash-nav-grid {
                    (nav_card("/search", ICON_BOARD, "Departure Board", "Live departures from any UK station."))
                    (nav_card("/predictions", ICON_CHART, "Prediction analytics", "Accuracy, calibration & biggest errors."))
                    (nav_card("/operators", ICON_TROPHY, "Operators", "Per-operator punctuality league & drill-down."))
                    (nav_card("/demo", ICON_GEAR, "Dev Console", "Ingest data, probe the registry, simulate checkout."))
                }
            }

            div .coverage-strip {
                (coverage_chip("Stations", fmt_big(coverage.stations)))
                (coverage_chip("Real delay records", fmt_big(coverage.real_records)))
                (coverage_chip("Synthetic records", fmt_big(coverage.synthetic_records)))
            }
        }
    }
}

fn net_stat(label: &str, value: usize, cls: &str) -> Markup {
    html! {
        div .net-stat {
            span class=(format!("net-value {cls}")) { (value) }
            span .net-label { (label) }
        }
    }
}

fn coverage_chip(label: &str, value: String) -> Markup {
    html! {
        div .cov-chip {
            span .cov-value { (value) }
            span .cov-label { (label) }
        }
    }
}

fn nav_card(href: &str, icon: PreEscaped<&'static str>, title: &str, sub: &str) -> Markup {
    html! {
        a .dash-nav-card href=(href) {
            span .dnc-icon { (icon) }
            h3 { (title) }
            p { (sub) }
        }
    }
}

// 20×20 Lucide-style line icons (stroke=currentColor). Decorative → aria-hidden.
const ICON_BOARD: PreEscaped<&'static str> = PreEscaped(r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="14" rx="2"/><path d="M3 9h18M8 18v3M16 18v3"/></svg>"#);
const ICON_CHART: PreEscaped<&'static str> = PreEscaped(r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 3v18h18"/><path d="M7 14l3-4 3 2 4-6"/></svg>"#);
const ICON_TROPHY: PreEscaped<&'static str> = PreEscaped(r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 4h12v3a6 6 0 0 1-12 0V4z"/><path d="M6 6H4v1a3 3 0 0 0 3 3M18 6h2v1a3 3 0 0 1-3 3M9 17h6M10 21h4M12 13v4"/></svg>"#);
const ICON_GEAR: PreEscaped<&'static str> = PreEscaped(r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>"#);
