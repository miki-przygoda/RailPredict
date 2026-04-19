//! Circuit breaker for GBR API calls.
//!
//! ## State machine
//!
//! ```text
//!  Closed ──(N consecutive 503s)──> Open ──(cool-down elapsed)──> HalfOpen
//!    ▲                                                                 │
//!    └──────────────(probe request succeeds)──────────────────────────┘
//!                                 │
//!              (probe request fails)──> Open (reset timer)
//! ```
//!
//! ## Transitions
//! - `Closed → Open`:    `FAILURE_THRESHOLD` consecutive 503/unavailable responses.
//! - `Open → HalfOpen`:  `COOL_DOWN_SECS` seconds have elapsed since the breaker opened.
//! - `HalfOpen → Closed`: the single probe request succeeds.
//! - `HalfOpen → Open`:  the probe request fails; reset the cool-down timer.
//!
//! ## Constants
//! - `FAILURE_THRESHOLD = 3`: three consecutive failures before opening. One transient
//!   error should not block the system; three suggests a real GBR outage.
//! - `COOL_DOWN_SECS = 30`: GBR outages typically resolve within seconds to minutes.
//!   30 seconds balances recovery speed against hammering a struggling upstream.

use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const FAILURE_THRESHOLD: u32 = 3;
const COOL_DOWN_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitBreakerState {
    /// Normal operation — requests flow through.
    Closed,
    /// GBR is unavailable — all requests are blocked immediately.
    Open,
    /// Cool-down elapsed — one probe request is allowed through to test recovery.
    HalfOpen,
}

impl std::fmt::Display for CircuitBreakerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "Closed"),
            Self::Open => write!(f, "Open"),
            Self::HalfOpen => write!(f, "HalfOpen"),
        }
    }
}

struct CircuitBreakerInner {
    state: CircuitBreakerState,
    consecutive_failures: u32,
    opened_at: Option<Instant>,
}

pub struct CircuitBreaker {
    inner: Mutex<CircuitBreakerInner>,
    failure_threshold: u32,
    cool_down: Duration,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, cool_down: Duration) -> Self {
        Self {
            inner: Mutex::new(CircuitBreakerInner {
                state: CircuitBreakerState::Closed,
                consecutive_failures: 0,
                opened_at: None,
            }),
            failure_threshold,
            cool_down,
        }
    }

    pub fn default_gbr() -> Self {
        Self::new(FAILURE_THRESHOLD, Duration::from_secs(COOL_DOWN_SECS))
    }

    /// Returns `true` if a request is currently allowed through.
    pub async fn is_request_allowed(&self) -> bool {
        let mut inner = self.inner.lock().await;
        match inner.state {
            CircuitBreakerState::Closed => true,
            CircuitBreakerState::Open => {
                // Check if cool-down has elapsed and promote to HalfOpen.
                if inner.opened_at.is_some_and(|t| t.elapsed() >= self.cool_down) {
                    inner.state = CircuitBreakerState::HalfOpen;
                    // Phase 2: record state transition to HalfOpen.
                    metrics::gauge!("circuit_breaker_state", "state" => "HalfOpen").set(1.0);
                    metrics::gauge!("circuit_breaker_state", "state" => "Open").set(0.0);
                    metrics::gauge!("circuit_breaker_state", "state" => "Closed").set(0.0);
                    true // allow the single probe request
                } else {
                    // Phase 2: count requests blocked by an Open breaker.
                    metrics::counter!("circuit_breaker_blocked_total").increment(1);
                    false
                }
            }
            CircuitBreakerState::HalfOpen => true, // probe already in flight
        }
    }

    /// Call on a successful response. Resets failure count; closes the breaker if HalfOpen.
    pub async fn record_success(&self) {
        let mut inner = self.inner.lock().await;
        inner.consecutive_failures = 0;
        let prev = inner.state;
        inner.state = CircuitBreakerState::Closed;
        inner.opened_at = None;
        // Phase 2: record state transition to Closed if we were in a non-Closed state.
        if prev != CircuitBreakerState::Closed {
            metrics::gauge!("circuit_breaker_state", "state" => "Closed").set(1.0);
            metrics::gauge!("circuit_breaker_state", "state" => "Open").set(0.0);
            metrics::gauge!("circuit_breaker_state", "state" => "HalfOpen").set(0.0);
        }
    }

    /// Call on a 503 / unavailable response. Opens the breaker after threshold failures.
    ///
    /// `#[cold]` biases the branch predictor in callers toward the not-taken (success)
    /// direction — recommended by CLAUDE.md pattern #8 for rare error paths.
    #[cold]
    pub async fn record_failure(&self) {
        let mut inner = self.inner.lock().await;
        inner.consecutive_failures += 1;

        if inner.consecutive_failures >= self.failure_threshold
            || inner.state == CircuitBreakerState::HalfOpen
        {
            let prev = inner.state;
            // HalfOpen probe failed: re-open and reset timer.
            inner.state = CircuitBreakerState::Open;
            inner.opened_at = Some(Instant::now());
            // Phase 2: record state transition to Open.
            if prev != CircuitBreakerState::Open {
                metrics::gauge!("circuit_breaker_state", "state" => "Open").set(1.0);
                metrics::gauge!("circuit_breaker_state", "state" => "Closed").set(0.0);
                metrics::gauge!("circuit_breaker_state", "state" => "HalfOpen").set(0.0);
            }
        }
    }

    pub async fn state(&self) -> CircuitBreakerState {
        self.inner.lock().await.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn starts_closed() {
        let cb = CircuitBreaker::default_gbr();
        assert_eq!(cb.state().await, CircuitBreakerState::Closed);
        assert!(cb.is_request_allowed().await);
    }

    #[tokio::test]
    async fn opens_after_threshold_failures() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(60));
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitBreakerState::Closed);
        cb.record_failure().await; // threshold hit
        assert_eq!(cb.state().await, CircuitBreakerState::Open);
        assert!(!cb.is_request_allowed().await);
    }

    #[tokio::test]
    async fn transitions_to_half_open_after_cool_down() {
        // 1ms cool-down for test speed
        let cb = CircuitBreaker::new(1, Duration::from_millis(1));
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitBreakerState::Open);

        tokio::time::sleep(Duration::from_millis(5)).await;
        let allowed = cb.is_request_allowed().await;
        assert!(allowed);
        assert_eq!(cb.state().await, CircuitBreakerState::HalfOpen);
    }

    #[tokio::test]
    async fn success_in_half_open_closes_breaker() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(1));
        cb.record_failure().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        cb.is_request_allowed().await; // moves to HalfOpen
        cb.record_success().await;
        assert_eq!(cb.state().await, CircuitBreakerState::Closed);
    }

    #[tokio::test]
    async fn failure_in_half_open_reopens_breaker() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(1));
        cb.record_failure().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        cb.is_request_allowed().await; // moves to HalfOpen
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitBreakerState::Open);
    }

    #[tokio::test]
    async fn success_resets_failure_count() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(60));
        cb.record_failure().await;
        cb.record_failure().await;
        cb.record_success().await;
        // Should be closed and failure count reset — one more failure should not open it
        cb.record_failure().await;
        assert_eq!(cb.state().await, CircuitBreakerState::Closed);
    }
}
