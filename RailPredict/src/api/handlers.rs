//! Synchronous HTTP request handlers.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::{NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
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
// Shared merge logic: DB timetable + live registry → Vec<DepartureBoardEntry>
// ---------------------------------------------------------------------------

/// Merge DB timetable rows and live registry entries into a sorted departure board.
///
/// Strategy (Item 2.3r):
/// 1. Fetch today's timetable from the DB.
/// 2. Get the live registry snapshot for the CRS.
/// 3. For each DB row, build a base entry; look for a matching registry entry
///    (same scheduled departure within 2 minutes) and overlay live data if found.
/// 4. Resolve destination names from the station table (batch lookup).
/// 5. Fall back to the registry-only list if the DB returns nothing.
pub async fn build_departure_board(
    state: &AppState,
    crs: &str,
) -> Vec<DepartureBoardEntry> {
    let today = Utc::now().date_naive();

    // Fetch DB timetable rows (Tier A).
    let db_rows = crate::db::static_data::departures_from(&state.db, crs, today)
        .await
        .unwrap_or_default();

    // Fetch live registry snapshot (Tier C / B).
    let registry_entries = state.registry.departure_snapshot(crs).await;

    // If no DB timetable data yet, degrade gracefully to registry-only.
    if db_rows.is_empty() {
        return registry_entries;
    }

    // Batch-resolve destination names for all registry entries that carry a CRS code.
    // `destination_name` on a registry entry holds the raw CRS code (set from Darwin XML).
    let unique_dest_crs: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        registry_entries
            .iter()
            .filter_map(|e| e.destination_name.clone())
            .filter(|d| d.len() == 3 && seen.insert(d.clone()))
            .collect()
    };

    let mut dest_name_map: HashMap<String, String> = HashMap::new();
    for dest_crs in &unique_dest_crs {
        if let Ok(Some(station)) =
            crate::db::static_data::get_station(&state.db, dest_crs).await
        {
            dest_name_map.insert(dest_crs.clone(), station.name);
        }
    }

    // Build merged entries from DB rows.
    let mut entries: Vec<DepartureBoardEntry> = db_rows
        .into_iter()
        .map(|call| {
            // Build a UTC DateTime from the DB date + time.
            let scheduled_dt = call.scheduled_departure.map(|t| {
                NaiveDateTime::new(call.operating_date, t).and_utc()
            });

            let scheduled_iso = scheduled_dt
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default();

            // Look for a matching registry entry: same CRS, scheduled departure within 2 min.
            let registry_match = scheduled_dt.and_then(|db_dt| {
                registry_entries.iter().find(|re| {
                    if let Ok(re_dt) = re.scheduled_departure.parse::<chrono::DateTime<Utc>>() {
                        let diff = (db_dt - re_dt).num_seconds().abs();
                        diff <= 120
                    } else {
                        false
                    }
                })
            });

            // Prefer the registry RID; fall back to uid trimmed.
            let rid = registry_match
                .map(|rm| rm.rid.clone())
                .unwrap_or_else(|| call.uid.trim().to_string());

            // Overlay live fields from the registry match.
            let (estimated_departure, delay_mins, platform, is_cancelled, last_updated_secs_ago) =
                if let Some(rm) = registry_match {
                    (
                        rm.estimated_departure.clone(),
                        rm.delay_mins,
                        rm.platform.clone().or_else(|| call.platform.clone()),
                        rm.is_cancelled,
                        rm.last_updated_secs_ago,
                    )
                } else {
                    (None, None, call.platform.clone(), None, None)
                };

            // Resolve destination name: use the registry match's destination CRS (if any)
            // falling back to the raw DB platform field as a placeholder (there is no
            // destination CRS on TimetableCall — destination lookup is registry-driven).
            let destination_name = registry_match
                .and_then(|rm| rm.destination_name.as_ref())
                .and_then(|crs_key| dest_name_map.get(crs_key))
                .cloned();

            DepartureBoardEntry {
                rid,
                scheduled_departure: scheduled_iso,
                estimated_departure,
                delay_mins,
                platform,
                is_cancelled,
                last_updated_secs_ago,
                destination_name,
            }
        })
        .collect();

    // Sort by scheduled departure.
    entries.sort_by_key(|e| {
        e.scheduled_departure
            .parse::<chrono::DateTime<Utc>>()
            .unwrap_or_default()
    });

    entries
}

// ---------------------------------------------------------------------------
// GET /stations/{crs}/departures  — Tier A + live overlay
//
// Returns merged DB timetable + live registry entries, sorted by scheduled
// departure. No direct GBR call.
// ---------------------------------------------------------------------------

pub async fn departures_handler(
    Path(crs): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Vec<DepartureBoardEntry>>, ApiError> {
    validate_crs(&crs)?;
    let crs_upper = crs.to_uppercase();
    let entries = build_departure_board(&state, &crs_upper).await;
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
// Item 2.4 — Station name autocomplete
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StationSearchQuery {
    pub q: String,
}

#[derive(Debug, Serialize)]
pub struct StationResult {
    pub crs: String,
    pub name: String,
}

/// `GET /stations/search?q=<term>`
///
/// Returns up to 10 stations whose names match a full-text search on the query term.
/// Returns an empty array when `q` is shorter than 2 characters.
pub async fn station_search_handler(
    Query(params): Query<StationSearchQuery>,
    State(state): State<AppState>,
) -> Result<Json<Vec<StationResult>>, ApiError> {
    let q = params.q.trim().to_string();
    if q.len() < 2 {
        return Ok(Json(vec![]));
    }

    let rows: Vec<StationResult> = sqlx::query_as::<_, (String, String)>(
        "SELECT crs, name FROM stations \
         WHERE to_tsvector('english', name) @@ plainto_tsquery('english', $1) \
         ORDER BY name LIMIT 10",
    )
    .bind(&q)
    .fetch_all(&state.db)
    .await
    .map_err(|e| ApiError::internal(format!("DB error: {e}")))?
    .into_iter()
    .map(|(crs, name)| StationResult { crs, name })
    .collect();

    Ok(Json(rows))
}
