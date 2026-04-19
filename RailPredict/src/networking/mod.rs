//! Networking layer — all outbound GBR API calls go through here.
//!
//! Components:
//!   - `GbrClient` trait + `LiveGbrClient` concrete impl (requires credentials)
//!   - `Coalescer` — deduplicates in-flight requests via `oneshot` fan-out
//!   - `RateLimiter` — token-bucket throttle in front of all outbound calls
//!   - `CircuitBreaker` — 503 detection; enters Cache Only mode on GBR failure

pub mod circuit_breaker;
pub mod coalescer;
pub mod gbr_client;
pub mod rate_limiter;

pub use circuit_breaker::{CircuitBreaker, CircuitBreakerState};
pub use coalescer::Coalescer;
pub use gbr_client::{GbrClient, GbrClientError, LiveGbrClient};
pub use rate_limiter::RateLimiter;
