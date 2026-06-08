//! Diagnostics console — `/dev`
//!
//! Two-panel layout:
//! - Left: internal diagnostics — interactive tests for every major capability (health,
//!   autocomplete, departure board, registry probe, live event monitor).
//! - Right: Ticketing stub — live purchase is not wired in this build; the circuit-breaker,
//!   idempotency layer, and purchase ledger exist but the GBR Retail write endpoint is
//!   pending commercial access.
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

use super::layout::{base, NavPage};

// ---------------------------------------------------------------------------
// Query / form structs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RidQuery {
    pub rid: String,
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
// Main page — GET /dev
// ---------------------------------------------------------------------------

pub async fn dev_page() -> Markup {
    base("Diagnostics", NavPage::DevConsole, html! {
        div .demo-console {

            // ── Page header ──────────────────────────────────────────────
            div .demo-header {
                div {
                    h1 { "Diagnostics" }
                    p .demo-subtitle {
                        "Internal diagnostics  ·  "
                        code { "v" (env!("CARGO_PKG_VERSION")) }
                    }
                }
                a .demo-back-link href="/" { "← Overview" }
            }

            // ── System Status — full-width above the grid ─────────────────
            div .demo-status-section {
                div .demo-section-title { "System Status" }
                div
                    hx-get="/ui/dev/status"
                    hx-trigger="load, every 5s"
                    hx-swap="innerHTML"
                {
                    p .demo-loading { "Polling…" }
                }
            }

            // ── Predicted vs Actual — full-width, the headline ML view ────
            div .demo-status-section {
                div .demo-section-title {
                    "Predicted vs Actual"
                }
                p .demo-hint {
                    "Per-train predictions captured at first sighting, and the actual "
                    "delay outcome recorded when each train deactivates. Rolling 24h "
                    "accuracy summary plus the 30 most recent entries."
                }
                div
                    hx-get="/ui/dev/predictions"
                    hx-trigger="load, every 10s"
                    hx-swap="innerHTML"
                {
                    p .demo-loading { "Loading predictions…" }
                }
            }

            div .demo-grid {

                // ══════════════════════════════════════════════════════════
                // LEFT PANEL — diagnostics
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
                            hx-get="/ui/dev/registry"
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
                                "Connecting to /ui/dev/events…"
                            }
                        }
                        script { (PreEscaped(r#"
(function() {
    var feed  = document.getElementById('demo-event-feed');
    var es    = new EventSource('/ui/dev/events');
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
                        }
                        p .demo-hint {
                            "Seed the database with stations, services, and timetable data. "
                            "Leave URL blank to use the " code { "GTFS_URL" } " env var."
                        }
                        div
                            hx-get="/ui/dev/ingest/freshness"
                            hx-trigger="load"
                            hx-swap="outerHTML"
                        {
                            p .demo-loading { "Checking data…" }
                        }
                        form
                            hx-post="/ui/dev/ingest/start"
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
    var es = new EventSource('/ui/dev/ingest/stream');
    es.addEventListener('ingest-update', function(e) {
        var panel = document.getElementById('ingest-progress');
        if (panel) panel.innerHTML = e.data;
    });
    es.onerror = function() {};
})();
"#)) }
                    }

                } // end left col

                // ── RIGHT PANEL — Ticketing (stub) ────────────────────────
                div .demo-col {
                    div .demo-section {
                        div .demo-section-title { "Ticketing" }
                        p .demo-hint {
                            "Live ticket purchase is not wired in this build. "
                            "The circuit-breaker, idempotency layer, and purchase ledger "
                            "exist; the GBR Retail write endpoint is pending commercial access."
                        }
                        p .demo-hint { "Status: " strong { "coming soon" } "." }
                    }
                }

            } // end demo-grid
        } // end demo-console
    })
}

// ---------------------------------------------------------------------------
// System Status fragment — GET /ui/dev/status  (polls every 5 s)
// ---------------------------------------------------------------------------

pub async fn dev_status_fragment(State(state): State<AppState>) -> Markup {
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
// Predictions panel fragment — GET /ui/dev/predictions
//
// Rolling accuracy + a live table of recent predicted-vs-actual outcomes.
// Reads from the prediction_outcomes ledger; no registry access here so the
// data survives train deactivations and process restarts.
// ---------------------------------------------------------------------------

pub async fn dev_predictions_fragment(State(state): State<AppState>) -> Markup {
    let recent = crate::db::predictions::recent_predictions(&state.db, 30)
        .await
        .unwrap_or_default();
    let summary = crate::db::predictions::accuracy_summary(&state.db, 24)
        .await
        .ok();

    let mae_str = summary
        .as_ref()
        .and_then(|s| s.mean_abs_error_mins)
        .map(|v| format!("{v:.1} min"))
        .unwrap_or_else(|| "—".to_string());
    let mean_pred_str = summary
        .as_ref()
        .and_then(|s| s.mean_predicted_mins)
        .map(|v| format!("{v:.1} min"))
        .unwrap_or_else(|| "—".to_string());
    let mean_actual_str = summary
        .as_ref()
        .and_then(|s| s.mean_actual_mins)
        .map(|v| format!("{v:.1} min"))
        .unwrap_or_else(|| "—".to_string());
    let finalised_count = summary.as_ref().map(|s| s.finalised_count).unwrap_or(0);

    html! {
        div .demo-status-grid {
            div .demo-status-card {
                span .demo-status-label { "Mean abs error · 24h" }
                span .demo-status-value { (mae_str) }
            }
            div .demo-status-card {
                span .demo-status-label { "Mean predicted · 24h" }
                span .demo-status-value { (mean_pred_str) }
            }
            div .demo-status-card {
                span .demo-status-label { "Mean actual · 24h" }
                span .demo-status-value { (mean_actual_str) }
            }
            div .demo-status-card {
                span .demo-status-label { "Finalised · 24h" }
                span .demo-status-value { (finalised_count) }
            }
            div .demo-status-card {
                span .demo-status-label { "Recent rows" }
                span .demo-status-value { (recent.len()) }
            }
        }

        @if recent.is_empty() {
            p .demo-hint style="margin-top:1rem;" {
                "No prediction outcomes recorded yet. Waiting for Darwin TS messages "
                "to drive the engine."
            }
        } @else {
            div .demo-pred-table-wrap {
                table .demo-pred-table {
                    thead {
                        tr {
                            th { "RID" }
                            th { "UID" }
                            th { "From" }
                            th { "Departs" }
                            th { "Predicted" }
                            th { "Conf" }
                            th { "Actual" }
                            th { "Error" }
                            th { "Status" }
                        }
                    }
                    tbody {
                        @for row in &recent {
                            (prediction_row(row))
                        }
                    }
                }
            }
        }
    }
}

fn prediction_row(o: &crate::db::predictions::PredictionOutcome) -> Markup {
    let (status_label, status_cls) = match o.final_delay_mins {
        Some(_) => ("Finalised", "status-ok"),
        None    => ("In flight", "status-warn"),
    };
    let actual = o.final_delay_mins.map(|v| format!("{v} min"));
    let err = o.abs_error_mins().map(|v| format!("{v} min"));
    let conf = o.prediction_confidence
        .map(|c| format!("{:.0}%", c * 100.0))
        .unwrap_or_else(|| "—".to_string());

    html! {
        tr {
            td { code { (o.rid) } }
            td { (o.uid) }
            td { (o.origin_crs) }
            td { (o.scheduled_departure.format("%H:%M")) }
            td { (o.predicted_delay_mins) " min" }
            td .dim { (conf) }
            td {
                @match actual {
                    Some(a) => (a),
                    None    => span .dim { "—" },
                }
            }
            td {
                @match err {
                    Some(e) => (e),
                    None    => span .dim { "—" },
                }
            }
            td { span .demo-status-value .(status_cls) { (status_label) } }
        }
    }
}

// ---------------------------------------------------------------------------
// Registry probe fragment — GET /ui/dev/registry?rid=XXX
// ---------------------------------------------------------------------------

pub async fn dev_registry_fragment(
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
// Ingest data-freshness indicator — GET /ui/dev/ingest/freshness
// Shows how many timetable_calls rows exist for today; loaded once on page open.
// ---------------------------------------------------------------------------

pub async fn dev_ingest_freshness(State(state): State<AppState>) -> Markup {
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
// Ingest start — POST /ui/dev/ingest/start
// ---------------------------------------------------------------------------

pub async fn dev_ingest_start(
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
                    hx-post="/ui/dev/ingest/start"
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
// Ingest SSE stream — GET /ui/dev/ingest/stream
// Emits the full rendered progress panel on every watch-channel change.
// ---------------------------------------------------------------------------

pub async fn dev_ingest_stream(
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
// SSE event monitor — GET /ui/dev/events
// Streams all state changes on the broadcast channel as plain HTML rows.
// Consumed by raw EventSource JS on the dev page (not htmx SSE extension).
// ---------------------------------------------------------------------------

pub async fn dev_events_sse(
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
