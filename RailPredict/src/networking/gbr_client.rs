//! GBR REST API client.
//!
//! `GbrClient` is a trait so tests can inject a mock without live credentials.
//! `LiveGbrClient` is the production implementation backed by `reqwest`.
//!
//! ## Credentials
//! Darwin Push Port: ActiveMQ username + password (applied for separately at
//! the National Rail open data portal — https://opendata.nationalrail.co.uk).
//! GBR Retail API: API key passed as `x-apikey` header.
//! Both are read from environment variables at construction time; never hardcoded.
//!
//! ## Endpoints (GBR Retail API v1)
//! All relative to `GBR_API_BASE_URL`. Auth via `x-apikey: <key>` header.
//!
//! | Constant                  | Method | Path                                          | Purpose                        |
//! |---------------------------|--------|-----------------------------------------------|--------------------------------|
//! | `ENDPOINT_TRAIN_STATUS`   | GET    | `/v1/train/{rid}/status`                      | Live status for one service    |
//!
//! Response shapes are defined as `serde` structs below. If GBR changes its schema,
//! update here; nothing else in the codebase should parse raw GBR JSON.

use async_trait::async_trait;
use serde::Deserialize;
use thiserror::Error;

use crate::types::{TrainId, TrainStatus};

// ---------------------------------------------------------------------------
// Endpoint constants
// ---------------------------------------------------------------------------

/// Base URL for the GBR Retail API. Override via `GBR_API_BASE_URL` env var in tests.
pub const GBR_API_BASE_URL: &str = "https://api.rtt.io/api";

pub const ENDPOINT_TRAIN_STATUS: &str = "/v1/train/{rid}/status";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum GbrClientError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("GBR returned 503 Service Unavailable — circuit breaker should open")]
    ServiceUnavailable,

    #[error("GBR returned 429 Too Many Requests — rate limiter should back off")]
    RateLimited,

    #[error("GBR returned unexpected status {status}: {body}")]
    UnexpectedStatus { status: u16, body: String },

    #[error("Train {0} not found in GBR response")]
    NotFound(TrainId),
}

/// Cloneable classification of a [`GbrClientError`], preserved after the typed error
/// is fanned out to many coalescer waiters (the error itself isn't `Clone` because of
/// `reqwest::Error`). The circuit breaker routes on this, never on message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GbrErrorKind {
    /// 503 — upstream unavailable; counts toward the breaker.
    Unavailable,
    /// 429 — rate limited; back off, do NOT trip the breaker.
    RateLimited,
    /// Transport failure (timeout, connection refused, DNS, TLS); counts toward the breaker.
    Transport,
    /// 5xx other than 503 (500/502/504); counts toward the breaker.
    ServerError,
    /// An unexpected non-5xx status that isn't 429.
    ClientError,
    /// Train not present in the GBR response.
    NotFound,
}

impl GbrErrorKind {
    /// Whether a failure of this kind should increment the circuit breaker. Only
    /// genuine upstream brownouts (503 / transport / 5xx) do — rate-limiting and
    /// client/not-found errors are not breaker failures.
    pub fn is_breaker_failure(self) -> bool {
        matches!(self, Self::Unavailable | Self::Transport | Self::ServerError)
    }
}

impl GbrClientError {
    /// Classify this error for circuit-breaker / rate-limit routing.
    pub fn kind(&self) -> GbrErrorKind {
        match self {
            GbrClientError::ServiceUnavailable => GbrErrorKind::Unavailable,
            GbrClientError::RateLimited => GbrErrorKind::RateLimited,
            GbrClientError::Http(_) => GbrErrorKind::Transport,
            GbrClientError::UnexpectedStatus { status, .. } if *status >= 500 => {
                GbrErrorKind::ServerError
            }
            GbrClientError::UnexpectedStatus { .. } => GbrErrorKind::ClientError,
            GbrClientError::NotFound(_) => GbrErrorKind::NotFound,
        }
    }
}

// ---------------------------------------------------------------------------
// RTT API response shapes
// ---------------------------------------------------------------------------

/// Top-level response from the RTT `/v1/train/{uid}/YYYY/MM/DD` endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RttServiceResponse {
    pub service_uid: String,
    pub run_date: String, // "YYYY-MM-DD"
    pub train_identity: Option<String>,
    pub is_passenger_train: Option<bool>,
    pub locations: Vec<RttLocation>,
}

/// A single calling point within an RTT service response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RttLocation {
    pub crs: Option<String>,
    pub description: Option<String>,
    pub gbtt_booked_departure: Option<String>,  // "HHMM"
    pub realtime_departure: Option<String>,     // "HHMM"
    pub platform: Option<String>,
    #[serde(default)]
    pub cancelled: bool,
    pub service_location: Option<String>, // "CALL", "PASS", "ORIGIN", "DESTINATION"
}

/// Parse an RTT "HHMM" time string into a `DateTime<Utc>` on a given date.
fn parse_rtt_time(s: &str, date: chrono::NaiveDate) -> Option<chrono::DateTime<chrono::Utc>> {
    let h = s.get(0..2)?.parse::<u32>().ok()?;
    let m = s.get(2..4)?.parse::<u32>().ok()?;
    chrono::NaiveTime::from_hms_opt(h, m, 0)
        .map(|t| chrono::NaiveDateTime::new(date, t).and_utc())
}

// ---------------------------------------------------------------------------
// Client trait
// ---------------------------------------------------------------------------

/// The interface all callers must use. `LiveGbrClient` implements this for production;
/// `MockGbrClient` (in tests) implements it without network access.
#[async_trait]
pub trait GbrClient: Send + Sync {
    /// Fetch the current live status for a single service by RID.
    async fn get_train_status(&self, rid: &TrainId) -> Result<TrainStatus, GbrClientError>;
}

// ---------------------------------------------------------------------------
// Live implementation
// ---------------------------------------------------------------------------

/// Production GBR client. Reads credentials from environment variables:
///   - `GBR_API_KEY` — x-apikey header value for the Retail API
pub struct LiveGbrClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl LiveGbrClient {
    /// Construct from environment variables. Returns `Err` if required vars are absent.
    pub fn from_env() -> anyhow::Result<Self> {
        let api_key = std::env::var("GBR_API_KEY")
            .map_err(|_| anyhow::anyhow!("GBR_API_KEY environment variable not set"))?;

        let base_url =
            std::env::var("GBR_API_BASE_URL").unwrap_or_else(|_| GBR_API_BASE_URL.to_string());

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        Ok(Self { http, api_key, base_url })
    }
}

#[async_trait]
impl GbrClient for LiveGbrClient {
    async fn get_train_status(&self, rid: &TrainId) -> Result<TrainStatus, GbrClientError> {
        let rid_str = match rid {
            TrainId::Rid(s) => s,
            _ => return Err(GbrClientError::NotFound(rid.clone())),
        };

        let url = format!(
            "{}{}",
            self.base_url,
            ENDPOINT_TRAIN_STATUS.replace("{rid}", rid_str)
        );

        // Phase 2: record latency histogram per endpoint + status.
        // Phase 4: annotate the current tracing span with RID and call metadata
        //          so Grafana latency spikes can be traced back to a specific RID in logs.
        let span = tracing::Span::current();
        span.record("rid", rid_str.as_str());
        span.record("endpoint", "train_status");

        let start = std::time::Instant::now();

        let resp = self
            .http
            .get(&url)
            .header("x-apikey", &self.api_key)
            .send()
            .await?;

        let status_code = resp.status().as_u16();
        let latency_ms = start.elapsed().as_millis() as f64;

        // Phase 4: record latency and status code on the span for log correlation.
        span.record("latency_ms", latency_ms);
        span.record("status_code", status_code);

        let outcome = match status_code {
            200 => "ok",
            429 => "rate_limited",
            503 => "unavailable",
            _ => "error",
        };

        // Phase 2: histogram with "endpoint" and "status" labels.
        metrics::histogram!(
            "gbr_api_latency_ms",
            "endpoint" => "train_status",
            "status"   => outcome
        )
        .record(latency_ms);

        tracing::debug!(
            rid = %rid_str,
            latency_ms,
            status_code,
            "GBR API call complete"
        );

        match status_code {
            200 => {
                let body: RttServiceResponse = resp
                    .json()
                    .await
                    .map_err(GbrClientError::Http)?;

                let run_date =
                    chrono::NaiveDate::parse_from_str(&body.run_date, "%Y-%m-%d")
                        .unwrap_or_else(|_| chrono::Utc::now().date_naive());

                // Use the first location as the origin for departure-time purposes.
                let origin = body
                    .locations
                    .first()
                    .ok_or_else(|| GbrClientError::NotFound(rid.clone()))?;

                let scheduled_dt = origin
                    .gbtt_booked_departure
                    .as_deref()
                    .and_then(|s| parse_rtt_time(s, run_date))
                    .unwrap_or_else(chrono::Utc::now);

                let estimated_dt = origin
                    .realtime_departure
                    .as_deref()
                    .and_then(|s| parse_rtt_time(s, run_date));

                let mut status =
                    TrainStatus::new(rid.clone(), scheduled_dt, scheduled_dt);

                status.actual_platform =
                    crate::types::train_status::Stamped::new(origin.platform.clone());

                let any_cancelled = body.locations.iter().any(|l| l.cancelled);
                status.is_cancelled =
                    crate::types::train_status::Stamped::new(Some(any_cancelled));

                if let Some(est) = estimated_dt {
                    status.actual_estimated_departure =
                        crate::types::train_status::Stamped::new(Some(est));
                    let delay = (est - scheduled_dt).num_minutes() as i32;
                    status.reported_delay_mins =
                        crate::types::train_status::Stamped::new(Some(delay));
                }

                status.last_update_source =
                    crate::types::train_status::UpdateSource::RestPoll;

                Ok(status)
            }
            429 => Err(GbrClientError::RateLimited),
            503 => Err(GbrClientError::ServiceUnavailable),
            _ => {
                let body = resp.text().await.unwrap_or_default();
                Err(GbrClientError::UnexpectedStatus { status: status_code, body })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Mock client (used in tests throughout the networking layer)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub mod mock {
    use super::*;
    use chrono::Utc;
    use std::sync::{Arc, Mutex};

    /// Configurable mock: returns a preset response or error for each call.
    pub struct MockGbrClient {
        /// If `Some`, returns this error on every call. If `None`, returns a stub status.
        pub force_error: Arc<Mutex<Option<GbrClientError>>>,
    }

    impl MockGbrClient {
        pub fn ok() -> Self {
            Self { force_error: Arc::new(Mutex::new(None)) }
        }

        pub fn failing(err: GbrClientError) -> Self {
            Self { force_error: Arc::new(Mutex::new(Some(err))) }
        }

        /// Replace the preset error (allows tests to change behaviour mid-run).
        pub fn set_error(&self, err: Option<GbrClientError>) {
            *self.force_error.lock().unwrap() = err;
        }
    }

    #[async_trait]
    impl GbrClient for MockGbrClient {
        async fn get_train_status(&self, rid: &TrainId) -> Result<TrainStatus, GbrClientError> {
            if let Some(ref e) = *self.force_error.lock().unwrap() {
                return Err(match e {
                    GbrClientError::ServiceUnavailable => GbrClientError::ServiceUnavailable,
                    GbrClientError::RateLimited => GbrClientError::RateLimited,
                    GbrClientError::NotFound(id) => GbrClientError::NotFound(id.clone()),
                    GbrClientError::Http(_) | GbrClientError::UnexpectedStatus { .. } => {
                        GbrClientError::ServiceUnavailable
                    }
                });
            }

            let now = Utc::now();
            Ok(TrainStatus::new(rid.clone(), now, now))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockGbrClient;
    use super::*;

    #[tokio::test]
    async fn mock_ok_returns_status() {
        let client = MockGbrClient::ok();
        let rid = TrainId::rid("202404170123456").unwrap();
        let result = client.get_train_status(&rid).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn mock_service_unavailable_propagates() {
        let client = MockGbrClient::failing(GbrClientError::ServiceUnavailable);
        let rid = TrainId::rid("202404170123456").unwrap();
        let result = client.get_train_status(&rid).await;
        assert!(matches!(result, Err(GbrClientError::ServiceUnavailable)));
    }
}
