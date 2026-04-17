//! axum HTTP API layer.
//!
//! ## API Contract
//!
//! | Method | Path                               | Tier | Returns                     | Notes                             |
//! |--------|------------------------------------|------|-----------------------------|-----------------------------------|
//! | GET    | /health                            | —    | `HealthResponse`            | Always 200                        |
//! | GET    | /stations/{crs}/departures         | A    | `Vec<DepartureBoardEntry>`  | Registry only, no GBR call        |
//! | GET    | /trains/{rid}                      | B    | `TrainSummary`              | Registry; 404 if unknown          |
//! | GET    | /trains/{rid}/live                 | C    | SSE `LiveUpdateEvent` JSON  | Heartbeat 15s; closes on Terminal |
//! | GET    | /                                  | —    | HTML search page            | maud server-rendered              |
//! | GET    | /trains/{rid}/view                 | B/C  | HTML detail page            | maud + htmx SSE                   |
//! | GET    | /ui/stations/departures?crs=XXX    | A    | HTML fragment               | htmx swap target                  |
//! | GET    | /ui/trains/{rid}/live              | C    | SSE HTML fragments          | htmx `sse-swap="update"`          |
//! | GET    | /static/{path}                     | —    | Embedded static asset       | rust-embed, no filesystem dep     |
//!
//! ## Middleware
//! - `CorsLayer`: permissive during development. Tighten for production.
//! - `TraceLayer`: logs method, path, status, latency for every request.
//!
//! ## Error shape
//! All JSON 4xx/5xx responses use `{ "error": "...", "code": "..." }` — see `types::ApiError`.

pub mod handlers;
pub mod sse;
pub mod types;

use std::sync::Arc;

use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use rust_embed::RustEmbed;
use tokio::sync::broadcast;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::{
    cache::TrainRegistry,
    frontend::{detail, search},
    state_machine::poll_manager::StateChangeEvent,
};

// ---------------------------------------------------------------------------
// Embedded static assets — baked into the binary at compile time via rust-embed
// ---------------------------------------------------------------------------

#[derive(RustEmbed)]
#[folder = "static/"]
struct StaticAssets;

async fn static_handler(Path(path): Path<String>) -> impl IntoResponse {
    match StaticAssets::get(&path) {
        Some(file) => {
            let mime = if path.ends_with(".css") {
                "text/css; charset=utf-8"
            } else if path.ends_with(".js") {
                "application/javascript"
            } else {
                "application/octet-stream"
            };
            ([(header::CONTENT_TYPE, mime)], file.data).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

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
        // Embedded static assets (CSS baked in at compile time)
        .route("/static/:path", get(static_handler))
        // Page routes — full server-rendered HTML pages
        .route("/", get(search::search_page))
        .route("/trains/:rid/view", get(detail::detail_page))
        // UI fragment routes — consumed by htmx partial swaps
        .route("/ui/stations/departures", get(search::departures_fragment))
        .route("/ui/trains/:rid/live", get(detail::ui_live_handler))
        // JSON API routes
        .route("/health", get(handlers::health_handler))
        .route("/stations/:crs/departures", get(handlers::departures_handler))
        .route("/trains/:rid", get(handlers::train_handler))
        .route("/trains/:rid/live", get(sse::live_handler))
        .with_state(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}
