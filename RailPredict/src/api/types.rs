//! Client-facing DTOs — the stable API contract.
//!
//! These are flat, serialisable views of internal types. No `TrainId` enum,
//! no `Stamped<T>` wrappers — just plain JSON-friendly fields.
//! Changing these shapes is a breaking API change; bump the minor version.

use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainSummary {
    pub rid: String,
    pub origin_crs: Option<String>,
    /// ISO 8601 scheduled departure.
    pub scheduled_departure: String,
    /// ISO 8601 best estimated departure (actual or estimated).
    pub estimated_departure: Option<String>,
    pub delay_mins: Option<i32>,
    pub platform: Option<String>,
    pub is_cancelled: bool,
    /// ISO 8601 timestamp of the last registry update.
    pub last_updated: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepartureBoardEntry {
    pub rid: String,
    /// ISO 8601 scheduled departure.
    pub scheduled_departure: String,
    /// ISO 8601 best estimated departure.
    pub estimated_departure: Option<String>,
    pub delay_mins: Option<i32>,
    pub platform: Option<String>,
    pub is_cancelled: bool,
}

/// Streamed over SSE for `GET /trains/{rid}/live`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveUpdateEvent {
    pub rid: String,
    /// Human-readable state label: "Dormant", "Monitored", "Active", "Critical", "Terminal".
    pub state: String,
    pub is_cancelled: Option<bool>,
    pub delay_mins: Option<i32>,
    pub platform: Option<String>,
    /// ISO 8601 timestamp when this event was generated.
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
}

// ---------------------------------------------------------------------------
// Error envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
    pub code: String,
}

impl ApiError {
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self { error: msg.into(), code: "NOT_FOUND".to_string() }
    }

    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self { error: msg.into(), code: "BAD_REQUEST".to_string() }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self { error: msg.into(), code: "INTERNAL_ERROR".to_string() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = match self.code.as_str() {
            "NOT_FOUND" => StatusCode::NOT_FOUND,
            "BAD_REQUEST" => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self)).into_response()
    }
}
