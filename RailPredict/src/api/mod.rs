//! axum HTTP API layer.
//!
//! ## API Contract
//!
//! | Method | Path                          | Tier | Returns                     | Notes                            |
//! |--------|-------------------------------|------|-----------------------------|----------------------------------|
//! | GET    | /health                       | —    | `HealthResponse`            | Always 200                       |
//! | GET    | /stations/{crs}/departures    | A    | `Vec<DepartureBoardEntry>`  | Registry only, no GBR call       |
//! | GET    | /trains/{rid}                 | B    | `TrainSummary`              | Registry; 404 if unknown         |
//! | GET    | /trains/{rid}/live            | C    | SSE `LiveUpdateEvent`       | Heartbeat 15s; closes on Terminal |
//!
//! ## Middleware
//! - `CorsLayer`: permissive during development (all origins). Tighten for production.
//! - `TraceLayer`: logs method, path, status, latency for every request.
//!
//! ## Error shape
//! All 4xx/5xx responses use `{ "error": "...", "code": "..." }` — see `types::ApiError`.

pub mod handlers;
pub mod sse;
pub mod types;

use std::sync::Arc;

use axum::{
    routing::get,
    Router,
};
use tokio::sync::broadcast;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::{cache::TrainRegistry, state_machine::poll_manager::StateChangeEvent};

// ---------------------------------------------------------------------------
// Application state — shared across all handlers via axum State extractor
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<TrainRegistry>,
    /// Broadcast sender: SSE handlers call `.subscribe()` to get a receiver.
    pub state_change_tx: broadcast::Sender<StateChangeEvent>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health_handler))
        .route("/stations/:crs/departures", get(handlers::departures_handler))
        .route("/trains/:rid", get(handlers::train_handler))
        .route("/trains/:rid/live", get(sse::live_handler))
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}
