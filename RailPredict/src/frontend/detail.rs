//! Train detail page and HTML SSE handler.

use std::convert::Infallible;

use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
};
use chrono::Utc;
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

pub async fn detail_page(Path(rid): Path<String>, State(state): State<AppState>) -> Markup {
    let snapshot = match TrainId::rid(&rid).ok().and_then(|id| state.registry.get(&id)) {
        Some(arc) => {
            let s = arc.read().await;
            Some((
                s.origin_crs.clone(),
                s.scheduled_departure.value.to_rfc3339(),
                s.best_delay_mins(),
                s.best_platform().map(str::to_string),
                s.is_cancelled.value,
            ))
        }
        None => None,
    };

    base(
        &format!("Train {rid}"),
        html! {
            @if let Some((origin, scheduled, delay, platform, cancelled)) = snapshot {
                div .train-header {
                    h1 { "Train " (rid) }
                    @if let Some(o) = &origin {
                        p .train-meta { "From " (o) }
                    }
                    p .train-meta {
                        "Scheduled " (scheduled.get(11..16).unwrap_or("--:--"))
                    }
                    div style="display:flex;gap:0.75rem;align-items:center;" {
                        (delay_badge(delay, cancelled))
                        (platform_chip(platform.as_deref()))
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
            (s.best_delay_mins(), s.best_platform().map(str::to_string), s.is_cancelled.value)
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
            div style="display:flex;gap:0.75rem;align-items:center;" {
                (delay_badge(live.delay_mins, live.is_cancelled.unwrap_or(false)))
                (platform_chip(live.platform.as_deref()))
            }
            p .train-meta { "State: " (live.state) }
            p .last-updated {
                "Updated " (live.timestamp.get(11..19).unwrap_or(&live.timestamp))
            }
        }
    }
}
