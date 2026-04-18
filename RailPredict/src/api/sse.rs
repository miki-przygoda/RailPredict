//! SSE endpoint: `GET /trains/{rid}/live`
//!
//! Subscribes the caller to the broadcast state-change channel, filters for
//! the requested RID, enriches each event with live registry data, and streams
//! as `text/event-stream`. A 15s keepalive comment prevents proxy timeouts.
//! The stream closes when the train reaches `Terminal` state.

use std::convert::Infallible;

use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
};
use chrono::Utc;
use tokio::sync::broadcast::error::RecvError;

use crate::{
    state_machine::{poll_manager::StateChangeEvent, TrainState},
    types::TrainId,
};

use super::{types::LiveUpdateEvent, AppState};

pub async fn live_handler(
    Path(rid): Path<String>,
    State(state): State<AppState>,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, super::types::ApiError>
{
    let train_id = TrainId::rid(&rid)
        .map_err(|_| super::types::ApiError::bad_request(format!("Invalid RID format: {rid}")))?;

    let mut rx = state.state_change_tx.subscribe();
    let registry = state.registry;

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) if event.train_id == train_id => {
                    let is_terminal = event.new_state == TrainState::Terminal;
                    let live = enrich(&registry, &event).await;
                    let data = serde_json::to_string(&live).unwrap_or_default();
                    yield Ok::<Event, Infallible>(Event::default().data(data));
                    if is_terminal {
                        break;
                    }
                }
                Ok(_) => {} // different train — skip
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(rid = %rid, skipped = n, "SSE receiver lagged — some events dropped");
                }
                Err(RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// Build a `LiveUpdateEvent` from a state-change event + current registry snapshot.
async fn enrich(
    registry: &crate::cache::TrainRegistry,
    event: &StateChangeEvent,
) -> LiveUpdateEvent {
    let (delay_mins, platform, is_cancelled) = match registry.get(&event.train_id) {
        Some(arc) => {
            let s = arc.read().await;
            (
                s.best_delay_mins(),
                s.best_platform().map(str::to_string),
                s.is_cancelled.value,
            )
        }
        None => (None, None, None),
    };

    LiveUpdateEvent {
        rid: event.train_id.to_string(),
        state: format!("{:?}", event.new_state),
        is_cancelled,
        delay_mins,
        platform,
        timestamp: Utc::now().to_rfc3339(),
    }
}
