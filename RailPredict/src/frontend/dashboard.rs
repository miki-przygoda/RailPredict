//! Dashboard page — served at `/`, outside rate limiting.
//!
//! Shows system health, live ML accuracy stats, and quick navigation.
//! DB queries run in parallel via tokio::join!.

use axum::extract::State;
use maud::{Markup, html};

use crate::api::AppState;

use super::layout::base;

struct PredStats {
    count_24h: i64,
    median_ae:  f64,
    pct_5min:   f64,
    bias:       f64,
}

pub async fn dashboard_page(State(state): State<AppState>) -> Markup {
    let db_ok = sqlx::query("SELECT 1")
        .execute(&state.db)
        .await
        .is_ok();

    let (station_count, history_count, pred_stats) = tokio::join!(
        fetch_station_count(&state, db_ok),
        fetch_history_count(&state, db_ok),
        fetch_pred_stats(&state, db_ok),
    );

    let train_count = state.registry.len();
    let stomp_ok = !state.registry.is_empty();

    base("Dashboard", render(
        db_ok, stomp_ok, train_count,
        station_count, history_count, pred_stats,
    ))
}

async fn fetch_station_count(state: &AppState, db_ok: bool) -> Option<i64> {
    if !db_ok { return None; }
    sqlx::query_scalar("SELECT COUNT(*) FROM stations")
        .fetch_one(&state.db)
        .await
        .ok()
}

async fn fetch_history_count(state: &AppState, db_ok: bool) -> Option<i64> {
    if !db_ok { return None; }
    sqlx::query_scalar("SELECT COUNT(*) FROM delay_history")
        .fetch_one(&state.db)
        .await
        .ok()
}

async fn fetch_pred_stats(state: &AppState, db_ok: bool) -> Option<PredStats> {
    if !db_ok { return None; }
    sqlx::query_as::<_, (i64, f64, f64, f64)>(
        "SELECT COUNT(*),
                PERCENTILE_CONT(0.5) WITHIN GROUP
                    (ORDER BY ABS(predicted_delay_mins - delay_mins))::float8,
                (AVG(CASE WHEN ABS(predicted_delay_mins - delay_mins) <= 5
                          THEN 1.0 ELSE 0.0 END) * 100)::float8,
                AVG((predicted_delay_mins - delay_mins)::float8)
         FROM delay_history
         WHERE recorded_at > NOW() - INTERVAL '24 hours'
           AND predicted_delay_mins IS NOT NULL
           AND delay_mins BETWEEN -120 AND 600
           AND ABS(predicted_delay_mins - delay_mins) < 300",
    )
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .map(|(count, median, pct, bias)| PredStats {
        count_24h: count,
        median_ae: median,
        pct_5min: pct,
        bias,
    })
}

fn render(
    db_ok: bool,
    stomp_ok: bool,
    train_count: usize,
    station_count: Option<i64>,
    history_count: Option<i64>,
    pred_stats: Option<PredStats>,
) -> Markup {
    html! {
        div .dashboard {

            // ── Hero ──────────────────────────────────────────────────────────
            div .dash-hero {
                div .dash-hero-left {
                    div .dash-hero-brand {
                        span .dash-hero-dot {}
                        h1 { "RailPredict" }
                    }
                    p .dash-hero-sub {
                        "UK Rail data engine — v" (env!("CARGO_PKG_VERSION"))
                    }
                    div .dash-status-strip {
                        span .dash-status-pill .(if db_ok { "pill-ok" } else { "pill-error" }) {
                            span .dash-status-dot {}
                            @if db_ok { "DB Online" } @else { "DB Down" }
                        }
                        span .dash-status-pill .(if stomp_ok { "pill-ok" } else { "pill-warn" }) {
                            span .dash-status-dot {}
                            @if stomp_ok { "Darwin Live" } @else { "Darwin Awaiting" }
                        }
                    }
                }
                div .dash-hero-right {
                    a .dash-cta-btn href="/search" {
                        "Search departures"
                        span .dash-cta-arrow { "→" }
                    }
                    a .dash-secondary-btn href="/predictions" {
                        "ML analytics"
                    }
                }
            }

            // ── Metrics row ───────────────────────────────────────────────────
            div .dash-metrics {
                div .dash-metric {
                    span .dm-label { "Active trains" }
                    span .dm-value { (train_count) }
                }
                div .dash-metric {
                    span .dm-label { "Stations" }
                    span .dm-value {
                        @if let Some(n) = station_count { (fmt_big(n)) } @else { "—" }
                    }
                }
                div .dash-metric {
                    span .dm-label { "Delay records" }
                    span .dm-value {
                        @if let Some(n) = history_count { (fmt_big(n)) } @else { "—" }
                    }
                }
                @if let Some(p) = &pred_stats {
                    div .dash-metric.dm-ml {
                        span .dm-label { "Predictions (24 h)" }
                        span .dm-value { (fmt_big(p.count_24h)) }
                    }
                    div .dash-metric.dm-ml {
                        span .dm-label { "Median accuracy" }
                        span .dm-value {
                            (format!("{:.1}", p.median_ae))
                            span .dm-unit { " min" }
                        }
                    }
                    div .dash-metric.dm-ml {
                        span .dm-label { "Within ±5 min" }
                        span .dm-value
                            .(if p.pct_5min >= 70.0 { "dm-ok" }
                              else if p.pct_5min >= 50.0 { "dm-warn" }
                              else { "dm-bad" })
                        {
                            (format!("{:.0}", p.pct_5min))
                            span .dm-unit { "%" }
                        }
                    }
                    div .dash-metric.dm-ml {
                        span .dm-label { "Model bias" }
                        span .dm-value
                            .(if p.bias.abs() <= 1.0 { "dm-ok" }
                              else if p.bias.abs() <= 3.0 { "dm-warn" }
                              else { "dm-bad" })
                        {
                            @if p.bias >= 0.0 { "+" }
                            (format!("{:.1}", p.bias))
                            span .dm-unit { " min" }
                        }
                    }
                } @else {
                    div .dash-metric.dm-ml {
                        span .dm-label { "ML model" }
                        span .dm-value.dm-muted { "No data" }
                    }
                }
            }

            // ── Quick access ──────────────────────────────────────────────────
            section .dash-section {
                p .dash-section-label { "Quick access" }
                div .dash-nav-grid {
                    a .dash-nav-card href="/search" {
                        div .dnc-icon { "🚆" }
                        h3 { "Departure Board" }
                        p { "Live departures from any UK station. Name or CRS autocomplete." }
                    }
                    a .dash-nav-card href="/predictions" {
                        div .dnc-icon { "📊" }
                        h3 { "ML Analytics" }
                        p {
                            "24-hour accuracy stats, station leaderboard, and biggest recent errors."
                        }
                    }
                    a .dash-nav-card href="/demo" {
                        div .dnc-icon { "⚙" }
                        h3 { "Dev Console" }
                        p { "Ingest GTFS data, probe the train registry, and simulate checkout." }
                    }
                    a .dash-nav-card href="/report" target="_blank" {
                        div .dnc-icon { "📄" }
                        h3 { "Delay Report (7d)" }
                        p { "Generate a self-contained HTML snapshot of the last 7 days of delay and prediction data." }
                    }
                    a .dash-nav-card href="/report?days=1" target="_blank" {
                        div .dnc-icon { "⚡" }
                        h3 { "Live Report (24h)" }
                        p { "Same report scoped to the last 24 hours — most recent delay and accuracy data." }
                    }
                }
            }

            // ── API reference (collapsed) ─────────────────────────────────────
            details .dash-api-details {
                summary .dash-api-summary { "REST API reference" }
                table .api-table {
                    thead {
                        tr {
                            th { "Method" }
                            th { "Endpoint" }
                            th { "Returns" }
                        }
                    }
                    tbody {
                        tr {
                            td { span .method-badge { "GET" } }
                            td { code { "/stations/{crs}/departures" } }
                            td { "JSON departure board" }
                        }
                        tr {
                            td { span .method-badge { "GET" } }
                            td { code { "/trains/{rid}" } }
                            td { "JSON train summary" }
                        }
                        tr {
                            td { span .method-badge { "GET" } }
                            td { code { "/trains/{rid}/live" } }
                            td { "SSE live updates" }
                        }
                        tr {
                            td { span .method-badge { "GET" } }
                            td { code { "/journeys?from=XXX&to=YYY" } }
                            td { "JSON direct services" }
                        }
                        tr {
                            td { span .method-badge { "GET" } }
                            td { code { "/stations/search?q=..." } }
                            td { "JSON station autocomplete" }
                        }
                        tr {
                            td { span .method-badge { "GET" } }
                            td { code { "/health" } }
                            td { "DB health probe" }
                        }
                    }
                }
            }
        }
    }
}

fn fmt_big(n: i64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1_000_000.0) }
    else if n >= 1_000 { format!("{:.0}k", n as f64 / 1_000.0) }
    else { n.to_string() }
}
