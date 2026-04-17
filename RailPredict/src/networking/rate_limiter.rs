//! Token-bucket rate limiter for outbound GBR API calls.
//!
//! ## Rate limit values
//! GBR does not publish a hard rate limit in its public documentation.
//! `MAX_REQUESTS_PER_SECOND = 10` is a conservative safe value based on typical
//! rail data API policies. `BURST_CAPACITY = 20` allows short bursts (e.g. startup
//! warm-up) without triggering server-side throttling.
//!
//! If GBR publishes an official limit or if 429 responses are observed in production,
//! adjust these constants and redeploy — no logic changes required.
//!
//! ## Algorithm
//! Classic token bucket: tokens accumulate at `refill_rate` per second up to `capacity`.
//! Each request consumes one token. If the bucket is empty, the caller awaits until
//! enough tokens have refilled. This provides smooth throughput with burst tolerance.

use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

/// Conservative request rate based on typical rail data API policies.
#[allow(dead_code)]
pub const MAX_REQUESTS_PER_SECOND: u32 = 10;
/// Burst headroom for startup warm-up or short request spikes.
#[allow(dead_code)]
pub const BURST_CAPACITY: u32 = 20;

pub struct RateLimiter {
    inner: Mutex<RateLimiterInner>,
}

struct RateLimiterInner {
    tokens: f64,
    capacity: f64,
    refill_rate: f64, // tokens per nanosecond
    last_refill: Instant,
}

impl RateLimiter {
    pub fn new(max_per_second: u32, burst_capacity: u32) -> Self {
        Self {
            inner: Mutex::new(RateLimiterInner {
                tokens: burst_capacity as f64,
                capacity: burst_capacity as f64,
                refill_rate: max_per_second as f64 / 1_000_000_000.0,
                last_refill: Instant::now(),
            }),
        }
    }

    /// Default instance using the published constants.
    #[allow(dead_code)]
    pub fn default_gbr() -> Self {
        Self::new(MAX_REQUESTS_PER_SECOND, BURST_CAPACITY)
    }

    /// Acquire one token, sleeping if necessary until one is available.
    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut inner = self.inner.lock().await;
                inner.refill();

                if inner.tokens >= 1.0 {
                    inner.tokens -= 1.0;
                    None
                } else {
                    // How long until 1 token refills?
                    let deficit = 1.0 - inner.tokens;
                    let nanos = (deficit / inner.refill_rate) as u64;
                    Some(Duration::from_nanos(nanos))
                }
            };

            match wait {
                None => return,
                Some(d) => tokio::time::sleep(d).await,
            }
        }
    }

    /// Returns the current token count (for diagnostics / tests).
    pub async fn available_tokens(&self) -> f64 {
        let mut inner = self.inner.lock().await;
        inner.refill();
        inner.tokens
    }
}

impl RateLimiterInner {
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed_nanos = (now - self.last_refill).as_nanos() as f64;
        let new_tokens = elapsed_nanos * self.refill_rate;
        self.tokens = (self.tokens + new_tokens).min(self.capacity);
        self.last_refill = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn starts_with_full_burst_capacity() {
        let rl = RateLimiter::new(10, 5);
        let tokens = rl.available_tokens().await;
        assert!((tokens - 5.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn acquire_decrements_tokens() {
        let rl = RateLimiter::new(10, 5);
        rl.acquire().await;
        let tokens = rl.available_tokens().await;
        assert!(tokens < 5.0);
    }

    #[tokio::test]
    async fn acquire_many_within_burst() {
        let rl = RateLimiter::new(10, 5);
        // 5 acquires should all succeed without sleeping (burst capacity = 5)
        for _ in 0..5 {
            rl.acquire().await;
        }
        let tokens = rl.available_tokens().await;
        assert!(tokens < 1.0);
    }

    #[tokio::test]
    async fn tokens_refill_over_time() {
        let rl = RateLimiter::new(1000, 1); // 1000/s refill, burst 1
        rl.acquire().await; // drain the 1 token
        tokio::time::sleep(Duration::from_millis(5)).await; // should refill ~5 tokens
        let tokens = rl.available_tokens().await;
        assert!(tokens >= 1.0, "expected refill, got {tokens}");
    }
}
