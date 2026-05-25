//! Developer Console — `/demo`
//!
//! Two-panel layout:
//! - Left: Feature Lab — interactive tests for every major capability (health,
//!   autocomplete, departure board, registry probe, live event monitor).
//! - Right: Ticket Purchase Demo — a full end-to-end booking UI that shows
//!   what the Tier C purchase flow (backlog item 2.8) will look and feel like
//!   once the GBR Purchase API is wired. The "Confirm Purchase" step is
//!   simulated: it shows exactly what the real API call would contain.
//!
//! All routes here live in the infra router (no rate limiting).

use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    extract::{Form, Query, State},
    response::sse::{Event, KeepAlive, Sse},
};
use chrono::Utc;
use maud::{Markup, PreEscaped, html};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::{
    api::AppState,
    ingestion::gtfs::{IngestPhase, IngestStatus, run_ingest_with_watch, today_call_count},
    state_machine::TrainState,
    types::TrainId,
};

use super::{components::platform_chip, layout::base};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn pence_to_pounds(pence: i32) -> String {
    format!("£{:.2}", pence as f64 / 100.0)
}

/// Pseudo-unique reference built from wall-clock nanos — no uuid crate needed.
fn gen_idempotency_key() -> String {
    let secs = Utc::now().timestamp();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{secs:x}-{nanos:08x}")
}

fn gen_booking_ref() -> String {
    let micros = Utc::now().timestamp_micros();
    format!("RP{:06X}", (micros.unsigned_abs() % 0x00FF_FFFF) as u32)
}

// ---------------------------------------------------------------------------
// Query / form structs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RidQuery {
    pub rid: String,
}

#[derive(Deserialize)]
pub struct DemoJourneyQuery {
    pub from: String,
    pub to: String,
    pub date: Option<String>,
}

#[derive(Deserialize)]
pub struct CheckoutQuery {
    pub uid: String,
    pub from: String,
    pub to: String,
    pub dep: String, // "HH:MM"
}

#[derive(Deserialize)]
pub struct PurchaseForm {
    pub uid: String,
    pub from_crs: String,
    pub to_crs: String,
    pub dep_time: String,
    pub passengers: u8,
    pub idempotency_key: String,
    pub fare_pence: i32,
}

#[derive(Deserialize)]
pub struct IngestStartForm {
    #[serde(default)]
    pub url: String,
    /// When "1", skip the freshness guard and ingest even if today's data exists.
    #[serde(default)]
    pub force: String,
}

// ---------------------------------------------------------------------------
// Main page — GET /demo
// ---------------------------------------------------------------------------

pub async fn demo_page() -> Markup {
    base("Developer Console", html! {
        div .demo-console {

            // ── Page header ──────────────────────────────────────────────
            div .demo-header {
                div {
                    h1 { "Developer Console" }
                    p .demo-subtitle {
                        "Feature Lab  ·  Purchase Demo  ·  "
                        code { "v" (env!("CARGO_PKG_VERSION")) }
                    }
                }
                a .demo-back-link href="/" { "← Dashboard" }
            }

            // ── System Status — full-width above the grid ─────────────────
            div .demo-status-section {
                div .demo-section-title { "System Status" }
                div
                    hx-get="/ui/demo/status"
                    hx-trigger="load, every 5s"
                    hx-swap="innerHTML"
                {
                    p .demo-loading { "Polling…" }
                }
            }

            div .demo-grid {

                // ══════════════════════════════════════════════════════════
                // LEFT PANEL — Feature Lab
                // ══════════════════════════════════════════════════════════
                div .demo-col {

                    // 1. Station Autocomplete tester ────────────────────────
                    div .demo-section {
                        div .demo-section-title { "Station Autocomplete" }
                        p .demo-hint { "Type 2+ characters — fires /ui/stations/search." }
                        div .search-input-group {
                            input
                                type="text"
                                id="demo-ac-q"
                                placeholder="Station name…"
                                autocomplete="off"
                                hx-get="/ui/stations/search"
                                hx-trigger="input changed delay:300ms"
                                hx-target="#demo-ac-results"
                                hx-vals="js:{q: document.getElementById('demo-ac-q').value,\
                                             crs_input_id: 'demo-ac-crs',\
                                             q_input_id: 'demo-ac-q'}";
                            input type="hidden" id="demo-ac-crs" value="";
                            div #demo-ac-results {}
                        }
                        p .demo-crs-display {
                            "Selected CRS: "
                            code id="demo-ac-crs-display" { "—" }
                        }
                        script { (PreEscaped(r#"
document.addEventListener('htmx:afterSettle', function() {
    var crs = document.getElementById('demo-ac-crs');
    var disp = document.getElementById('demo-ac-crs-display');
    if (crs && disp && crs.value) disp.textContent = crs.value;
});
"#)) }
                    }

                    // 2. Departure Board tester ─────────────────────────────
                    div .demo-section {
                        div .demo-section-title { "Departure Board" }
                        p .demo-hint { "Enter a station name to load live timetable rows." }
                        form
                            hx-get="/ui/stations/departures"
                            hx-target="#demo-board-results"
                            hx-trigger="submit"
                            hx-swap="innerHTML"
                            .demo-inline-form
                        {
                            div .search-input-group {
                                input
                                    type="text"
                                    name="q"
                                    id="demo-board-q"
                                    placeholder="Station name…"
                                    autocomplete="off"
                                    hx-get="/ui/stations/search"
                                    hx-trigger="input changed delay:300ms"
                                    hx-target="#demo-board-sugg"
                                    hx-vals="js:{q: document.getElementById('demo-board-q').value,\
                                                 crs_input_id: 'demo-board-crs',\
                                                 q_input_id: 'demo-board-q'}";
                                input type="hidden" name="crs" id="demo-board-crs" value="";
                                div #demo-board-sugg {}
                            }
                            button type="submit" { "Load" }
                        }
                        div #demo-board-results {}
                    }

                    // 3. Registry Probe ─────────────────────────────────────
                    div .demo-section {
                        div .demo-section-title { "Registry Probe" }
                        p .demo-hint {
                            "Enter a RID to inspect the in-memory "
                            code { "TrainStatus" }
                            " entry."
                        }
                        form
                            hx-get="/ui/demo/registry"
                            hx-target="#demo-reg-result"
                            hx-trigger="submit"
                            hx-swap="innerHTML"
                            .demo-inline-form
                        {
                            input
                                type="text"
                                name="rid"
                                placeholder="RID (e.g. 202404170123456)"
                                .demo-mono-input;
                            button type="submit" { "Probe" }
                        }
                        div #demo-reg-result {}
                    }

                    // 4. Live Event Monitor ─────────────────────────────────
                    div .demo-section {
                        div .demo-section-title {
                            "Live Event Monitor"
                            span .demo-badge .badge-sse { "SSE" }
                        }
                        p .demo-hint {
                            "Streams all state-change events from the broadcast channel in real time."
                        }
                        div #demo-event-feed .demo-event-feed {
                            p .demo-loading id="event-feed-placeholder" {
                                "Connecting to /ui/demo/events…"
                            }
                        }
                        script { (PreEscaped(r#"
(function() {
    var feed  = document.getElementById('demo-event-feed');
    var es    = new EventSource('/ui/demo/events');
    var MAX_ROWS = 30;
    es.addEventListener('state-change', function(e) {
        var ph = document.getElementById('event-feed-placeholder');
        if (ph) ph.remove();
        var row = document.createElement('div');
        row.className = 'demo-event-row';
        row.innerHTML = e.data;
        feed.insertBefore(row, feed.firstChild);
        var rows = feed.querySelectorAll('.demo-event-row');
        if (rows.length > MAX_ROWS) rows[rows.length - 1].remove();
    });
    es.onerror = function() {
        var ph = document.getElementById('event-feed-placeholder');
        if (ph) { ph.className = 'demo-error'; ph.textContent = 'Stream disconnected — reload to reconnect.'; }
    };
})();
"#)) }
                    }

                    // 5. GTFS Data Ingest ───────────────────────────────────
                    div .demo-section {
                        div .demo-section-title {
                            "Data Ingest"
                            span .demo-badge .badge-tier-a { "Tier A" }
                        }
                        p .demo-hint {
                            "Seed the database with stations, services, and timetable data. "
                            "Leave URL blank to use the " code { "GTFS_URL" } " env var."
                        }
                        div
                            hx-get="/ui/demo/ingest/freshness"
                            hx-trigger="load"
                            hx-swap="outerHTML"
                        {
                            p .demo-loading { "Checking data…" }
                        }
                        form
                            hx-post="/ui/demo/ingest/start"
                            hx-target="#ingest-feedback"
                            hx-swap="innerHTML"
                            .demo-inline-form
                        {
                            input
                                type="url"
                                name="url"
                                id="ingest-url-input"
                                placeholder="GTFS URL (or blank for GTFS_URL env)"
                                .demo-mono-input;
                            button type="submit" .ingest-start-btn { "▶ Start" }
                        }
                        div #ingest-feedback {}
                        div #ingest-progress {
                            div .ingest-idle { "Idle — no ingest running." }
                        }
                        script { (PreEscaped(r#"
(function() {
    var es = new EventSource('/ui/demo/ingest/stream');
    es.addEventListener('ingest-update', function(e) {
        var panel = document.getElementById('ingest-progress');
        if (panel) panel.innerHTML = e.data;
    });
    es.onerror = function() {};
})();
"#)) }
                    }

                } // end left col

                // ══════════════════════════════════════════════════════════
                // RIGHT PANEL — Ticket Purchase Demo
                // ══════════════════════════════════════════════════════════
                div .demo-col {
                    div .demo-section .purchase-panel {

                        div .demo-section-title {
                            "Ticket Purchase"
                            span .demo-badge .badge-tier-c { "Tier C Demo" }
                        }
                        p .demo-hint {
                            "Simulates the end-to-end booking flow. "
                            "Step 3 shows the exact API call that item 2.8 would make."
                        }

                        // Step 1: Find journey ─────────────────────────────
                        div .purchase-step-header {
                            span .purchase-step-num { "1" }
                            " Find your journey"
                        }
                        form
                            hx-get="/ui/demo/journeys"
                            hx-target="#purchase-journey-results"
                            hx-trigger="submit"
                            hx-swap="innerHTML"
                            .purchase-search-form
                        {
                            div .purchase-from-to {
                                div .search-input-group {
                                    input
                                        type="text"
                                        name="from-q"
                                        id="purch-from-q"
                                        placeholder="From"
                                        autocomplete="off"
                                        hx-get="/ui/stations/search"
                                        hx-trigger="input changed delay:300ms"
                                        hx-target="#purch-from-sugg"
                                        hx-vals="js:{q: document.getElementById('purch-from-q').value,\
                                                     crs_input_id: 'purch-from-crs',\
                                                     q_input_id: 'purch-from-q'}";
                                    input type="hidden" name="from" id="purch-from-crs" value="";
                                    div #purch-from-sugg {}
                                }
                                span .purchase-arrow { "→" }
                                div .search-input-group {
                                    input
                                        type="text"
                                        name="to-q"
                                        id="purch-to-q"
                                        placeholder="To"
                                        autocomplete="off"
                                        hx-get="/ui/stations/search"
                                        hx-trigger="input changed delay:300ms"
                                        hx-target="#purch-to-sugg"
                                        hx-vals="js:{q: document.getElementById('purch-to-q').value,\
                                                     crs_input_id: 'purch-to-crs',\
                                                     q_input_id: 'purch-to-q'}";
                                    input type="hidden" name="to" id="purch-to-crs" value="";
                                    div #purch-to-sugg {}
                                }
                            }
                            div .purchase-date-row {
                                input
                                    type="date"
                                    name="date"
                                    .purchase-date-input;
                                button type="submit" .purchase-search-btn { "Search trains" }
                            }
                        }

                        // Step 2: Results ─────────────────────────────────
                        div #purchase-journey-results {}

                        // Step 3: Checkout (loaded by "Book" button on a result card) ──
                        div #purchase-checkout {}

                        // Step 4: Outcome (loaded by "Confirm & Pay") ──────
                        div #purchase-outcome {}

                    } // end purchase-panel
                } // end right col

            } // end demo-grid
        } // end demo-console
    })
}

// ---------------------------------------------------------------------------
// System Status fragment — GET /ui/demo/status  (polls every 5 s)
// ---------------------------------------------------------------------------

pub async fn demo_status_fragment(State(state): State<AppState>) -> Markup {
    let train_count = state.registry.len();

    let (db_ok, station_count, timetable_count, history_count) = {
        let ok = sqlx::query("SELECT 1")
            .execute(&state.db)
            .await
            .is_ok();

        let stations: Option<i64> = if ok {
            sqlx::query_scalar("SELECT COUNT(*) FROM stations")
                .fetch_one(&state.db).await.ok()
        } else { None };

        let timetable: Option<i64> = if ok {
            sqlx::query_scalar("SELECT COUNT(*) FROM timetable_calls")
                .fetch_one(&state.db).await.ok()
        } else { None };

        let history: Option<i64> = if ok {
            sqlx::query_scalar("SELECT COUNT(*) FROM delay_history")
                .fetch_one(&state.db).await.ok()
        } else { None };

        (ok, stations, timetable, history)
    };

    let (db_label, db_cls) = if db_ok { ("Connected", "status-ok") } else { ("Unreachable", "status-error") };
    let stomp_label = if train_count > 0 { "Live data flowing" } else { "Awaiting Darwin feed" };
    let stomp_cls   = if train_count > 0 { "status-ok" } else { "status-warn" };

    html! {
        div .demo-status-grid {
            div .demo-status-card {
                span .demo-status-label { "Database" }
                span .demo-status-value .(db_cls) { (db_label) }
            }
            div .demo-status-card {
                span .demo-status-label { "Darwin STOMP" }
                span .demo-status-value .(stomp_cls) { (stomp_label) }
            }
            div .demo-status-card {
                span .demo-status-label { "Registry trains" }
                span .demo-status-value { (train_count) }
            }
            div .demo-status-card {
                span .demo-status-label { "Stations" }
                span .demo-status-value {
                    @if let Some(n) = station_count { (n) } @else { "—" }
                }
            }
            div .demo-status-card {
                span .demo-status-label { "Timetable rows" }
                span .demo-status-value {
                    @if let Some(n) = timetable_count { (n) } @else { "—" }
                }
            }
            div .demo-status-card {
                span .demo-status-label { "Delay records" }
                span .demo-status-value {
                    @if let Some(n) = history_count { (n) } @else { "—" }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Registry probe fragment — GET /ui/demo/registry?rid=XXX
// ---------------------------------------------------------------------------

pub async fn demo_registry_fragment(
    Query(q): Query<RidQuery>,
    State(state): State<AppState>,
) -> Markup {
    let rid = q.rid.trim().to_string();

    let train_id = match TrainId::rid(&rid) {
        Ok(id) => id,
        Err(_) => return html! {
            p .demo-error { "Invalid RID format. Expected 15-digit numeric string." }
        },
    };

    let entry = state.registry.get(&train_id);

    match entry {
        None => html! {
            p .demo-error { "RID " code { (rid) } " not found in registry." }
            p .demo-hint { "The registry only holds trains that have appeared in a Darwin push message. With no live Darwin feed, try any RID from timetable_calls." }
        },
        Some(arc) => {
            let s = arc.read().await;
            html! {
                div .demo-registry-card {
                    div .demo-reg-row {
                        span .demo-reg-key { "RID" }
                        code .demo-reg-val { (rid) }
                    }
                    div .demo-reg-row {
                        span .demo-reg-key { "Origin CRS" }
                        code .demo-reg-val { (s.origin_crs.as_deref().unwrap_or("—")) }
                    }
                    div .demo-reg-row {
                        span .demo-reg-key { "Scheduled dep" }
                        code .demo-reg-val { (s.scheduled_departure.value.format("%H:%M UTC").to_string()) }
                    }
                    div .demo-reg-row {
                        span .demo-reg-key { "Est. departure" }
                        code .demo-reg-val {
                            @if let Some(est) = s.actual_estimated_departure.value {
                                (est.format("%H:%M UTC").to_string())
                            } @else { "—" }
                        }
                    }
                    div .demo-reg-row {
                        span .demo-reg-key { "Delay" }
                        code .demo-reg-val { (s.best_delay_mins().map(|d| format!("{d} min")).unwrap_or_else(|| "—".into())) }
                    }
                    div .demo-reg-row {
                        span .demo-reg-key { "Platform" }
                        code .demo-reg-val { (s.best_platform().unwrap_or("—")) }
                    }
                    div .demo-reg-row {
                        span .demo-reg-key { "Cancelled" }
                        code .demo-reg-val {
                            @match s.is_cancelled.value {
                                None        => "unknown",
                                Some(true)  => "yes",
                                Some(false) => "no",
                            }
                        }
                    }
                    div .demo-reg-row {
                        span .demo-reg-key { "Destination" }
                        code .demo-reg-val { (s.destination_crs.as_deref().unwrap_or("—")) }
                    }
                    div .demo-reg-row {
                        span .demo-reg-key { "Sched dep (stamp)" }
                        code .demo-reg-val { (s.scheduled_departure.last_updated.format("%H:%M:%S UTC").to_string()) }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Journey search with "Book" buttons — GET /ui/demo/journeys
// Used only by the purchase panel (separate from /ui/journeys).
// ---------------------------------------------------------------------------

pub async fn demo_journeys_fragment(
    Query(q): Query<DemoJourneyQuery>,
    State(state): State<AppState>,
) -> Markup {
    let from = q.from.trim().to_uppercase();
    let to   = q.to.trim().to_uppercase();

    if from == to || from.len() != 3 || to.len() != 3 {
        return html! { p .demo-error { "Enter valid 3-letter CRS codes for both stations." } };
    }

    let date = q.date.as_deref()
        .and_then(|s| s.parse::<chrono::NaiveDate>().ok())
        .unwrap_or_else(|| Utc::now().date_naive());

    let rows = sqlx::query_as::<_, (String, chrono::NaiveTime, Option<String>)>(
        "SELECT tc_from.uid, tc_from.scheduled_departure, tc_from.platform \
         FROM timetable_calls tc_from \
         JOIN timetable_calls tc_to \
             ON tc_to.uid            = tc_from.uid \
            AND tc_to.operating_date = tc_from.operating_date \
            AND tc_to.location_crs   = $2 \
            AND tc_to.call_order     > tc_from.call_order \
         WHERE tc_from.location_crs  = $1 \
           AND tc_from.operating_date = $3 \
         ORDER BY tc_from.scheduled_departure \
         LIMIT 8",
    )
    .bind(&from)
    .bind(&to)
    .bind(date)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    if rows.is_empty() {
        return html! {
            p .demo-error {
                "No direct services found from " (from) " to " (to)
                " on " (date.format("%d %b").to_string()) "."
            }
        };
    }

    html! {
        div .purchase-step-header .purchase-step-spaced {
            span .purchase-step-num { "2" }
            " Select a service"
        }
        div .purchase-results {
            @for (uid, dep_time, platform) in &rows {
                @let dep_str = dep_time.format("%H:%M").to_string();
                div .purchase-result-card {
                    div .purchase-result-left {
                        span .purchase-result-time { (dep_str) }
                        span .purchase-result-route { (from.clone()) " → " (to.clone()) }
                        (platform_chip(platform.as_deref(), true))
                    }
                    button
                        type="button"
                        .purchase-book-btn
                        hx-get="/ui/demo/checkout"
                        hx-target="#purchase-checkout"
                        hx-swap="innerHTML"
                        hx-vals={
                            "{\"uid\":\"" (uid) "\",\"from\":\"" (from) "\",\"to\":\"" (to) "\",\"dep\":\"" (dep_str) "\"}"
                        }
                    { "Book →" }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Checkout section — GET /ui/demo/checkout?uid=&from=&to=&dep=
// ---------------------------------------------------------------------------

pub async fn demo_checkout_fragment(
    Query(q): Query<CheckoutQuery>,
    State(state): State<AppState>,
) -> Markup {
    let uid  = q.uid.trim().to_string();
    let from = q.from.trim().to_uppercase();
    let to   = q.to.trim().to_uppercase();
    let dep  = q.dep.clone();

    let today = Utc::now().date_naive();

    // Resolve station names for display.
    let from_name: Option<String> = sqlx::query_scalar("SELECT name FROM stations WHERE crs = $1")
        .bind(&from)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

    let to_name: Option<String> = sqlx::query_scalar("SELECT name FROM stations WHERE crs = $1")
        .bind(&to)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);

    let fare: Option<crate::db::static_data::Fare> =
        crate::db::static_data::cheapest_fare(&state.db, &from, &to, today)
            .await
            .ok()
            .flatten();

    let fare_pence = fare.as_ref().map(|f| f.price_pence).unwrap_or(0);
    let idempotency_key = gen_idempotency_key();

    let from_display = from_name.as_deref().unwrap_or(&from);
    let to_display   = to_name.as_deref().unwrap_or(&to);

    html! {
        div .purchase-step-header .purchase-step-spaced-lg {
            span .purchase-step-num { "3" }
            " Review & pay"
        }
        div .purchase-checkout-card {

            // Service summary row
            div .checkout-service-row {
                div .checkout-station {
                    span .checkout-station-name { (from_display) }
                    span .checkout-station-crs  { (from.clone()) }
                }
                div .checkout-time-col {
                    span .checkout-dep-time { (dep) }
                    span .checkout-train-id { code { (uid.get(..8).unwrap_or(&uid)) "…" } }
                }
                div .checkout-station {
                    span .checkout-station-name { (to_display) }
                    span .checkout-station-crs  { (to.clone()) }
                }
            }

            // Fare section
            div .checkout-fare-section {
                @if fare_pence > 0 {
                    div .checkout-fare-row {
                        span { "1 × Adult Standard" }
                        span .checkout-fare-amount { (pence_to_pounds(fare_pence)) }
                    }
                    div .checkout-fare-row .checkout-fare-total {
                        span { "Total" }
                        span .checkout-fare-total-amount { (pence_to_pounds(fare_pence)) }
                    }
                } @else {
                    p .demo-hint { "Fare data not available for this route (run GTFS ingest first)." }
                }
            }

            // Passenger details (mock form — no real validation needed for demo)
            div .checkout-passenger-section {
                p .checkout-section-label { "Passenger details" }
                div .checkout-name-grid {
                    input type="text" placeholder="First name" .checkout-field;
                    input type="text" placeholder="Last name"  .checkout-field;
                }
                input type="email" placeholder="Email address" .checkout-field .checkout-field-full;
            }

            // Developer info bar (always visible on demo page)
            details .checkout-dev-info {
                summary { "Developer info" }
                div .checkout-dev-body {
                    div .dev-info-row {
                        span .dev-info-label { "Endpoint" }
                        code { "POST /journeys/" (uid) "/purchase" }
                    }
                    div .dev-info-row {
                        span .dev-info-label { "Idempotency key" }
                        code .dev-key { (idempotency_key) }
                    }
                    div .dev-info-row {
                        span .dev-info-label { "Circuit breaker" }
                        code { "PurchaseCircuitBreaker (threshold=1, cool_down=60s)" }
                    }
                    div .dev-info-row {
                        span .dev-info-label { "Backlog item" }
                        code { "Improvements.md §2.8" }
                    }
                    p .dev-info-note {
                        "The purchase API is not yet implemented. "
                        "Clicking below simulates the success path so you can see the full UI flow."
                    }
                }
            }

            // Submit (POST to /ui/demo/purchase)
            form
                hx-post="/ui/demo/purchase"
                hx-target="#purchase-outcome"
                hx-swap="innerHTML"
                hx-on--before-request="this.querySelector('button').disabled=true; this.querySelector('button').textContent='Processing…'"
            {
                input type="hidden" name="uid"              value=(uid);
                input type="hidden" name="from_crs"        value=(from);
                input type="hidden" name="to_crs"          value=(to);
                input type="hidden" name="dep_time"        value=(dep);
                input type="hidden" name="passengers"      value="1";
                input type="hidden" name="idempotency_key" value=(idempotency_key);
                input type="hidden" name="fare_pence"      value=(fare_pence);

                button type="submit" .purchase-confirm-btn {
                    "🔒  Confirm & Pay " (pence_to_pounds(fare_pence))
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Purchase outcome — POST /ui/demo/purchase
// ---------------------------------------------------------------------------

pub async fn demo_purchase_fragment(
    State(state): State<AppState>,
    Form(form): Form<PurchaseForm>,
) -> Markup {
    let booking_ref = gen_booking_ref();
    let confirmed_at = Utc::now().format("%H:%M:%S UTC, %d %b %Y").to_string();
    let total = pence_to_pounds(form.fare_pence * form.passengers as i32);

    let from_name: Option<String> =
        sqlx::query_scalar("SELECT name FROM stations WHERE crs = $1")
            .bind(&form.from_crs)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);

    let to_name: Option<String> =
        sqlx::query_scalar("SELECT name FROM stations WHERE crs = $1")
            .bind(&form.to_crs)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);

    let from_display = from_name.as_deref().unwrap_or(&form.from_crs);
    let to_display   = to_name.as_deref().unwrap_or(&form.to_crs);

    // What the real request body would look like.
    let mock_request_json = format!(
        r#"{{
  "journey_uid":        "{uid}",
  "origin_crs":         "{from}",
  "destination_crs":    "{to}",
  "departure_time":     "{dep}",
  "passenger_count":    {passengers},
  "fare_id":            "ANYTIME_SINGLE_{from}_{to}",
  "idempotency_key":    "{key}"
}}"#,
        uid        = form.uid,
        from       = form.from_crs,
        to         = form.to_crs,
        dep        = form.dep_time,
        passengers = form.passengers,
        key        = form.idempotency_key,
    );

    // What the real response body would look like.
    let mock_response_json = format!(
        r#"{{
  "booking_ref":        "{ref}",
  "journey_uid":        "{uid}",
  "status":             "confirmed",
  "total_price_pence":  {pence},
  "confirmed_at":       "{at}"
}}"#,
        ref   = booking_ref,
        uid   = form.uid,
        pence = form.fare_pence * form.passengers as i32,
        at    = Utc::now().to_rfc3339(),
    );

    html! {
        div .purchase-step-header .purchase-step-spaced-lg {
            span .purchase-step-num .step-done { "✓" }
            " Booking confirmed (simulated)"
        }
        div .purchase-outcome-card {

            // Confirmation banner
            div .purchase-confirmed-banner {
                div .purchase-confirmed-icon { "✓" }
                div {
                    p .purchase-confirmed-ref { "Booking ref: " strong { (booking_ref) } }
                    p .purchase-confirmed-detail {
                        (from_display) " → " (to_display)
                        " · " (form.dep_time)
                        " · " (form.passengers) " passenger"
                        @if form.passengers > 1 { "s" }
                    }
                    p .purchase-confirmed-total { "Total paid: " strong { (total) } }
                    p .purchase-confirmed-time { "Confirmed at " (confirmed_at) }
                }
            }

            // Simulated e-ticket strip
            div .ticket-strip {
                div .ticket-left {
                    span .ticket-from { (form.from_crs.clone()) }
                    span .ticket-arrow { "→" }
                    span .ticket-to   { (form.to_crs.clone()) }
                }
                div .ticket-right {
                    span .ticket-dep  { (form.dep_time.clone()) }
                    span .ticket-class { "Standard" }
                }
                div .ticket-barcode {
                    // Fake barcode — visual only
                    @for i in 0..22 {
                        @let w = if i % 3 == 0 { "3px" } else if i % 5 == 0 { "2px" } else { "1px" };
                        span .barcode-bar style={"width:" (w)} {}
                    }
                }
            }

            // Developer detail
            details .checkout-dev-info open {
                summary { "Developer info — what item 2.8 would execute" }
                div .checkout-dev-body {
                    p .dev-info-note {
                        strong { "This was a simulated purchase." }
                        " The purchase API (Improvements.md §2.8) is not yet implemented. "
                        "Below is the exact HTTP call that would be made once it is."
                    }
                    div .dev-info-row {
                        span .dev-info-label { "Endpoint" }
                        code { "POST /journeys/" (form.uid) "/purchase" }
                    }
                    div .dev-info-row {
                        span .dev-info-label { "Idempotency key" }
                        code .dev-key { (form.idempotency_key) }
                    }
                    div .dev-info-row {
                        span .dev-info-label { "Circuit breaker" }
                        code { "PurchaseCircuitBreaker · threshold=1 · cool_down=60s" }
                    }
                    p .dev-info-label .dev-info-code-header { "Request body (JSON):" }
                    pre .dev-code { (mock_request_json) }
                    p .dev-info-label .dev-info-code-header { "Response body (201 Created):" }
                    pre .dev-code { (mock_response_json) }
                    p .dev-info-note .dev-info-note-spaced {
                        "In production this call goes through "
                        code { "LiveGbrClient::purchase()" }
                        " → "
                        code { "PurchaseCircuitBreaker" }
                        " → GBR Retail API v1 "
                        code { "/bookings" }
                        ". The idempotency key is persisted to "
                        code { "purchase_attempts" }
                        " before the HTTP call leaves the server."
                    }
                }
            }

            button
                type="button"
                .purchase-reset-btn
                onclick="document.getElementById('purchase-checkout').innerHTML=''; \
                         document.getElementById('purchase-outcome').innerHTML=''; \
                         document.getElementById('purchase-journey-results').innerHTML='';"
            { "← Search again" }
        }
    }
}

// ---------------------------------------------------------------------------
// Ingest progress renderer — pure function, no I/O
// ---------------------------------------------------------------------------

fn render_ingest_progress(s: &IngestStatus) -> Markup {
    let phase_order: &[(&str, IngestPhase)] = &[
        ("Download",  IngestPhase::Downloading),
        ("Stations",  IngestPhase::Stations),
        ("Services",  IngestPhase::Services),
        ("Calls",     IngestPhase::TimetableCalls),
        ("Complete",  IngestPhase::Complete),
    ];

    // Compute which phase index is current (for done/active styling).
    let current_idx = phase_order.iter().position(|(_, p)| p == &s.phase).unwrap_or(
        if s.phase == IngestPhase::Failed { phase_order.len() } else { 0 }
    );

    let phase_label = match &s.phase {
        IngestPhase::Idle         => "Idle",
        IngestPhase::Downloading  => "Downloading…",
        IngestPhase::Parsing      => "Parsing…",
        IngestPhase::Stations     => "Upserting stations…",
        IngestPhase::Services     => "Upserting services…",
        IngestPhase::TimetableCalls => "Upserting timetable calls…",
        IngestPhase::Complete     => "Complete",
        IngestPhase::Failed       => "Failed",
    };

    let data_phase = format!("{:?}", s.phase).to_lowercase();

    html! {
        div .ingest-status-panel data-phase=(data_phase) {
            // Phase step indicator
            div .ingest-phases {
                @for (i, (label, _phase)) in phase_order.iter().enumerate() {
                    @let cls = if i < current_idx { "ingest-phase done" }
                               else if i == current_idx { "ingest-phase active" }
                               else { "ingest-phase" };
                    span .(cls) {
                        @if i < current_idx { "✓ " }
                        @else if i == current_idx && s.phase == IngestPhase::Failed { "✗ " }
                        (label)
                    }
                    @if i < phase_order.len() - 1 { span .ingest-phase-sep { "›" } }
                }
            }

            // Status label + elapsed
            div .ingest-status-row {
                span .ingest-phase-label {
                    @if s.phase == IngestPhase::Failed { span style="color:var(--red)" { (phase_label) } }
                    @else if s.phase == IngestPhase::Complete { span style="color:var(--green)" { (phase_label) } }
                    @else { (phase_label) }
                }
                @if let (Some(start), Some(end)) = (s.started_at, s.finished_at) {
                    span .ingest-elapsed {
                        (format!("{:.1}s", (end - start).num_milliseconds() as f64 / 1000.0))
                    }
                } @else if let Some(start) = s.started_at {
                    span .ingest-elapsed { (format!("{}s…", (Utc::now() - start).num_seconds())) }
                }
            }

            // Row counters
            @if s.phase != IngestPhase::Idle {
                div .ingest-counters {
                    div .ingest-counter {
                        span .ingest-counter-label { "Stations" }
                        strong .ingest-counter-value { (s.stations) }
                    }
                    div .ingest-counter {
                        span .ingest-counter-label { "Services" }
                        strong .ingest-counter-value { (s.services) }
                    }
                    div .ingest-counter {
                        span .ingest-counter-label { "Calls" }
                        strong .ingest-counter-value { (s.timetable_calls) }
                    }
                }
            }

            // Error message
            @if let Some(err) = &s.error {
                div .ingest-error { "Error: " (err) }
            }

            // Log feed (newest first)
            @if !s.log.is_empty() {
                div .ingest-log {
                    @for line in s.log.iter().rev().take(15) {
                        div .ingest-log-line { (line) }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Ingest data-freshness indicator — GET /ui/demo/ingest/freshness
// Shows how many timetable_calls rows exist for today; loaded once on page open.
// ---------------------------------------------------------------------------

pub async fn demo_ingest_freshness(State(state): State<AppState>) -> Markup {
    match today_call_count(&state.db).await {
        Ok(0) => html! {
            div .ingest-freshness .freshness-empty {
                span .freshness-icon { "⚠" }
                span { "No timetable data for today — run ingest before using the departure board." }
            }
        },
        Ok(n) => {
            let today = Utc::now().format("%d %b %Y").to_string();
            html! {
                div .ingest-freshness .freshness-ok {
                    span .freshness-icon { "✓" }
                    span {
                        strong { (n) }
                        " timetable calls loaded for " (today) " — data is fresh."
                    }
                }
            }
        }
        Err(_) => html! {
            div .ingest-freshness .freshness-empty {
                span .freshness-icon { "?" }
                span { "Could not query timetable data." }
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Ingest start — POST /ui/demo/ingest/start
// ---------------------------------------------------------------------------

pub async fn demo_ingest_start(
    State(state): State<AppState>,
    Form(form): Form<IngestStartForm>,
) -> Markup {
    let url = {
        let trimmed = form.url.trim().to_string();
        if trimmed.is_empty() {
            match std::env::var("GTFS_URL").ok() {
                Some(u) => u,
                None => return html! {
                    p .demo-error {
                        "No URL provided and " code { "GTFS_URL" } " env var is not set."
                    }
                },
            }
        } else {
            trimmed
        }
    };

    // Reject if already running.
    let phase = state.ingest_status.borrow().phase.clone();
    if !matches!(phase, IngestPhase::Idle | IngestPhase::Complete | IngestPhase::Failed) {
        return html! {
            p .demo-warn { "Ingest already in progress (" (format!("{phase:?}")) "). Wait for it to finish." }
        };
    }

    // Freshness guard: warn before overwriting data that already exists for today.
    let is_forced = form.force.trim() == "1";
    if !is_forced
        && let Ok(count) = today_call_count(&state.db).await
        && count > 0
    {
        let url_for_form = url.clone();
        return html! {
            div .ingest-guard {
                p .ingest-guard-msg {
                    "⚠  Today already has "
                    strong { (count) }
                    " timetable rows in the database — the app will use this data after a restart."
                    br;
                    "Re-ingesting is safe (uses " code { "ON CONFLICT DO NOTHING" }
                    ") but takes several minutes. Are you sure?"
                }
                form
                    hx-post="/ui/demo/ingest/start"
                    hx-target="#ingest-feedback"
                    hx-swap="innerHTML"
                    .demo-inline-form
                {
                    input type="hidden" name="url"   value=(url_for_form);
                    input type="hidden" name="force" value="1";
                    button type="submit" .ingest-force-btn { "Yes, re-ingest anyway" }
                }
            }
        };
    }

    // Reset to Downloading — fires the watch channel, SSE stream updates immediately.
    state.ingest_status.send_modify(|s| {
        *s = IngestStatus::default();
        s.phase = IngestPhase::Downloading;
        s.started_at = Some(Utc::now());
        s.log.push(format!("Starting: {url}"));
    });

    let db = state.db.clone();
    let tx = Arc::clone(&state.ingest_status);
    let url_clone = url.clone();

    tokio::spawn(async move {
        match run_ingest_with_watch(&db, &url_clone, &tx).await {
            Ok(_) => {}
            Err(e) => tx.send_modify(|s| {
                s.phase = IngestPhase::Failed;
                s.finished_at = Some(Utc::now());
                s.error = Some(e.to_string());
                s.log.push(format!("FAILED: {e}"));
            }),
        }
    });

    html! {
        p .demo-success { "▶ Ingest started — progress shown below." }
    }
}

// ---------------------------------------------------------------------------
// Ingest SSE stream — GET /ui/demo/ingest/stream
// Emits the full rendered progress panel on every watch-channel change.
// ---------------------------------------------------------------------------

pub async fn demo_ingest_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.ingest_status.subscribe();

    let stream = async_stream::stream! {
        // Always emit the current state immediately on connect.
        {
            let current = rx.borrow_and_update().clone();
            let html = render_ingest_progress(&current).into_string();
            yield Ok(Event::default().event("ingest-update").data(html));
        }

        while let Ok(()) = rx.changed().await {
            let status = rx.borrow_and_update().clone();
            let html = render_ingest_progress(&status).into_string();
            yield Ok(Event::default().event("ingest-update").data(html));
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ---------------------------------------------------------------------------
// SSE event monitor — GET /ui/demo/events
// Streams all state changes on the broadcast channel as plain HTML rows.
// Consumed by raw EventSource JS on the demo page (not htmx SSE extension).
// ---------------------------------------------------------------------------

pub async fn demo_events_sse(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.state_change_tx.subscribe();

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let state_cls = match event.new_state {
                        TrainState::Dormant   => "es-dormant",
                        TrainState::Monitored => "es-monitored",
                        TrainState::Active    => "es-active",
                        TrainState::Critical  => "es-critical",
                        TrainState::Terminal  => "es-terminal",
                    };
                    let ts = Utc::now().format("%H:%M:%S").to_string();
                    let html = format!(
                        "<code class=\"es-rid\">{rid}</code> \
                         <span class=\"es-arrow\">→</span> \
                         <span class=\"es-state {cls}\">{state:?}</span> \
                         <span class=\"es-ts\">{ts}</span>",
                        rid   = event.train_id,
                        cls   = state_cls,
                        state = event.new_state,
                        ts    = ts,
                    );
                    yield Ok(Event::default().event("state-change").data(html));
                }
                Err(RecvError::Lagged(n)) => {
                    yield Ok(Event::default()
                        .event("state-change")
                        .data(format!("<span class=\"es-lag\">⚠ skipped {n} events (lagged)</span>")));
                }
                Err(RecvError::Closed) => break,
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}
