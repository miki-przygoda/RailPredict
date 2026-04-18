//! Dashboard page — served at `/`, outside rate limiting.
//!
//! Shows system status, DB stats, and navigation links to all major features.

use axum::extract::State;
use maud::{Markup, html};

use crate::api::AppState;

use super::layout::base;

pub async fn dashboard_page(State(state): State<AppState>) -> Markup {
    let train_count = state.registry.len();

    let db_ok = sqlx::query("SELECT 1")
        .execute(&state.db)
        .await
        .is_ok();

    let station_count: Option<i64> = if db_ok {
        sqlx::query_scalar("SELECT COUNT(*) FROM stations")
            .fetch_one(&state.db)
            .await
            .ok()
    } else {
        None
    };

    let history_count: Option<i64> = if db_ok {
        sqlx::query_scalar("SELECT COUNT(*) FROM delay_history")
            .fetch_one(&state.db)
            .await
            .ok()
    } else {
        None
    };

    let (db_label, db_class) = if db_ok {
        ("Connected", "status-ok")
    } else {
        ("Unreachable", "status-error")
    };

    let stomp_label = if !state.registry.is_empty() {
        "Live data flowing"
    } else {
        "Awaiting Darwin feed"
    };

    base("Dashboard", html! {
        div .dashboard {
            h1 { "RailPredict" }
            p .subtitle { "High-efficiency UK Rail data engine — v" (env!("CARGO_PKG_VERSION")) }

            // ── System status ───────────────────────────────────────────────
            section .dashboard-section {
                h2 { "System Status" }
                div .status-grid {
                    div .status-card {
                        span .status-label { "Database" }
                        span .status-value .(db_class) { (db_label) }
                    }
                    div .status-card {
                        span .status-label { "Darwin STOMP" }
                        span .status-value { (stomp_label) }
                    }
                    div .status-card {
                        span .status-label { "Active trains" }
                        span .status-value { (train_count) }
                    }
                    div .status-card {
                        span .status-label { "Stations in DB" }
                        span .status-value {
                            @if let Some(n) = station_count { (n) } @else { "—" }
                        }
                    }
                    div .status-card {
                        span .status-label { "Delay records" }
                        span .status-value {
                            @if let Some(n) = history_count { (n) } @else { "—" }
                        }
                    }
                }
            }

            // ── Navigation ──────────────────────────────────────────────────
            section .dashboard-section {
                h2 { "Explore" }
                div .nav-grid {
                    a .nav-card href="/search" {
                        h3 { "Departure Board" }
                        p { "Search departures from any UK station by CRS code." }
                    }
                    a .nav-card href="/health" {
                        h3 { "Health Check" }
                        p { "Live DB connectivity probe — used by load balancers." }
                    }
                    a .nav-card href="/metrics" {
                        h3 { "Prometheus Metrics" }
                        p { "Ingestion counters, API latency histograms, circuit breaker state." }
                    }
                }
            }

            // ── Quick API reference ─────────────────────────────────────────
            section .dashboard-section {
                h2 { "API" }
                table .api-table {
                    thead {
                        tr {
                            th { "Method" } th { "Path" } th { "Returns" }
                        }
                    }
                    tbody {
                        tr { td { "GET" } td { code { "/stations/{crs}/departures" } } td { "JSON departure board" } }
                        tr { td { "GET" } td { code { "/trains/{rid}" }             } td { "JSON train summary" } }
                        tr { td { "GET" } td { code { "/trains/{rid}/live" }        } td { "SSE live updates" } }
                        tr { td { "GET" } td { code { "/health" }                   } td { "DB health probe" } }
                        tr { td { "GET" } td { code { "/metrics" }                  } td { "Prometheus scrape" } }
                    }
                }
            }
        }
    })
}
