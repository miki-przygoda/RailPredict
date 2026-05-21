//! Train detail page and HTML SSE handler.

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
    state_machine::{poll_manager::StateChangeEvent, TrainState},
    types::TrainId,
};

use super::components::{delay_badge, platform_chip};
use super::layout::base;

fn pence_to_pounds(pence: i32) -> String {
    format!("£{:.2}", pence as f64 / 100.0)
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
            ))
        }
        None => None,
    };

    let fare_pence: Option<i32> = if let Some((ref origin, _, _, _, _, ref dest)) = snapshot {
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

    base(
        &format!("Train {rid}"),
        html! {
            @if let Some((origin, scheduled, delay, platform, cancelled, _dest)) = snapshot {
                div .train-header {
                    h1 { "Train " (rid) }
                    @if let Some(o) = &origin {
                        p .train-meta { "Departing from " (o) }
                    }
                    p .train-meta {
                        "Scheduled departure " (scheduled.get(11..16).unwrap_or("--:--"))
                    }
                    div .train-header-badges {
                        (delay_badge(delay, cancelled))
                        (platform_chip(platform.as_deref(), false))
                        @if let Some(pence) = fare_pence {
                            span .fare-chip { "From " (pence_to_pounds(pence)) }
                        }
                    }
                }
                div .live-section hx-ext="sse" sse-connect={ "/ui/trains/" (rid) "/live" } {
                    h2 { "Live updates" }
                    div # "live-status" sse-swap="update" hx-swap="outerHTML" {
                        p .connecting { "Connecting…" }
                    }
                }
            } @else {
                p .not-found { "Train " (rid) " not found in registry." }
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
    let (delay_mins, platform, is_cancelled) = match registry.get(&event.train_id) {
        Some(arc) => {
            let s = arc.read().await;
            (s.best_delay_mins(), s.best_platform().map(str::to_string), s.is_cancelled.value.unwrap_or(false))
        }
        None => (None, None, false),
    };

    let live = LiveUpdateEvent {
        rid: event.train_id.to_string(),
        state: format!("{:?}", event.new_state),
        is_cancelled: Some(is_cancelled),
        delay_mins,
        platform: platform.clone(),
        timestamp: Utc::now().to_rfc3339(),
    };

    html! {
        div # "live-status" {
            div .train-header-badges {
                (delay_badge(live.delay_mins, live.is_cancelled.unwrap_or(false)))
                (platform_chip(live.platform.as_deref(), false))
            }
            p .train-meta { "State: " (live.state) }
            p .last-updated {
                "Last updated " (live.timestamp.get(11..19).unwrap_or(&live.timestamp)) " UTC"
            }
        }
    }
}
