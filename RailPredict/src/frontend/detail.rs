//! Train detail page and HTML SSE handler.
//!
//! For a finalised service the page renders the captured journey: a per-stop trajectory
//! panel (scheduled vs actual per call, delay cell, cancellations) plus arrival/recovery
//! rollups, sourced from `db::journeys`. Live services keep the prediction card + htmx SSE
//! section. A per-train convergence chart (`charts::convergence_plot`) shows how the
//! prediction tracked toward the actual.

use std::convert::Infallible;

use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
};
use chrono::{Utc, Local};
use maud::{Markup, html};
use tokio::sync::broadcast::error::RecvError;

use crate::{
    api::{
        types::LiveUpdateEvent,
        AppState,
    },
    db::predictions::{convergence_for_rid, prediction_for_rid, ConvergencePoint, PredictionOutcome},
    state_machine::{StateChangeEvent, TrainState},
    types::TrainId,
};

use crate::cache::location_names;
use crate::db::journeys::{journey_calls_for, journey_header, JourneyCallView, JourneyHeaderView};

use super::charts;
use super::components::{delay_badge, pence_to_pounds, platform_chip, prediction_chip};
use super::layout::{base, NavPage};

/// Bucket a confidence value (0.0–1.0) into a coarse label for UI display.
fn confidence_label(c: f32) -> &'static str {
    match c {
        c if c >= 0.66 => "High",
        c if c >= 0.33 => "Medium",
        _ => "Low",
    }
}

/// Render the prediction snapshot + outcome card for the detail page.
fn prediction_card(
    live_predicted: Option<i32>,
    live_confidence: Option<f32>,
    outcome: Option<&PredictionOutcome>,
) -> Markup {
    html! {
        section .panel.prediction-card {
            div .panel-head { h2 { "Prediction" } }
            div .panel-body {
            div .prediction-grid {
                // Live prediction (from registry)
                div .prediction-cell {
                    div .prediction-label { "Live prediction" }
                    div .prediction-value {
                        @match live_predicted {
                            Some(mins) => { (mins) " min" }
                            None       => { span .dim { "—" } }
                        }
                    }
                    @if let Some(c) = live_confidence {
                        div .prediction-sub {
                            "Confidence " (confidence_label(c)) " "
                            span .dim { "(" (format!("{:.0}%", c * 100.0)) ")" }
                        }
                    }
                }

                // Persisted "first prediction" snapshot from prediction_outcomes
                @if let Some(o) = outcome {
                    div .prediction-cell {
                        div .prediction-label { "First prediction" }
                        div .prediction-value {
                            (o.predicted_delay_mins) " min"
                        }
                        div .prediction-sub {
                            "Locked in at "
                            (o.predicted_at.format("%H:%M UTC"))
                        }
                    }
                }

                // Outcome (only if finalised)
                @if let Some(o) = outcome
                    && let (Some(actual), Some(err)) = (o.final_delay_mins, o.abs_error_mins())
                {
                    div .prediction-cell {
                        div .prediction-label { "Actual outcome" }
                        div .prediction-value { (actual) " min" }
                        div .prediction-sub {
                            "Error " (err) " min · finalised "
                            @if let Some(ts) = o.finalised_at {
                                (ts.format("%H:%M UTC"))
                            } @else { "—" }
                        }
                    }
                }
            }

            // Correlation signal (if any was recorded)
            @if let Some(o) = outcome
                && let Some(corr_rid) = o.correlation_preceding_rid.as_deref()
            {
                p .prediction-correlation {
                    "Adjusted because preceding service "
                    code { (corr_rid) }
                    @if let Some(d) = o.correlation_preceding_delay_mins {
                        " was running " (d) " min late."
                    }
                }
            }
            }
        }
    }
}

/// Per-train prediction-convergence panel from `prediction_snapshots`.
fn convergence_panel(points: &[ConvergencePoint]) -> Markup {
    html! {
        section .panel.conv-panel {
            div .panel-head {
                h2 { "Prediction convergence" }
                span .panel-meta { "predicted delay as departure nears" }
            }
            div .panel-body {
                @if points.len() >= 2 {
                    @let predicted: Vec<i32> = points.iter().map(|p| p.predicted_delay_mins).collect();
                    @let final_delay = points[0].final_delay_mins;
                    div .conv-wrap { (charts::convergence_plot(&predicted, final_delay)) }
                    div .conv-legend {
                        span .conv-l-line { "predicted" }
                        span .conv-l-final { "actual · " (final_delay) "m" }
                    }
                    p .chart-note { "Snapshots from far before departure (left) to near departure (right)." }
                } @else {
                    p .panel-empty { "No prediction snapshots recorded for this train." }
                }
            }
        }
    }
}

/// A signed per-stop delay cell: "+5m" (late), "on time" (≤0), or "—".
fn delay_cell(d: Option<i32>) -> Markup {
    html! {
        @match d {
            Some(m) if m > 0 => { span .val-bad { "+" (m) "m" } }
            Some(_)          => { span .val-ok { "on time" } }
            None             => { span .dim { "—" } }
        }
    }
}

/// The finalised-journey panel: an arrival/recovery summary, the per-stop delay trajectory,
/// and the full calling-point list — the marquee surface for Full-Journey Capture.
fn journey_panel(header: &JourneyHeaderView, calls: &[JourneyCallView]) -> Markup {
    // Trajectory = best per-stop delay (departure, else arrival) along the route.
    let traj: Vec<f64> = calls
        .iter()
        .filter_map(|c| c.dep_delay_mins.or(c.arr_delay_mins))
        .map(|d| d as f64)
        .collect();
    let dest = header
        .destination_tpl
        .as_deref()
        .map(location_names::name_or_code)
        .unwrap_or("—");
    html! {
        section .panel {
            div .panel-head {
                h2 { "Journey" }
                span .panel-meta { (header.n_calls.unwrap_or(calls.len() as i16)) " stops" }
            }
            div .panel-body {
                p .dash-sub {
                    "Departed " (location_names::name_or_code(&header.origin_tpl)) " "
                    (delay_cell(header.origin_delay_mins))
                    " · arrived " (dest) " " (delay_cell(header.arrival_delay_mins))
                    @if let Some(r) = header.recovered_mins { @if r > 0 {
                        " · " span .val-ok { "recovered " (r) " min" }
                    } }
                    @if let Some(m) = header.max_delay_mins { " · peak +" (m.max(0)) "m" }
                }
                @if traj.len() >= 2 {
                    div .conv-wrap { (charts::area_spark(&traj, "journey-traj")) }
                    p .chart-note { "Delay (min) at each calling point · origin → destination." }
                }
                @if !calls.is_empty() {
                    table .rt-table {
                        thead { tr { th { "Stop" } th .r { "Arr" } th .r { "Dep" } th .r { "Plat" } } }
                        tbody {
                            @for c in calls {
                                tr {
                                    td {
                                        (location_names::name_or_code(&c.tpl))
                                        @if c.is_cancelled { " " span .op-code { "cancelled" } }
                                    }
                                    td .r { (delay_cell(c.arr_delay_mins)) }
                                    td .r { (delay_cell(c.dep_delay_mins)) }
                                    td .r.muted { (c.platform.as_deref().unwrap_or("—")) }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub async fn detail_page(Path(rid): Path<String>, State(state): State<AppState>) -> Markup {
    let snapshot = match TrainId::rid(&rid).ok().and_then(|id| state.registry.get(&id)) {
        Some(arc) => {
            let s = arc.read().await;
            Some((
                s.origin_crs.clone(),
                s.scheduled_departure.value.to_rfc3339(),
                s.best_delay_mins(),
                s.best_platform().map(str::to_string),
                s.is_cancelled.value.unwrap_or(false),
                s.destination_crs.clone(),
                s.predicted_delay_mins.value,
                s.volatility.historical_reliability,
            ))
        }
        None => None,
    };

    let fare_pence: Option<i32> = if let Some((ref origin, _, _, _, _, ref dest, _, _)) = snapshot {
        if let (Some(o), Some(d)) = (origin, dest) {
            let today = Local::now().date_naive();
            crate::db::static_data::cheapest_fare(&state.db, o, d, today)
                .await
                .ok()
                .flatten()
                .map(|f| f.price_pence)
        } else {
            None
        }
    } else {
        None
    };

    // Persisted first-prediction snapshot + (optional) finalised outcome.
    let outcome = prediction_for_rid(&state.db, &rid).await.ok().flatten();
    let convergence = convergence_for_rid(&state.db, &rid).await.unwrap_or_default();
    let journey = journey_header(&state.db, &rid).await.ok().flatten();
    let journey_calls = journey_calls_for(&state.db, &rid).await.unwrap_or_default();

    base(
        &format!("Train {rid}"),
        NavPage::Departures,
        html! {
            @if let Some((origin, scheduled, delay, platform, cancelled, _dest, pred, conf)) = snapshot {
                div .dash-header {
                    div {
                        h1 .dash-title { "Train " (rid) }
                        p .dash-sub {
                            @if let Some(o) = &origin { "From " (location_names::name_or_code(o)) " · " }
                            "scheduled " (scheduled.get(11..16).unwrap_or("--:--"))
                        }
                    }
                    div .dash-header-right.train-header-badges {
                        (delay_badge(delay, cancelled))
                        (platform_chip(platform.as_deref(), false))
                        @if let Some(pence) = fare_pence {
                            span .fare-chip { "From " (pence_to_pounds(pence)) }
                        }
                    }
                }

                (prediction_card(pred, conf, outcome.as_ref()))
                (convergence_panel(&convergence))

                section .panel.live-section hx-ext="sse" sse-connect={ "/ui/trains/" (rid) "/live" } {
                    div .panel-head { h2 { "Live updates" } }
                    div .panel-body {
                        div # "live-status" sse-swap="update" hx-swap="outerHTML" {
                            p .connecting { "Connecting…" }
                        }
                    }
                }
            } @else if let Some(h) = &journey {
                div .dash-header {
                    div {
                        h1 .dash-title { "Train " (rid) }
                        p .dash-sub {
                            "From " (location_names::name_or_code(&h.origin_tpl))
                            " · scheduled " (h.scheduled_departure.format("%H:%M")) " UTC · finalised"
                            @if let Some(t) = &h.toc { " · " (t.trim()) }
                        }
                    }
                    div .dash-header-right.train-header-badges {
                        (delay_badge(h.arrival_delay_mins, h.was_cancelled))
                    }
                }
                (prediction_card(None, None, outcome.as_ref()))
                (convergence_panel(&convergence))
                (journey_panel(h, &journey_calls))
            } @else {
                div .panel { div .panel-body { p .panel-empty { "Train " (rid) " not found." } } }
            }
        },
    )
}

/// SSE endpoint that sends maud HTML fragments for htmx OOB swaps.
///
/// `GET /ui/trains/:rid/live` — consumed by `hx-ext="sse"` on the detail page.
/// Each SSE event is named `update` so `sse-swap="update"` targets it directly.
pub async fn ui_live_handler(
    Path(rid): Path<String>,
    State(state): State<AppState>,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, crate::api::types::ApiError>
{
    let train_id = TrainId::rid(&rid).map_err(|_| {
        crate::api::types::ApiError::bad_request(format!("Invalid RID format: {rid}"))
    })?;

    let mut rx = state.state_change_tx.subscribe();
    let registry = state.registry;

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) if event.train_id == train_id => {
                    let is_terminal = event.new_state == TrainState::Terminal;
                    let fragment = enrich_to_html(&registry, &event).await;
                    yield Ok::<Event, Infallible>(
                        Event::default().event("update").data(fragment.into_string())
                    );
                    if is_terminal { break; }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(rid = %rid, skipped = n, "HTML SSE receiver lagged");
                }
                Err(RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn enrich_to_html(
    registry: &crate::cache::TrainRegistry,
    event: &StateChangeEvent,
) -> Markup {
    let (delay_mins, predicted_delay_mins, prediction_confidence, platform, is_cancelled) =
        match registry.get(&event.train_id) {
            Some(arc) => {
                let s = arc.read().await;
                (
                    s.best_delay_mins(),
                    s.predicted_delay_mins.value,
                    s.volatility.historical_reliability,
                    s.best_platform().map(str::to_string),
                    s.is_cancelled.value.unwrap_or(false),
                )
            }
            None => (None, None, None, None, false),
        };

    let live = LiveUpdateEvent {
        rid: event.train_id.to_string(),
        state: format!("{:?}", event.new_state),
        is_cancelled: Some(is_cancelled),
        delay_mins,
        predicted_delay_mins,
        prediction_confidence,
        platform: platform.clone(),
        timestamp: Utc::now().to_rfc3339(),
    };

    html! {
        div # "live-status" {
            div .train-header-badges {
                (delay_badge(live.delay_mins, live.is_cancelled.unwrap_or(false)))
                (platform_chip(live.platform.as_deref(), false))
                (prediction_chip(live.predicted_delay_mins, live.delay_mins))
            }
            p .train-meta { "State: " (live.state) }
            p .last-updated {
                "Last updated " (live.timestamp.get(11..19).unwrap_or(&live.timestamp)) " UTC"
            }
        }
    }
}
