//! Request coalescer — the "Waiter" pattern.
//!
//! If N callers request the same `TrainId` while a fetch is already in flight, only
//! one HTTP request is made. Each caller gets a `oneshot::Receiver`; when the response
//! arrives the first responder fans the result to all waiters simultaneously.
//!
//! ## Why `oneshot` not `broadcast`
//! `oneshot` is a single-use, one-sender / one-receiver channel. Each waiting caller
//! gets their own `oneshot::Receiver`, and the completing task sends to all of them.
//! `broadcast` would require all receivers to be registered before the send and would
//! clone the value N times regardless — `oneshot` fan-out is more explicit and avoids
//! the "lagged receiver" problem.
//!
//! ## Data structure
//! `Mutex<HashMap<TrainId, Vec<oneshot::Sender<Result<TrainStatus, GbrClientError>>>>>>`
//!
//! Presence of a key means a fetch is already in flight. New arrivals append their
//! sender to the vec. The task that initiated the fetch removes the key and fans out.
//! Lock is held only briefly (map read/write), never across an await.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};

use crate::types::{TrainId, TrainStatus};

use super::gbr_client::{GbrClient, GbrClientError, GbrErrorKind};

type Waiters = Vec<oneshot::Sender<Result<TrainStatus, CoalescerError>>>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum CoalescerError {
    /// A GBR call failed. `kind` carries the breaker-routing classification
    /// (the underlying `GbrClientError` isn't `Clone`); `detail` is for logging.
    #[error("GBR request failed [{kind:?}]: {detail}")]
    Gbr { kind: GbrErrorKind, detail: String },
    #[error("In-flight request was dropped before completing")]
    InFlightDropped,
}

impl From<GbrClientError> for CoalescerError {
    fn from(e: GbrClientError) -> Self {
        Self::Gbr { kind: e.kind(), detail: e.to_string() }
    }
}

pub struct Coalescer {
    in_flight: Arc<Mutex<HashMap<TrainId, Waiters>>>,
    client: Arc<dyn GbrClient>,
}

impl Coalescer {
    pub fn new(client: Arc<dyn GbrClient>) -> Self {
        Self {
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            client,
        }
    }

    /// Request the status for `train_id`. If a fetch is already in flight for this ID,
    /// the caller joins the waiter list and receives the result when it arrives.
    /// Otherwise, a new fetch is initiated.
    pub async fn get(
        &self,
        train_id: TrainId,
    ) -> Result<TrainStatus, CoalescerError> {
        let (tx, rx) = oneshot::channel();

        let should_fetch = {
            let mut map = self.in_flight.lock().await;
            let entry = map.entry(train_id.clone()).or_default();
            let is_first = entry.is_empty();
            entry.push(tx);
            is_first
        };

        if should_fetch {
            // We are the designated fetcher for this train_id.
            let in_flight = Arc::clone(&self.in_flight);
            let client = Arc::clone(&self.client);
            let id = train_id.clone();

            tokio::spawn(async move {
                let result = client.get_train_status(&id).await;

                // Remove the entry and collect all waiters atomically.
                let waiters = {
                    let mut map = in_flight.lock().await;
                    map.remove(&id).unwrap_or_default()
                };

                // Fan out — each waiter gets their own send.
                for tx in waiters {
                    let payload = match &result {
                        Ok(status) => Ok(status.clone()),
                        Err(e) => Err(CoalescerError::Gbr { kind: e.kind(), detail: e.to_string() }),
                    };
                    // Ignore send errors: receiver may have timed out or been dropped.
                    let _ = tx.send(payload);
                }
            });
        }

        rx.await.map_err(|_| CoalescerError::InFlightDropped)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::networking::gbr_client::mock::MockGbrClient;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A mock that counts how many times `get_train_status` is called.
    struct CountingMock {
        inner: MockGbrClient,
        call_count: Arc<AtomicU32>,
    }

    impl CountingMock {
        fn new() -> (Self, Arc<AtomicU32>) {
            let count = Arc::new(AtomicU32::new(0));
            (
                Self { inner: MockGbrClient::ok(), call_count: Arc::clone(&count) },
                count,
            )
        }
    }

    #[async_trait::async_trait]
    impl GbrClient for CountingMock {
        async fn get_train_status(
            &self,
            rid: &TrainId,
        ) -> Result<TrainStatus, GbrClientError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            // Small delay to allow concurrent requests to arrive before the first completes.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            self.inner.get_train_status(rid).await
        }
    }

    #[tokio::test]
    async fn single_request_succeeds() {
        let client = Arc::new(MockGbrClient::ok());
        let coalescer = Coalescer::new(client);
        let rid = TrainId::rid("202404170123456").unwrap();
        let result = coalescer.get(rid).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn concurrent_requests_coalesced_into_one_http_call() {
        let (mock, call_count) = CountingMock::new();
        let coalescer = Arc::new(Coalescer::new(Arc::new(mock)));
        let rid = TrainId::rid("202404170123456").unwrap();

        // Launch 5 concurrent requests for the same train.
        let handles: Vec<_> = (0..5)
            .map(|_| {
                let c = Arc::clone(&coalescer);
                let id = rid.clone();
                tokio::spawn(async move { c.get(id).await })
            })
            .collect();

        for h in handles {
            assert!(h.await.unwrap().is_ok());
        }

        // Only 1 HTTP call should have been made despite 5 callers.
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn gbr_errors_classify_for_breaker_routing() {
        use GbrErrorKind::*;
        assert_eq!(GbrClientError::ServiceUnavailable.kind(), Unavailable);
        assert_eq!(GbrClientError::RateLimited.kind(), RateLimited);
        assert_eq!(
            GbrClientError::UnexpectedStatus { status: 502, body: String::new() }.kind(),
            ServerError
        );
        assert_eq!(
            GbrClientError::UnexpectedStatus { status: 404, body: String::new() }.kind(),
            ClientError
        );
        // The breaker must count brownouts (503/transport/5xx) but NOT rate-limits or 4xx.
        assert!(Unavailable.is_breaker_failure());
        assert!(Transport.is_breaker_failure());
        assert!(ServerError.is_breaker_failure());
        assert!(!RateLimited.is_breaker_failure());
        assert!(!ClientError.is_breaker_failure());
    }

    #[tokio::test]
    async fn error_propagates_to_all_waiters() {
        let mock = MockGbrClient::failing(GbrClientError::ServiceUnavailable);
        let coalescer = Arc::new(Coalescer::new(Arc::new(mock)));
        let rid = TrainId::rid("202404170123456").unwrap();

        let handles: Vec<_> = (0..3)
            .map(|_| {
                let c = Arc::clone(&coalescer);
                let id = rid.clone();
                tokio::spawn(async move { c.get(id).await })
            })
            .collect();

        for h in handles {
            assert!(h.await.unwrap().is_err());
        }
    }
}
