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
//! | `ENDPOINT_DEPARTURES`     | GET    | `/v1/station/{crs}/departures`                | Departure board for a station  |
//!
//! Response shapes are defined as `serde` structs below. If GBR changes its schema,
//! update here; nothing else in the codebase should parse raw GBR JSON.

use async_trait::async_trait;
use thiserror::Error;

use crate::types::{TrainId, TrainStatus};

// ---------------------------------------------------------------------------
// Endpoint constants
// ---------------------------------------------------------------------------

/// Base URL for the GBR Retail API. Override via `GBR_API_BASE_URL` env var in tests.
#[allow(dead_code)]
pub const GBR_API_BASE_URL: &str = "https://api.rtt.io/api";

#[allow(dead_code)]
pub const ENDPOINT_TRAIN_STATUS: &str = "/v1/train/{rid}/status";
#[allow(dead_code)]
pub const ENDPOINT_DEPARTURES: &str = "/v1/station/{crs}/departures";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
#[allow(dead_code)]
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

#[allow(dead_code)]
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

        let resp = self
            .http
            .get(&url)
            .header("x-apikey", &self.api_key)
            .send()
            .await?;

        match resp.status().as_u16() {
            200 => {
                // TODO: parse GBR JSON into TrainStatus once Darwin credentials arrive
                // and response schema is confirmed. For now return NotFound to signal
                // the caller should fall back to cache.
                Err(GbrClientError::NotFound(rid.clone()))
            }
            429 => Err(GbrClientError::RateLimited),
            503 => Err(GbrClientError::ServiceUnavailable),
            status => {
                let body = resp.text().await.unwrap_or_default();
                Err(GbrClientError::UnexpectedStatus { status, body })
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
