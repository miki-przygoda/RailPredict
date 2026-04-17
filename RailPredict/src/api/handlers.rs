//! Synchronous HTTP request handlers.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;

use crate::types::TrainId;

use super::{
    types::{ApiError, DepartureBoardEntry, HealthResponse, TrainSummary},
    AppState,
};

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

pub async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
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
    let crs_upper = crs.to_uppercase();
    let arcs = state.registry.snapshot_all();

    let mut entries: Vec<DepartureBoardEntry> = Vec::new();

    for arc in arcs {
        let status = arc.read().await;
        if status.origin_crs.as_deref().map(str::to_uppercase).as_deref() != Some(&crs_upper) {
            continue;
        }

        entries.push(DepartureBoardEntry {
            rid: status.id.to_string(),
            scheduled_departure: status.scheduled_departure.value.to_rfc3339(),
            estimated_departure: status
                .actual_estimated_departure
                .value
                .map(|dt| dt.to_rfc3339()),
            delay_mins: status.best_delay_mins(),
            platform: status.best_platform().map(str::to_string),
            is_cancelled: status.is_cancelled.value,
        });
    }

    entries.sort_by_key(|e| e.scheduled_departure.clone());

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

// ---------------------------------------------------------------------------
// Unused import suppression
// ---------------------------------------------------------------------------

#[allow(unused_imports)]
use Utc as _;
