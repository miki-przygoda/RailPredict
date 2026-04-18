//! Synchronous HTTP request handlers.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use tokio::time::Duration;

use crate::types::TrainId;

use super::{
    types::{ApiError, DepartureBoardEntry, HealthResponse, TrainSummary},
    AppState,
};

// ---------------------------------------------------------------------------
// CRS validation helper
// ---------------------------------------------------------------------------

/// Validate a CRS code: exactly 3 ASCII alphabetic characters (A–Z, case-insensitive).
/// Returns `Err(ApiError::bad_request(...))` if the input is invalid.
fn validate_crs(crs: &str) -> Result<(), ApiError> {
    if crs.len() == 3 && crs.chars().all(|c| c.is_ascii_alphabetic()) {
        Ok(())
    } else {
        Err(ApiError::bad_request("CRS must be exactly 3 ASCII letters"))
    }
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

/// `GET /health`
///
/// Probes DB liveness with a 1-second timeout.
/// Returns 200 `{ "status": "ok" }` when healthy.
/// Returns 503 `{ "status": "degraded", "detail": "db unreachable" }` when the DB
/// cannot be reached within 1 second.
pub async fn health_handler(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let db_ok = tokio::time::timeout(
        Duration::from_secs(1),
        sqlx::query("SELECT 1").execute(&state.db),
    )
    .await
    .map(|result| result.is_ok())
    .unwrap_or(false);

    if db_ok {
        (
            StatusCode::OK,
            Json(HealthResponse {
                status: "ok",
                version: env!("CARGO_PKG_VERSION"),
                detail: None,
            }),
        )
            .into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse {
                status: "degraded",
                version: env!("CARGO_PKG_VERSION"),
                detail: Some("db unreachable"),
            }),
        )
            .into_response()
    }
}

// ---------------------------------------------------------------------------
// GET /stations/{crs}/departures  — Tier A
//
// Returns all trains in the registry whose origin_crs matches the requested
// CRS code, sorted by scheduled departure. No live GBR call.
// ---------------------------------------------------------------------------

pub async fn departures_handler(
    Path(crs): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Vec<DepartureBoardEntry>>, ApiError> {
    validate_crs(&crs)?;
    let entries = state.registry.departure_snapshot(&crs).await;
    Ok(Json(entries))
}

// ---------------------------------------------------------------------------
// GET /trains/{rid}  — Tier B
//
// Returns the current TrainSummary from the registry.
// Returns 404 if the RID is not registered.
// ---------------------------------------------------------------------------

pub async fn train_handler(
    Path(rid): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<TrainSummary>, ApiError> {
    let train_id = TrainId::rid(&rid)
        .map_err(|_| ApiError::bad_request(format!("Invalid RID format: {rid}")))?;

    let arc = state
        .registry
        .get(&train_id)
        .ok_or_else(|| ApiError::not_found(format!("Train {rid} not found in registry")))?;

    let status = arc.read().await;
    let last_updated = status
        .scheduled_departure
        .last_updated
        .max(status.actual_estimated_departure.last_updated)
        .max(status.actual_platform.last_updated);

    Ok(Json(TrainSummary {
        rid: status.id.to_string(),
        origin_crs: status.origin_crs.clone(),
        scheduled_departure: status.scheduled_departure.value.to_rfc3339(),
        estimated_departure: status
            .actual_estimated_departure
            .value
            .map(|dt| dt.to_rfc3339()),
        delay_mins: status.best_delay_mins(),
        platform: status.best_platform().map(str::to_string),
        is_cancelled: status.is_cancelled.value,
        last_updated: last_updated.to_rfc3339(),
    }))
}

