//! Darwin feed freshness tracking for `/health` and the `darwin_feed_lag_seconds` gauge.
//!
//! The ingestion pipeline stamps [`FeedHealth::record_message`] on every STOMP
//! `MESSAGE` frame it receives (before filtering/parsing, so a quiet watched-route
//! filter does not look like a dead feed). The HTTP layer reads it via
//! [`FeedHealth::evaluate`] to decide whether ingestion is alive.
//!
//! Lock-free: a single `AtomicI64` of unix-millis, written from the ingestion task
//! and read from request handlers. `0` means "no message received yet".
//!
//! The tracker lives on `PipelineContext` (`Arc`-shared), so it survives STOMP
//! reconnects just like the registry.

use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{DateTime, TimeZone, Utc};

/// Default staleness threshold. Darwin normally delivers several messages per
/// second; 5 minutes of silence comfortably covers a reconnect with the maximum
/// 120 s backoff while still flagging a dead feed quickly.
pub const DEFAULT_FEED_STALE_AFTER_SECS: u64 = 300;

/// Shared freshness tracker for the Darwin Push Port feed.
#[derive(Debug)]
pub struct FeedHealth {
    /// `false` when Darwin credentials are not configured (ingestion disabled on
    /// purpose). A disabled feed is reported as such, not as a failure.
    enabled: bool,
    /// Process start (unix-millis): the reference point before the first message.
    started_at_ms: i64,
    /// Unix-millis of the most recent Darwin message; `0` = none yet.
    last_message_ms: AtomicI64,
    /// Silence longer than this marks the feed stale.
    stale_after_secs: u64,
}

/// Result of a freshness check at a given instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FeedState {
    /// Ingestion is intentionally disabled (no Darwin credentials).
    Disabled,
    /// No message yet, but still within the startup grace period.
    Starting { secs_since_start: f64 },
    /// Last message is within the threshold.
    Fresh { lag_secs: f64 },
    /// No message within the threshold (either ever, or since the last one).
    Stale { lag_secs: f64 },
}

impl FeedState {
    /// `true` when `/health` should report the service as degraded.
    pub fn is_unhealthy(&self) -> bool {
        matches!(self, FeedState::Stale { .. })
    }

    /// Short machine-readable label for the health JSON.
    pub fn label(&self) -> &'static str {
        match self {
            FeedState::Disabled => "disabled",
            FeedState::Starting { .. } => "starting",
            FeedState::Fresh { .. } => "fresh",
            FeedState::Stale { .. } => "stale",
        }
    }

    /// Seconds since the last message (or since startup if none yet); `None` when disabled.
    pub fn lag_secs(&self) -> Option<f64> {
        match *self {
            FeedState::Disabled => None,
            FeedState::Starting { secs_since_start } => Some(secs_since_start),
            FeedState::Fresh { lag_secs } | FeedState::Stale { lag_secs } => Some(lag_secs),
        }
    }
}

impl FeedHealth {
    /// Tracker for an enabled feed, starting its grace period now.
    pub fn new(stale_after_secs: u64) -> Self {
        Self::new_at(stale_after_secs, Utc::now())
    }

    /// Tracker for an enabled feed with an explicit start time (tests).
    pub fn new_at(stale_after_secs: u64, started_at: DateTime<Utc>) -> Self {
        Self {
            enabled: true,
            started_at_ms: started_at.timestamp_millis(),
            last_message_ms: AtomicI64::new(0),
            stale_after_secs,
        }
    }

    /// Tracker for a deliberately disabled feed (no Darwin credentials).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            started_at_ms: Utc::now().timestamp_millis(),
            last_message_ms: AtomicI64::new(0),
            stale_after_secs: DEFAULT_FEED_STALE_AFTER_SECS,
        }
    }

    /// Stamp "a Darwin message arrived now". Called once per received frame.
    pub fn record_message(&self) {
        self.record_message_at(Utc::now());
    }

    /// Stamp a message arrival at an explicit time (tests).
    pub fn record_message_at(&self, at: DateTime<Utc>) {
        self.last_message_ms.fetch_max(at.timestamp_millis(), Ordering::Relaxed);
    }

    /// Time of the most recent Darwin message, if any.
    pub fn last_message_at(&self) -> Option<DateTime<Utc>> {
        match self.last_message_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Utc.timestamp_millis_opt(ms).single(),
        }
    }

    pub fn stale_after_secs(&self) -> u64 {
        self.stale_after_secs
    }

    /// Classify the feed at `now`.
    pub fn evaluate(&self, now: DateTime<Utc>) -> FeedState {
        if !self.enabled {
            return FeedState::Disabled;
        }
        let now_ms = now.timestamp_millis();
        let threshold = self.stale_after_secs as f64;
        match self.last_message_ms.load(Ordering::Relaxed) {
            0 => {
                let secs = (now_ms - self.started_at_ms).max(0) as f64 / 1000.0;
                if secs > threshold {
                    FeedState::Stale { lag_secs: secs }
                } else {
                    FeedState::Starting { secs_since_start: secs }
                }
            }
            last => {
                let lag = (now_ms - last).max(0) as f64 / 1000.0;
                if lag > threshold {
                    FeedState::Stale { lag_secs: lag }
                } else {
                    FeedState::Fresh { lag_secs: lag }
                }
            }
        }
    }

    /// Publish the current lag to the `darwin_feed_lag_seconds` Prometheus gauge.
    /// No-op for a disabled feed. Returns the evaluated state.
    pub fn publish_gauge(&self, now: DateTime<Utc>) -> FeedState {
        let state = self.evaluate(now);
        if let Some(lag) = state.lag_secs() {
            metrics::gauge!("darwin_feed_lag_seconds").set(lag);
        }
        state
    }
}

impl Default for FeedHealth {
    fn default() -> Self {
        Self::new(DEFAULT_FEED_STALE_AFTER_SECS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap()
    }

    #[test]
    fn disabled_feed_is_never_unhealthy() {
        let fh = FeedHealth::disabled();
        let s = fh.evaluate(Utc::now() + Duration::days(1));
        assert_eq!(s, FeedState::Disabled);
        assert!(!s.is_unhealthy());
        assert_eq!(s.lag_secs(), None);
    }

    #[test]
    fn no_message_within_grace_is_starting() {
        let fh = FeedHealth::new_at(300, t0());
        let s = fh.evaluate(t0() + Duration::seconds(60));
        assert_eq!(s.label(), "starting");
        assert!(!s.is_unhealthy());
    }

    #[test]
    fn no_message_after_grace_is_stale() {
        let fh = FeedHealth::new_at(300, t0());
        let s = fh.evaluate(t0() + Duration::seconds(301));
        assert_eq!(s.label(), "stale");
        assert!(s.is_unhealthy());
    }

    #[test]
    fn recent_message_is_fresh_and_old_message_is_stale() {
        let fh = FeedHealth::new_at(300, t0());
        fh.record_message_at(t0() + Duration::seconds(10));
        let fresh = fh.evaluate(t0() + Duration::seconds(70));
        assert_eq!(fresh, FeedState::Fresh { lag_secs: 60.0 });
        assert!(!fresh.is_unhealthy());

        // Feed goes silent: 10 minutes after the last message it must be stale.
        let stale = fh.evaluate(t0() + Duration::seconds(610));
        assert_eq!(stale, FeedState::Stale { lag_secs: 600.0 });
        assert!(stale.is_unhealthy());
    }

    #[test]
    fn publish_gauge_exports_feed_lag() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let fh = FeedHealth::new_at(300, t0());
        fh.record_message_at(t0());
        metrics::with_local_recorder(&recorder, || {
            fh.publish_gauge(t0() + Duration::seconds(42));
        });
        let rendered = handle.render();
        assert!(
            rendered.contains("darwin_feed_lag_seconds 42"),
            "gauge missing from scrape output: {rendered}"
        );
    }

    #[test]
    fn late_stamp_never_moves_last_message_backwards() {
        let fh = FeedHealth::new_at(300, t0());
        fh.record_message_at(t0() + Duration::seconds(100));
        fh.record_message_at(t0() + Duration::seconds(50));
        assert_eq!(fh.last_message_at(), Some(t0() + Duration::seconds(100)));
    }
}
