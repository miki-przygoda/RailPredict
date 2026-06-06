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
//! ## Full-Journey Capture: the `journey` accumulator
//! The primary analytics structure is `journey: BTreeMap<u16, CallObservation>` — the whole
//! per-call journey, accumulated across `schedule` + partial TS messages and keyed by a stable
//! per-TIPLOC `seq` (`tpl_seq`). Each `CallObservation` merges sticky (never overwrite a known
//! actual/cancel with a later null); on deactivation the journey is snapshotted to the
//! `journeys` / `journey_calls` tables. `toc` / `train_category` / reason codes ride alongside.
//!
//! The older `calling_points: Vec<(TIPLOC, scheduled_arr)>` field is retained separately for
//! `TrainRegistry`'s reverse TIPLOC index (cascade propagation, Tier-C-staged).

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{TrainId, VolatilityContext};

/// A timestamped wrapper around any field that is updated from an external source.
/// Allows per-field staleness checks without embedding timestamps in business logic.
///
/// The `version` field encodes source priority to resolve write conflicts atomically
/// inside the registry write lock (eliminating the TOCTOU window between check and write):
///   - Darwin firehose: `msg_ts.timestamp_millis() as u64` (~1.75 × 10¹²) — always wins
///   - GBR REST poll:   monotonic `AtomicU64` counter (1, 2, 3 …)
///   - Prediction engine: 0 — lowest priority, only fills gaps
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stamped<T> {
    pub value: T,
    pub last_updated: DateTime<Utc>,
    pub version: u64,
}

impl<T> Stamped<T> {
    pub fn new(value: T) -> Self {
        Self { value, last_updated: Utc::now(), version: 0 }
    }

    /// Construct with an explicit source version for priority-ordered writes.
    pub fn with_version(value: T, version: u64) -> Self {
        Self { value, last_updated: Utc::now(), version }
    }

    /// Returns `true` if this field is older than `max_age`.
    #[allow(dead_code)]
    pub fn is_stale(&self, max_age: chrono::Duration) -> bool {
        Utc::now() - self.last_updated > max_age
    }
}

impl<T: Clone> Stamped<T> {
    /// Replace `self` with `incoming` iff `incoming.version > self.version`.
    /// Call inside the registry write lock — the check-and-write is atomic in that context.
    pub fn apply_if_newer(&mut self, incoming: Stamped<T>) {
        if incoming.version > self.version {
            *self = incoming;
        }
    }
}

/// The source that produced an update — tracked so consumers know provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateSource {
    RestPoll,
    StompFirehose,
    PredictionEngine,
}

/// One accumulated calling point of a service's journey (Full-Journey Capture).
///
/// Built up across the train's life from Darwin `schedule` (planned times + activity) and
/// `TS` (live estimated/actual times, platform, per-stop cancel) messages. Stored in
/// `TrainStatus::journey` keyed by a stable per-TIPLOC `seq`, then snapshotted to the
/// `journeys` / `journey_calls` tables when the train deactivates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallObservation {
    pub tpl: String,
    pub seq: u16,
    pub sched_arr: Option<DateTime<Utc>>,
    pub sched_dep: Option<DateTime<Utc>>,
    pub est_arr: Option<DateTime<Utc>>,
    pub act_arr: Option<DateTime<Utc>>,
    pub est_dep: Option<DateTime<Utc>>,
    pub act_dep: Option<DateTime<Utc>>,
    pub platform: Option<String>,
    pub plat_confirmed: Option<bool>,
    pub is_cancelled: bool,
    /// Planned activity codes from `schedule` (e.g. "T" stop, "R" request, "U" set-down).
    pub activity: Option<String>,
}

impl CallObservation {
    pub fn new(tpl: String, seq: u16) -> Self {
        Self {
            tpl,
            seq,
            sched_arr: None,
            sched_dep: None,
            est_arr: None,
            act_arr: None,
            est_dep: None,
            act_dep: None,
            platform: None,
            plat_confirmed: None,
            is_cancelled: false,
            activity: None,
        }
    }

    /// Merge a newer observation of the same stop in: fill any non-null field from `inc`,
    /// never nulling an existing value. Darwin `TS` messages are partial, so a later message
    /// may only refine one stop's forecast. Cancellation is sticky.
    pub fn merge_from(&mut self, inc: &CallObservation) {
        if inc.sched_arr.is_some() {
            self.sched_arr = inc.sched_arr;
        }
        if inc.sched_dep.is_some() {
            self.sched_dep = inc.sched_dep;
        }
        if inc.est_arr.is_some() {
            self.est_arr = inc.est_arr;
        }
        if inc.act_arr.is_some() {
            self.act_arr = inc.act_arr;
        }
        if inc.est_dep.is_some() {
            self.est_dep = inc.est_dep;
        }
        if inc.act_dep.is_some() {
            self.act_dep = inc.act_dep;
        }
        if inc.platform.is_some() {
            self.platform = inc.platform.clone();
        }
        if inc.plat_confirmed.is_some() {
            self.plat_confirmed = inc.plat_confirmed;
        }
        if inc.activity.is_some() {
            self.activity = inc.activity.clone();
        }
        if inc.is_cancelled {
            self.is_cancelled = true;
        }
    }
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

    /// Working timetable departure time (`wtd` from Darwin TS). Internal schedule
    /// with engineering margins. `None` until first Darwin TS message with `wtd` is seen.
    pub working_departure: Option<DateTime<Utc>>,

    // --- Cancellation ---

    pub is_cancelled: Stamped<Option<bool>>,

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

    // --- Full-Journey Capture ---

    /// Operating company code (`toc`) from the Darwin `schedule` message. `None` until seen.
    pub toc: Option<String>,

    /// Train category (express / stopper / freight) from `schedule`. `None` until seen.
    pub train_category: Option<String>,

    /// The whole journey, accumulated across `schedule` + `TS` messages and keyed by a stable
    /// per-TIPLOC `seq`. Snapshotted to `journeys` / `journey_calls` on deactivation.
    #[serde(default)]
    pub journey: BTreeMap<u16, CallObservation>,

    /// First-seen TIPLOC → canonical `seq`, so partial TS messages reconcile to one ordering.
    #[serde(default)]
    pub tpl_seq: HashMap<String, u16>,

    /// Latest late-running reason code + the TIPLOC it was attributed to (Darwin reason codes).
    pub late_reason_code: Option<i32>,
    pub cancel_reason_code: Option<i32>,
    pub reason_tiploc: Option<String>,

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
        let scheduled_departure = Stamped { value: scheduled_departure, last_updated: now, version: 0 };
        let public_departure = Stamped { value: public_departure, last_updated: now, version: 0 };

        Self {
            id,
            scheduled_departure,
            public_departure,
            actual_estimated_departure: Stamped::new(None),
            reported_delay_mins: Stamped::new(None),
            predicted_delay_mins: Stamped::new(None),
            scheduled_platform: Stamped::new(None),
            actual_platform: Stamped::new(None),
            working_departure: None,
            is_cancelled: Stamped::new(None),
            origin_crs: None,
            destination_crs: None,
            uid: None,
            calling_points: Vec::new(),
            toc: None,
            train_category: None,
            journey: BTreeMap::new(),
            tpl_seq: HashMap::new(),
            late_reason_code: None,
            cancel_reason_code: None,
            reason_tiploc: None,
            volatility: VolatilityContext::unknown(),
            last_update_source: UpdateSource::RestPoll,
        }
    }

    /// Fold one parsed calling point into the accumulated `journey`, merging with any prior
    /// observation of the same TIPLOC. The incoming `seq` is advisory — the canonical `seq`
    /// is resolved (and assigned, first-seen) via `tpl_seq` so partial messages stay aligned.
    pub fn apply_call(&mut self, incoming: CallObservation) {
        if incoming.tpl.is_empty() {
            return;
        }
        let next = self.tpl_seq.len() as u16;
        let seq = *self.tpl_seq.entry(incoming.tpl.clone()).or_insert(next);
        let mut resolved = incoming;
        resolved.seq = seq;
        self.journey
            .entry(seq)
            .or_insert_with(|| CallObservation::new(resolved.tpl.clone(), seq))
            .merge_from(&resolved);
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

    #[test]
    fn apply_if_newer_overwrites_when_version_higher() {
        let mut existing = Stamped::with_version(1u32, 5);
        let incoming = Stamped::with_version(2u32, 10);
        existing.apply_if_newer(incoming);
        assert_eq!(existing.value, 2);
        assert_eq!(existing.version, 10);
    }

    #[test]
    fn apply_if_newer_rejects_when_version_lower() {
        let mut existing = Stamped::with_version(1u32, 10);
        let incoming = Stamped::with_version(2u32, 5);
        existing.apply_if_newer(incoming);
        assert_eq!(existing.value, 1);
        assert_eq!(existing.version, 10);
    }

    #[test]
    fn apply_if_newer_rejects_when_version_equal() {
        let mut existing = Stamped::with_version(1u32, 7);
        let incoming = Stamped::with_version(2u32, 7);
        existing.apply_if_newer(incoming);
        assert_eq!(existing.value, 1);
    }

    fn call(tpl: &str, seq: u16) -> CallObservation {
        CallObservation::new(tpl.to_string(), seq)
    }

    #[test]
    fn apply_call_merges_partial_observations_of_same_stop() {
        let mut s = make_status();
        let now = Utc::now();
        // First message: only the scheduled departure for WAKEFLD.
        let mut c1 = call("WAKEFLD", 0);
        c1.sched_dep = Some(now);
        s.apply_call(c1);
        // Later partial message: only the actual departure for the same stop.
        let mut c2 = call("WAKEFLD", 0);
        c2.act_dep = Some(now + chrono::Duration::minutes(3));
        s.apply_call(c2);

        assert_eq!(s.journey.len(), 1, "same TIPLOC merges into one entry");
        let merged = &s.journey[&0];
        assert!(merged.sched_dep.is_some(), "earlier field retained");
        assert!(merged.act_dep.is_some(), "later field merged in");
    }

    #[test]
    fn apply_call_assigns_stable_first_seen_seq() {
        let mut s = make_status();
        s.apply_call(call("LEEDS", 9)); // incoming seq is advisory and ignored
        s.apply_call(call("WAKEFLD", 9));
        s.apply_call(call("LEEDS", 9)); // revisit — must keep its original canonical seq

        assert_eq!(s.journey.len(), 2);
        assert_eq!(s.tpl_seq["LEEDS"], 0);
        assert_eq!(s.tpl_seq["WAKEFLD"], 1);
        assert_eq!(s.journey[&0].tpl, "LEEDS");
        assert_eq!(s.journey[&1].tpl, "WAKEFLD");
    }

    #[test]
    fn apply_call_ignores_empty_tpl() {
        let mut s = make_status();
        s.apply_call(call("", 0));
        assert!(s.journey.is_empty());
    }

    #[test]
    fn merge_does_not_null_existing_actual() {
        let mut base = call("X", 0);
        base.act_arr = Some(Utc::now());
        let empty = call("X", 0); // carries no actual
        base.merge_from(&empty);
        assert!(base.act_arr.is_some(), "merging an empty obs must not clear an actual");
    }

    #[test]
    fn darwin_version_always_beats_gbr_counter() {
        // Darwin versions are timestamp_millis (~1.75e12); GBR counters are small.
        let gbr_version: u64 = 9999;
        let darwin_version: u64 = 1_750_000_000_000;
        let mut field = Stamped::with_version(Some(5i32), gbr_version);
        let darwin_update = Stamped::with_version(Some(3i32), darwin_version);
        field.apply_if_newer(darwin_update);
        assert_eq!(field.value, Some(3));
        assert_eq!(field.version, darwin_version);
    }
}
