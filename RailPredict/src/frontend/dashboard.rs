//! Dashboard page — served at `/`, outside rate limiting.

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

    let stomp_ok = !state.registry.is_empty();

    base("Dashboard", html! {
        div .dashboard {
            // ── Hero ────────────────────────────────────────────────────────
            div .dashboard-hero {
                h1 { "RailPredict" }
                p .subtitle { "UK Rail data engine — v" (env!("CARGO_PKG_VERSION")) }
            }

            // ── Live stats ──────────────────────────────────────────────────
            div .stat-row {
                div .stat-card {
                    span .stat-label { "Database" }
                    span .stat-value .(if db_ok { "ok" } else { "error" }) {
                        @if db_ok { "Online" } @else { "Down" }
                    }
                }
                div .stat-card {
                    span .stat-label { "Darwin STOMP" }
                    span .stat-value .(if stomp_ok { "ok" } else { "warn" }) {
                        @if stomp_ok { "Live" } @else { "Awaiting" }
                    }
                }
                div .stat-card {
                    span .stat-label { "Active trains" }
                    span .stat-value { (train_count) }
                }
                div .stat-card {
                    span .stat-label { "Stations" }
                    span .stat-value {
                        @if let Some(n) = station_count { (n) } @else { "—" }
                    }
                }
                div .stat-card {
                    span .stat-label { "Delay records" }
                    span .stat-value {
                        @if let Some(n) = history_count { (n) } @else { "—" }
                    }
                }
            }

            // ── Explore ─────────────────────────────────────────────────────
            section .dashboard-section {
                p .section-header { "Explore" }
                div .nav-grid {
                    a .nav-card href="/search" {
                        h3 { "Departure Board" }
                        p { "Live departures from any UK station. Type a name, pick from autocomplete." }
                    }
                    a .nav-card href="/demo" {
                        h3 { "Developer Console" }
                        p { "Ingest GTFS timetable data, probe the registry, and simulate a ticket purchase." }
                    }
                    a .nav-card href="/metrics" {
                        h3 { "Prometheus Metrics" }
                        p { "Ingestion counters, API latency histograms, circuit breaker state." }
                    }
                    a .nav-card href="/health" {
                        h3 { "Health Check" }
                        p { "DB connectivity probe used by load balancers." }
                    }
                }
            }

            // ── API reference ────────────────────────────────────────────────
            section .dashboard-section {
                p .section-header { "REST API" }
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
    })
}
