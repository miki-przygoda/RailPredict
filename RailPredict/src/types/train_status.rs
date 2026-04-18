//! `TrainStatus` — the single source of truth for a UK rail service.
//!
//! Updated from three distinct sources:
//!   1. REST polling (GBR API)  — authoritative for seat availability and pricing
//!   2. STOMP firehose (Darwin) — authoritative for real-time movement and delay
//!   3. Prediction engine       — fills `predicted_delay_mins` when live data is stale
//!
//! Each time-varying field carries a `last_updated` timestamp so stale-data detection
//! can be applied per-field rather than per-record.
//!
//! ## Phase 2 (AdvancedAnalytics): `calling_points` added
//! Stores the full TIPLOC sequence for the service, populated from Darwin TS `<Location>`
//! elements. Used by `TrainRegistry`'s reverse TIPLOC index for cascade propagation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{TrainId, VolatilityContext};

/// A timestamped wrapper around any field that is updated from an external source.
/// Allows per-field staleness checks without embedding timestamps in business logic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stamped<T> {
    pub value: T,
    pub last_updated: DateTime<Utc>,
}

impl<T> Stamped<T> {
    pub fn new(value: T) -> Self {
        Self { value, last_updated: Utc::now() }
    }

    /// Returns `true` if this field is older than `max_age`.
    #[allow(dead_code)]
    pub fn is_stale(&self, max_age: chrono::Duration) -> bool {
        Utc::now() - self.last_updated > max_age
    }
}

/// The source that produced an update — tracked so consumers know provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateSource {
    RestPoll,
    StompFirehose,
    PredictionEngine,
}

/// Single source of truth for a UK rail service. Held behind `Arc<RwLock<TrainStatus>>`
/// in the train registry. All mutations are applied by a single writer task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainStatus {
    /// The primary identifier used to register this train; other IDs may be stored externally
    /// in the registry's lookup table.
    pub id: TrainId,

    // --- Departure times ---

    /// The timetabled departure time (from the CIF/GTFS schedule). Never changes once set.
    pub scheduled_departure: Stamped<DateTime<Utc>>,

    /// The time shown to passengers on departure boards. May differ from scheduled by a planned
    /// alteration (e.g. an advertised revised time before the day of travel).
    pub public_departure: Stamped<DateTime<Utc>>,

    /// The best current estimate of actual departure. Updated frequently from Darwin TS messages.
    pub actual_estimated_departure: Stamped<Option<DateTime<Utc>>>,

    // --- Delay ---

    /// Delay in minutes as reported by the last authoritative source.
    /// `None` until first live or predicted data arrives.
    pub reported_delay_mins: Stamped<Option<i32>>,

    /// Delay predicted by the local prediction engine from historical data.
    /// Used when `reported_delay_mins` is stale or absent.
    pub predicted_delay_mins: Stamped<Option<i32>>,

    // --- Platform ---

    /// Scheduled platform from timetable.
    pub scheduled_platform: Stamped<Option<String>>,

    /// Current confirmed or estimated platform from Darwin.
    pub actual_platform: Stamped<Option<String>>,

    // --- Cancellation ---

    pub is_cancelled: Stamped<Option<bool>>,
    pub cancellation_reason: Stamped<Option<String>>,

    // --- Origin station ---

    /// CRS code of the origin station, set from the first Darwin TS message.
    pub origin_crs: Option<String>,

    /// CRS code of the destination station, set from the last `<Location>` element in
    /// the Darwin TS message. `None` until the first multi-location TS message is parsed.
    pub destination_crs: Option<String>,

    /// RTTI UID (e.g. "C12345") — stable service identity used as the prediction key.
    /// `None` until the first Darwin TS message carrying a `uid` attribute is processed.
    pub uid: Option<String>,

    // --- Calling pattern (Phase 2 AdvancedAnalytics: TIPLOC cascade) ---

    /// Full ordered list of (TIPLOC code, scheduled arrival time) pairs for this service.
    /// Populated from Darwin TS `<Location>` elements as they arrive.
    /// Empty until the first TS message with location data is processed.
    /// Used by `TrainRegistry::update_tiploc_index` to maintain the reverse TIPLOC → TrainId map.
    pub calling_points: Vec<(String, DateTime<Utc>)>,

    // --- Environmental context ---

    pub volatility: VolatilityContext,

    // --- Provenance ---

    /// Source of the most recent overall status update.
    pub last_update_source: UpdateSource,
}

impl TrainStatus {
    /// Construct a fresh `TrainStatus` with only the scheduled times known.
    /// All live/predicted fields start as `None`.
    pub fn new(
        id: TrainId,
        scheduled_departure: DateTime<Utc>,
        public_departure: DateTime<Utc>,
    ) -> Self {
        let now = Utc::now();
        let scheduled_departure = Stamped { value: scheduled_departure, last_updated: now };
        let public_departure = Stamped { value: public_departure, last_updated: now };

        Self {
            id,
            scheduled_departure,
            public_departure,
            actual_estimated_departure: Stamped::new(None),
            reported_delay_mins: Stamped::new(None),
            predicted_delay_mins: Stamped::new(None),
            scheduled_platform: Stamped::new(None),
            actual_platform: Stamped::new(None),
            is_cancelled: Stamped::new(None),
            cancellation_reason: Stamped::new(None),
            origin_crs: None,
            destination_crs: None,
            uid: None,
            calling_points: Vec::new(),
            volatility: VolatilityContext::unknown(),
            last_update_source: UpdateSource::RestPoll,
        }
    }

    // Returns reported delay if known, otherwise the Tier B prediction.
    /// The best available delay estimate: live reported first, prediction as fallback.
    pub fn best_delay_mins(&self) -> Option<i32> {
        self.reported_delay_mins.value.or(self.predicted_delay_mins.value)
    }

    // Returns confirmed platform if known, otherwise the scheduled platform.
    /// The best available platform: confirmed/actual first, scheduled as fallback.
    pub fn best_platform(&self) -> Option<&str> {
        self.actual_platform
            .value
            .as_deref()
            .or(self.scheduled_platform.value.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_status() -> TrainStatus {
        let base = Utc::now();
        TrainStatus::new(
            TrainId::rid("202404170123456").unwrap(),
            base,
            base,
        )
    }

    #[test]
    fn new_status_has_no_delay() {
        assert!(make_status().best_delay_mins().is_none());
    }

    #[test]
    fn best_delay_prefers_reported() {
        let mut s = make_status();
        s.reported_delay_mins = Stamped::new(Some(5));
        s.predicted_delay_mins = Stamped::new(Some(10));
        assert_eq!(s.best_delay_mins(), Some(5));
    }

    #[test]
    fn best_delay_falls_back_to_predicted() {
        let mut s = make_status();
        s.predicted_delay_mins = Stamped::new(Some(7));
        assert_eq!(s.best_delay_mins(), Some(7));
    }

    #[test]
    fn best_platform_prefers_actual() {
        let mut s = make_status();
        s.scheduled_platform = Stamped::new(Some("3".to_string()));
        s.actual_platform = Stamped::new(Some("4A".to_string()));
        assert_eq!(s.best_platform(), Some("4A"));
    }

    #[test]
    fn best_platform_falls_back_to_scheduled() {
        let mut s = make_status();
        s.scheduled_platform = Stamped::new(Some("3".to_string()));
        assert_eq!(s.best_platform(), Some("3"));
    }

    #[test]
    fn new_status_is_not_cancelled() {
        assert_eq!(make_status().is_cancelled.value, None);
    }

    #[test]
    fn best_platform_none_when_both_absent() {
        assert!(make_status().best_platform().is_none());
    }

    #[test]
    fn stamped_is_stale_when_older_than_max_age() {
        use chrono::Duration;
        // Create a Stamped with a timestamp 2 minutes in the past.
        let mut s = Stamped::new(42u32);
        s.last_updated = Utc::now() - Duration::minutes(2);
        assert!(s.is_stale(Duration::minutes(1)));
    }

    #[test]
    fn stamped_not_stale_when_within_max_age() {
        use chrono::Duration;
        let s = Stamped::new(42u32);
        assert!(!s.is_stale(Duration::minutes(5)));
    }

    #[test]
    fn stamped_new_captures_current_time() {
        let before = Utc::now();
        let s = Stamped::new(0u32);
        let after = Utc::now();
        assert!(s.last_updated >= before && s.last_updated <= after);
    }
}
