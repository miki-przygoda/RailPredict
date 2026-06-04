//! Overview cockpit — served at `/`, outside rate limiting.
//!
//! A live Signal-Terminal metrics cockpit: KPI strip (on-time %, avg delay,
//! prediction MAE, trains tracked) with area sparklines, a live network-state
//! panel, a prediction-accuracy panel, and a data-coverage footer. Re-scopable by
//! the global time-range picker; htmx range swaps return just the cockpit fragment.

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use maud::{Markup, PreEscaped, html};
use serde::Deserialize;

use crate::api::AppState;
use crate::db::{analytics, overview, predictions};
use crate::frontend::charts::{self, KpiTone, Polarity};
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

/// Human label for the active range, used in panel captions.
fn range_label(range: &str) -> &'static str {
    match range {
        "24h" => "last 24 h",
        "30d" => "last 30 days",
        "all" => "all time",
        _ => "last 7 days",
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

    let (headline, series, accuracy, acc_series, coverage) = tokio::join!(
        overview::headline_metrics(&state.db, hours),
        overview::daily_series(&state.db, hours),
        predictions::accuracy_summary(&state.db, hours),
        analytics::accuracy_over_time(&state.db, hours),
        overview::coverage_counts(&state.db),
    );
    let net = state.registry.network_summary(6).await;

    let body = render_cockpit(
        range,
        db_ok,
        headline.unwrap_or_default(),
        series.unwrap_or_default(),
        accuracy.ok(),
        acc_series.unwrap_or_default(),
        coverage.unwrap_or_default(),
        net,
    );

    if headers.contains_key("hx-request") {
        body
    } else {
        base("Overview", NavPage::Dashboard, body)
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

use super::components::compact_count;

#[allow(clippy::too_many_arguments)]
fn render_cockpit(
    range: &str,
    db_ok: bool,
    headline: overview::HeadlineMetrics,
    series: Vec<overview::DailyPoint>,
    accuracy: Option<predictions::AccuracySummary>,
    acc_series: Vec<analytics::AccuracyPoint>,
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
                div .dash-header-right {
                    @if net.tracked > 0 {
                        span .live-pill { span .live-dot {} "Live · " (net.tracked) " trains" }
                    } @else {
                        span .live-pill.idle { span .live-dot {} "Awaiting feed" }
                    }
                    (components::time_range_picker("/", range))
                }
            }

            div .kpi-strip {
                (charts::kpi_card("On-time", &fmt_opt(headline.on_time_pct, 1), Some("%"),
                    Some("live movement sample"),
                    delta(&ontime_spark).map(|d| (d, Polarity::HigherIsBetter)), Some(&ontime_spark), KpiTone::Ok))
                (charts::kpi_card("Avg delay", &fmt_opt(headline.avg_delay_mins, 1), Some("min"),
                    Some("across sampled movements"),
                    delta(&delay_spark).map(|d| (d, Polarity::LowerIsBetter)), Some(&delay_spark), KpiTone::Warn))
                (charts::kpi_card("Prediction MAE", &fmt_opt(headline.mae_mins, 2), Some("min"),
                    Some("predicted vs actual error"),
                    delta(&mae_spark).map(|d| (d, Polarity::LowerIsBetter)), Some(&mae_spark), KpiTone::Info))
                (charts::kpi_card("Trains tracked", &net.tracked.to_string(), None,
                    Some("on the Darwin feed"),
                    None, None, KpiTone::Neutral))
            }

            div .cockpit-grid {
                section .panel {
                    div .panel-head {
                        h2 { "Live network" }
                        span .panel-meta { @if db_ok { "● Darwin feed" } @else { "DB offline" } }
                    }
                    div .panel-body {
                        div .net-counts {
                            (net_stat("Tracked", net.tracked, "net-neutral"))
                            (net_stat("On time", net.on_time, "net-ok"))
                            (net_stat("Delayed", net.delayed, "net-warn"))
                            (net_stat("Cancelled", net.cancelled, "net-bad"))
                        }
                        @if net.worst.is_empty() {
                            p .panel-empty { "No delayed trains right now." }
                        } @else {
                            @let max_delay = net.worst.iter().map(|w| w.delay_mins).max().unwrap_or(1).max(1);
                            ul .worst-list {
                                @for w in &net.worst {
                                    @let pct = (w.delay_mins as f64 / max_delay as f64 * 100.0).round() as i64;
                                    li .worst-row {
                                        span .worst-route {
                                            code { (w.origin_crs.as_deref().unwrap_or("???")) }
                                            span .arrow { "→" }
                                            code { (w.destination_crs.as_deref().unwrap_or("???")) }
                                        }
                                        span .worst-bar { span .worst-fill style=(format!("width:{pct}%")) {} }
                                        span .worst-delay { "+" (w.delay_mins) "m" }
                                    }
                                }
                            }
                        }
                    }
                }

                section .panel {
                    div .panel-head {
                        h2 { "Prediction accuracy" }
                        a .panel-link href="/predictions" { "Full analytics →" }
                    }
                    div .panel-body {
                        @match accuracy.filter(|a| a.finalised_count > 0) {
                            Some(acc) => {
                                @let within5 = acc.within_5_count as f64 / acc.finalised_count as f64 * 100.0;
                                @let mae_series: Vec<f64> = acc_series.iter().filter_map(|p| p.mae_mins).collect();
                                div .acc-top {
                                    div .acc-headline {
                                        span .acc-big { (within5.round() as i64) span .acc-pct { "%" } }
                                        span .acc-cap { "within ±5 min" }
                                    }
                                    div .acc-detail {
                                        p .acc-sub {
                                            (compact_count(acc.finalised_count, 0)) " scored · MAE "
                                            (fmt_opt(acc.mean_abs_error_mins, 1)) " min"
                                        }
                                        @if mae_series.len() >= 2 {
                                            div .acc-chart { (charts::area_spark(&mae_series, "acc-trend")) }
                                            span .acc-cap { "MAE trend · " (range_label(range)) }
                                        }
                                    }
                                }
                            }
                            None => {
                                p .panel-empty { "No scored predictions in this window yet." }
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
                    (nav_card("/explore", ICON_EXPLORE, "Query Explorer", "Build your own delay & prediction queries."))
                }
            }

            div .coverage-strip {
                (coverage_chip("Stations", compact_count(coverage.stations, 1)))
                (coverage_chip("Delay records", compact_count(coverage.real_records, 1)))
                (coverage_chip("Predictions scored", compact_count(coverage.predictions_scored, 1)))
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
const ICON_EXPLORE: PreEscaped<&'static str> = PreEscaped(r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/></svg>"#);
