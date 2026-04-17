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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn not_found_sets_correct_code_and_message() {
        let e = ApiError::not_found("train missing");
        assert_eq!(e.code, "NOT_FOUND");
        assert!(e.error.contains("train missing"));
    }

    #[test]
    fn bad_request_sets_correct_code() {
        let e = ApiError::bad_request("invalid RID");
        assert_eq!(e.code, "BAD_REQUEST");
        assert!(e.error.contains("invalid RID"));
    }

    #[test]
    fn internal_sets_correct_code() {
        let e = ApiError::internal("registry failure");
        assert_eq!(e.code, "INTERNAL_ERROR");
    }

    #[test]
    fn not_found_maps_to_404() {
        let resp = ApiError::not_found("x").into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn bad_request_maps_to_400() {
        let resp = ApiError::bad_request("x").into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn internal_maps_to_500() {
        let resp = ApiError::internal("x").into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn train_summary_round_trips_through_json() {
        let s = TrainSummary {
            rid: "202404170123456".to_string(),
            origin_crs: Some("LDS".to_string()),
            scheduled_departure: "2024-04-17T12:00:00Z".to_string(),
            estimated_departure: None,
            delay_mins: Some(5),
            platform: Some("3".to_string()),
            is_cancelled: false,
            last_updated: "2024-04-17T12:01:00Z".to_string(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: TrainSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.rid, s.rid);
        assert_eq!(back.delay_mins, Some(5));
        assert!(!back.is_cancelled);
    }

    #[test]
    fn departure_board_entry_round_trips_through_json() {
        let entry = DepartureBoardEntry {
            rid: "202404170123456".to_string(),
            scheduled_departure: "2024-04-17T12:00:00Z".to_string(),
            estimated_departure: Some("2024-04-17T12:05:00Z".to_string()),
            delay_mins: Some(5),
            platform: None,
            is_cancelled: false,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: DepartureBoardEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.rid, entry.rid);
        assert_eq!(back.delay_mins, Some(5));
    }

    #[test]
    fn live_update_event_round_trips_through_json() {
        let ev = LiveUpdateEvent {
            rid: "202404170123456".to_string(),
            state: "Critical".to_string(),
            is_cancelled: Some(false),
            delay_mins: Some(3),
            platform: Some("4A".to_string()),
            timestamp: "2024-04-17T12:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: LiveUpdateEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.state, "Critical");
        assert_eq!(back.delay_mins, Some(3));
    }
}
