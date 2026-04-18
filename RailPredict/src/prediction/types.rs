//! Core data types for the Tier B prediction engine.
//!
//! `ServicePattern` is the stable key for historical data — keyed on
//! `(uid, weekday, origin_crs, departure_hour)` rather than RID, which changes daily.
//! `HistoricalStore` caps each pattern's ring at MAX_SAMPLES entries (≈13 weeks of daily
//! data) and enforces the cap on every insert.
//!
//! ## Phase 3 (AdvancedAnalytics): `departure_hour` added to `ServicePattern`
//! Splits history into per-hour buckets, so rush-hour delays don't contaminate off-peak
//! predictions for the same UID. Schema change: see migrations/20240417120005_add_departure_hour.sql.
//!
//! ## Phase 4 (AdvancedAnalytics): `most_recent_recorded_at` added to `HistoricalStore`
//! Used by `PredictionEngine::predict_and_update` to apply exponential confidence decay
//! when history is stale.

use std::collections::VecDeque;

use chrono::{DateTime, Utc, Weekday};
use dashmap::DashMap;

pub const MAX_SAMPLES: usize = 90;

/// The stable identity of a recurring rail service — independent of the daily RID.
///
/// Phase 3: `departure_hour` splits the pattern into per-hour buckets, so the same
/// UID on Monday at 08:00 and at 14:00 are tracked independently.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServicePattern {
    /// RTTI UID (e.g. "C12345") — identifies the service across operating days.
    pub uid: String,
    /// Day of the week the service runs.
    pub weekday: Weekday,
    /// CRS code of the origin station (e.g. "LEEDS").
    pub origin_crs: String,
    /// Hour of the scheduled departure (0–23). Added in Phase 3 to distinguish
    /// rush-hour from off-peak departures of the same UID.
    pub departure_hour: u8,
}

/// A single historical delay observation for a service pattern.
#[derive(Debug, Clone)]
pub struct DelayRecord {
    pub delay_mins: i32,
    pub recorded_at: DateTime<Utc>,
}

/// Thread-safe in-memory store mapping each `ServicePattern` to a capped ring of `DelayRecord`s.
pub struct HistoricalStore {
    inner: DashMap<ServicePattern, VecDeque<DelayRecord>>,
}

impl Default for HistoricalStore {
    fn default() -> Self {
        Self::new()
    }
}

impl HistoricalStore {
    pub fn new() -> Self {
        Self { inner: DashMap::new() }
    }

    /// Append a delay observation for `pattern`, evicting the oldest if over MAX_SAMPLES.
    pub fn insert(&self, pattern: ServicePattern, record: DelayRecord) {
        let mut entry = self.inner.entry(pattern).or_default();
        entry.push_back(record);
        while entry.len() > MAX_SAMPLES {
            entry.pop_front();
        }
    }

    /// Return a snapshot of all delay values for `pattern`, or `None` if no history exists.
    pub fn get_samples(&self, pattern: &ServicePattern) -> Option<Vec<i32>> {
        let entry = self.inner.get(pattern)?;
        if entry.is_empty() {
            return None;
        }
        Some(entry.iter().map(|r| r.delay_mins).collect())
    }

    /// Return the `recorded_at` timestamp of the most recent `DelayRecord` for this pattern.
    /// Returns `None` if no history exists.
    ///
    /// Used by Phase 4 (confidence decay): if the most recent record is older than
    /// `STALENESS_THRESHOLD_DAYS`, the prediction engine applies exponential decay to confidence.
    pub fn most_recent_recorded_at(&self, pattern: &ServicePattern) -> Option<DateTime<Utc>> {
        let entry = self.inner.get(pattern)?;
        // Records are ordered oldest-first (VecDeque, front = oldest), so the back is newest.
        entry.back().map(|r| r.recorded_at)
    }

    /// Flat snapshot of every record across all patterns. Used by the DB flush task.
    pub fn all_records(&self) -> Vec<(ServicePattern, DelayRecord)> {
        self.inner
            .iter()
            .flat_map(|entry| {
                let pattern = entry.key().clone();
                entry
                    .value()
                    .iter()
                    .map(move |r| (pattern.clone(), r.clone()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Number of distinct service patterns tracked.
    #[cfg(test)]
    pub fn pattern_count(&self) -> usize {
        self.inner.len()
    }

    /// Sample count for a given pattern (test helper).
    #[cfg(test)]
    pub fn sample_count(&self, pattern: &ServicePattern) -> usize {
        self.inner.get(pattern).map(|e| e.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn record(delay_mins: i32) -> DelayRecord {
        DelayRecord { delay_mins, recorded_at: Utc::now() }
    }

    fn pattern() -> ServicePattern {
        ServicePattern {
            uid: "C12345".to_string(),
            weekday: chrono::Weekday::Mon,
            origin_crs: "LDS".to_string(),
            departure_hour: 9,
        }
    }

    #[test]
    fn get_samples_returns_none_for_unknown_pattern() {
        let store = HistoricalStore::new();
        assert!(store.get_samples(&pattern()).is_none());
    }

    #[test]
    fn insert_and_get_samples_roundtrip() {
        let store = HistoricalStore::new();
        let p = pattern();
        store.insert(p.clone(), record(5));
        store.insert(p.clone(), record(10));
        let samples = store.get_samples(&p).unwrap();
        assert_eq!(samples.len(), 2);
        assert!(samples.contains(&5));
        assert!(samples.contains(&10));
    }

    #[test]
    fn insert_caps_at_max_samples() {
        let store = HistoricalStore::new();
        let p = pattern();
        for i in 0..MAX_SAMPLES + 10 {
            store.insert(p.clone(), record(i as i32));
        }
        assert_eq!(store.sample_count(&p), MAX_SAMPLES);
    }

    #[test]
    fn insert_evicts_oldest_entry_at_cap() {
        let store = HistoricalStore::new();
        let p = pattern();
        for _ in 0..MAX_SAMPLES {
            store.insert(p.clone(), record(0));
        }
        store.insert(p.clone(), record(999));
        let samples = store.get_samples(&p).unwrap();
        assert_eq!(samples.len(), MAX_SAMPLES);
        assert!(samples.contains(&999), "newest value must survive eviction");
        assert_eq!(
            samples.iter().filter(|&&v| v == 0).count(),
            MAX_SAMPLES - 1,
            "exactly one 0 should have been evicted"
        );
    }

    #[test]
    fn pattern_count_tracks_distinct_patterns() {
        let store = HistoricalStore::new();
        let p1 = ServicePattern {
            uid: "C12345".to_string(),
            weekday: chrono::Weekday::Mon,
            origin_crs: "LDS".to_string(),
            departure_hour: 9,
        };
        let p2 = ServicePattern {
            uid: "C12345".to_string(),
            weekday: chrono::Weekday::Tue,
            origin_crs: "LDS".to_string(),
            departure_hour: 9,
        };
        store.insert(p1, record(1));
        store.insert(p2, record(2));
        assert_eq!(store.pattern_count(), 2);
    }

    #[test]
    fn different_uids_are_distinct_patterns() {
        let store = HistoricalStore::new();
        let p1 = ServicePattern { uid: "A00001".to_string(), weekday: chrono::Weekday::Mon, origin_crs: "LDS".to_string(), departure_hour: 8 };
        let p2 = ServicePattern { uid: "A00002".to_string(), weekday: chrono::Weekday::Mon, origin_crs: "LDS".to_string(), departure_hour: 8 };
        store.insert(p1.clone(), record(3));
        store.insert(p2.clone(), record(7));
        assert_eq!(store.get_samples(&p1).unwrap(), vec![3]);
        assert_eq!(store.get_samples(&p2).unwrap(), vec![7]);
    }

    #[test]
    fn different_hours_are_distinct_patterns() {
        let store = HistoricalStore::new();
        let p_peak = ServicePattern {
            uid: "C12345".to_string(),
            weekday: chrono::Weekday::Mon,
            origin_crs: "LDS".to_string(),
            departure_hour: 8,
        };
        let p_offpeak = ServicePattern {
            uid: "C12345".to_string(),
            weekday: chrono::Weekday::Mon,
            origin_crs: "LDS".to_string(),
            departure_hour: 14,
        };
        store.insert(p_peak.clone(), record(15)); // rush-hour delay
        store.insert(p_offpeak.clone(), record(2)); // off-peak on time
        assert_eq!(store.get_samples(&p_peak).unwrap(), vec![15]);
        assert_eq!(store.get_samples(&p_offpeak).unwrap(), vec![2]);
        assert_eq!(store.pattern_count(), 2);
    }

    #[test]
    fn most_recent_recorded_at_returns_none_for_empty() {
        let store = HistoricalStore::new();
        assert!(store.most_recent_recorded_at(&pattern()).is_none());
    }

    #[test]
    fn most_recent_recorded_at_returns_newest_timestamp() {
        use chrono::Duration;
        let store = HistoricalStore::new();
        let p = pattern();
        let older = Utc::now() - Duration::days(10);
        let newer = Utc::now() - Duration::days(1);
        store.insert(p.clone(), DelayRecord { delay_mins: 5, recorded_at: older });
        store.insert(p.clone(), DelayRecord { delay_mins: 3, recorded_at: newer });
        let most_recent = store.most_recent_recorded_at(&p).unwrap();
        // Within a millisecond of `newer` (clock precision in tests)
        assert!((most_recent - newer).num_milliseconds().abs() < 100);
    }
}
