//! Synchronous HTTP request handlers.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::{NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::time::Duration;

use crate::{export, types::TrainId};

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
        .unwrap_or_else(|e| {
            tracing::warn!(crs = %crs, error = %e, "departures_from query failed; serving empty timetable");
            Vec::new()
        });

    // Look up the station's TIPLOC so the registry snapshot can match Darwin entries
    // (which store TIPLOCs like "WATRLMN" rather than the 3-letter CRS "WAT").
    let tiploc: Option<String> = sqlx::query_scalar(
        "SELECT tiploc FROM stations WHERE UPPER(crs) = $1 AND tiploc IS NOT NULL",
    )
    .bind(crs.to_uppercase())
    .fetch_optional(&state.db)
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(crs = %crs, error = %e, "tiploc lookup query failed; registry match may miss Darwin entries");
        None
    })
    .flatten();

    let crs_codes: Vec<&str> = std::iter::once(crs)
        .chain(tiploc.as_deref())
        .collect();

    // Fetch live registry snapshot (Tier C / B).
    let registry_entries = state.registry.departure_snapshot(&crs_codes).await;

    // If no DB timetable data yet, degrade gracefully to registry-only.
    if db_rows.is_empty() {
        return registry_entries;
    }

    // Batch-resolve destination names for all registry entries that carry a CRS code.
    // `destination_name` on a registry entry holds the raw CRS code (set from Darwin XML).
    let unique_dest_codes: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        registry_entries
            .iter()
            .filter_map(|e| e.destination_name.clone())
            .filter(|d| (3..=7).contains(&d.len()) && seen.insert(d.clone()))
            .collect()
    };

    let mut dest_name_map: HashMap<String, String> = HashMap::new();
    for dest_code in &unique_dest_codes {
        if dest_code.len() == 3 {
            match crate::db::static_data::get_station(&state.db, dest_code).await {
                Ok(Some(station)) => {
                    dest_name_map.insert(dest_code.clone(), station.name);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(crs = %crs, dest_code = %dest_code, error = %e, "destination get_station lookup failed; name unresolved");
                }
            }
        } else {
            // TIPLOC (4–7 chars) — look up via stations table tiploc column.
            match sqlx::query_scalar::<_, String>(
                "SELECT name FROM stations WHERE UPPER(tiploc) = $1",
            )
            .bind(dest_code.to_uppercase())
            .fetch_optional(&state.db)
            .await
            {
                Ok(Some(name)) => {
                    dest_name_map.insert(dest_code.clone(), name);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(crs = %crs, dest_tiploc = %dest_code, error = %e, "destination tiploc name lookup failed; name unresolved");
                }
            }
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
            // `is_platform_planned` is true only when no live source has confirmed a platform
            // and we are falling back to the static timetable DB value.
            let (estimated_departure, delay_mins, platform, is_platform_planned, is_cancelled, last_updated_secs_ago, predicted_delay_mins) =
                if let Some(rm) = registry_match {
                    let live_platform = rm.platform.clone();
                    let planned = live_platform.is_none() && call.platform.is_some();
                    (
                        rm.estimated_departure.clone(),
                        rm.delay_mins,
                        live_platform.or_else(|| call.platform.clone()),
                        planned,
                        rm.is_cancelled,
                        rm.last_updated_secs_ago,
                        rm.predicted_delay_mins,
                    )
                } else {
                    let planned = call.platform.is_some();
                    (None, None, call.platform.clone(), planned, None, None, None)
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
                is_platform_planned,
                is_cancelled,
                last_updated_secs_ago,
                destination_name,
                predicted_delay_mins,
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
        predicted_delay_mins: status.predicted_delay_mins.value,
        prediction_confidence: status.volatility.historical_reliability,
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
    pub crs_input_id: Option<String>,
    pub q_input_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Item 5.1 — Journey search (A → B)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct JourneyQuery {
    pub from: String,
    pub to: String,
    pub date: Option<String>, // YYYY-MM-DD; defaults to today
}

/// `GET /journeys?from=XXX&to=YYY[&date=YYYY-MM-DD]`
///
/// Returns all direct services that call both `from` and `to` in order
/// (origin `call_order` < destination `call_order`) on the given operating date.
pub async fn journey_handler(
    Query(params): Query<JourneyQuery>,
    State(state): State<AppState>,
) -> Result<Json<Vec<DepartureBoardEntry>>, ApiError> {
    validate_crs(&params.from)?;
    validate_crs(&params.to)?;

    if params.from.eq_ignore_ascii_case(&params.to) {
        return Err(ApiError::bad_request("Origin and destination must differ"));
    }

    let from = params.from.to_uppercase();
    let to = params.to.to_uppercase();

    let date = params
        .date
        .as_deref()
        .and_then(|s| s.parse::<chrono::NaiveDate>().ok())
        .unwrap_or_else(|| chrono::Utc::now().date_naive());

    // Join timetable_calls to itself: find services calling both `from` and `to`
    // in order. The table has no trip_id; join on (uid, operating_date).
    // No scheduled_arrival column exists; use scheduled_departure of the `to` call.
    let rows = sqlx::query_as::<_, (String, chrono::NaiveTime, Option<String>)>(
        "SELECT tc_from.uid, \
                tc_from.scheduled_departure, \
                tc_from.platform \
         FROM timetable_calls tc_from \
         JOIN timetable_calls tc_to \
             ON tc_to.uid            = tc_from.uid \
            AND tc_to.operating_date = tc_from.operating_date \
            AND tc_to.location_crs   = $2 \
            AND tc_to.call_order     > tc_from.call_order \
         WHERE tc_from.location_crs  = $1 \
           AND tc_from.operating_date = $3 \
         ORDER BY tc_from.scheduled_departure",
    )
    .bind(&from)
    .bind(&to)
    .bind(date)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        tracing::error!("journey lookup query failed: {e}");
        ApiError::internal("journey lookup failed")
    })?;

    let entries: Vec<DepartureBoardEntry> = rows
        .into_iter()
        .map(|(uid, dep_time, platform)| {
            let scheduled_dt = chrono::NaiveDateTime::new(date, dep_time).and_utc();
            let is_platform_planned = platform.is_some();
            DepartureBoardEntry {
                rid: uid.trim().to_string(),
                scheduled_departure: scheduled_dt.to_rfc3339(),
                estimated_departure: None,
                delay_mins: None,
                platform,
                is_platform_planned,
                is_cancelled: None,
                last_updated_secs_ago: None,
                destination_name: Some(to.clone()),
                predicted_delay_mins: None,
            }
        })
        .collect();

    Ok(Json(entries))
}

#[derive(Debug, Serialize)]
pub struct StationResult {
    pub crs: String,
    pub name: String,
}

// ---------------------------------------------------------------------------
// Report handler — GET /report
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ReportQuery {
    #[serde(default = "default_days")]
    days: u32,
}

fn default_days() -> u32 { 7 }

/// `GET /report[?days=N]`
///
/// Generates and serves the self-contained delay-intelligence HTML report
/// directly in the browser. Equivalent to `make export` but served live.
pub async fn report_handler(
    Query(params): Query<ReportQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    match export::render_html(&state.db, params.days).await {
        Ok(html) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            html,
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("Report generation failed: {e}"),
        )
            .into_response(),
    }
}
