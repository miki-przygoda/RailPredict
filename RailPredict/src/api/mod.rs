//! axum HTTP API layer.
//!
//! ## API Contract
//!
//! | Method | Path                               | Tier | Returns                     | Notes                             |
//! |--------|------------------------------------|------|-----------------------------|-----------------------------------|
//! | GET    | /health                            | —    | `HealthResponse`            | 200 ok / 503 degraded             |
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
//! - `CorsLayer`: permissive in debug mode; restricted to `CORS_ALLOWED_ORIGINS` in production.
//! - `TraceLayer`: logs method, path, status, latency for every request.
//! - `GovernorLayer`: per-IP rate limiting (default 60 req/s); excludes /health and /metrics.
//!
//! ## Error shape
//! All JSON 4xx/5xx responses use `{ "error": "...", "code": "..." }` — see `types::ApiError`.

pub mod handlers;
pub mod sse;
pub mod types;

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use metrics_exporter_prometheus::PrometheusHandle;
use rust_embed::RustEmbed;
use tokio::sync::broadcast;
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::{
    cache::TrainRegistry,
    db::Db,
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
    /// DB connection pool — Tier A static data queries (timetable_calls, stations).
    /// Added as prereq for Observability epic (cache hit ratio metric) and Improvements 2.3.
    pub db: Db,
    /// Prometheus scrape handle — rendered by GET /metrics.
    pub prometheus: Arc<PrometheusHandle>,
    /// Allowed CORS origins from `CORS_ALLOWED_ORIGINS` env var.
    /// `None` means permissive (development / debug mode only).
    pub cors_allowed_origins: Option<Vec<String>>,
    /// Max requests per second per IP (0 = disabled).
    pub http_rate_limit_per_sec: u64,
}

// ---------------------------------------------------------------------------
// Metrics handler — GET /metrics
//
// Renders the current Prometheus scrape output as plain text.
// Gated behind the METRICS_ENABLED env var (default: enabled).
// This route is intentionally NOT behind the CORS middleware — it is for
// internal scraping by Prometheus only, not browser access.
// ---------------------------------------------------------------------------

async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    let enabled = std::env::var("METRICS_ENABLED")
        .map(|v| v.to_lowercase() != "false" && v != "0")
        .unwrap_or(true);

    if !enabled {
        return (StatusCode::NOT_FOUND, "Metrics disabled").into_response();
    }

    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        state.prometheus.render(),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router(state: AppState) -> Router {
    // Build the CORS layer from config.
    // Permissive only in dev mode (cors_allowed_origins is None, meaning LOG_LEVEL=debug).
    // In production, origins are restricted to the configured allow-list.
    let cors_layer = match &state.cors_allowed_origins {
        None => {
            // Dev/debug mode — permissive.
            CorsLayer::permissive()
        }
        Some(origins) => {
            let allow_list: Vec<HeaderValue> = origins
                .iter()
                .filter_map(|o| o.parse().ok())
                .collect();
            CorsLayer::new()
                .allow_origin(allow_list)
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::OPTIONS,
                ])
                .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
        }
    };

    // Build the per-IP rate-limit layer for the public API sub-router.
    // /health and /metrics are excluded (they live on separate sub-routers).
    let rate_limit_per_sec = state.http_rate_limit_per_sec;

    // /metrics and /health are registered on a separate sub-router WITHOUT CORS or
    // rate limiting so that Prometheus can scrape without preflight issues and
    // health checks are never throttled.
    let infra_router = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(handlers::health_handler))
        .with_state(state.clone());

    // Public API sub-router — rate-limited.
    let api_router = build_api_router(state, rate_limit_per_sec, cors_layer);

    api_router.merge(infra_router)
}

fn build_api_router(state: AppState, rate_limit_per_sec: u64, cors_layer: CorsLayer) -> Router {
    let base = Router::new()
        // Embedded static assets (CSS baked in at compile time)
        .route("/static/:path", get(static_handler))
        // Page routes — full server-rendered HTML pages
        .route("/", get(search::search_page))
        .route("/trains/:rid/view", get(detail::detail_page))
        // UI fragment routes — consumed by htmx partial swaps
        .route("/ui/stations/departures", get(search::departures_fragment))
        .route("/ui/trains/:rid/live", get(detail::ui_live_handler))
        // JSON API routes
        .route("/stations/:crs/departures", get(handlers::departures_handler))
        .route("/trains/:rid", get(handlers::train_handler))
        .route("/trains/:rid/live", get(sse::live_handler))
        .with_state(state)
        .layer(cors_layer)
        .layer(TraceLayer::new_for_http());

    if rate_limit_per_sec == 0 {
        // Rate limiting disabled.
        base
    } else {
        let governor_conf = Arc::new(
            GovernorConfigBuilder::default()
                .per_second(rate_limit_per_sec)
                .burst_size(rate_limit_per_sec as u32)
                .finish()
                .expect("GovernorConfig is valid"),
        );
        base.layer(GovernorLayer {
            config: governor_conf,
        })
    }
}
