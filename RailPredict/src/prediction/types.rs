//! Core data types for the Tier B prediction engine.
//!
//! `ServicePattern` is the stable key for historical data — keyed on (uid, weekday, origin_crs)
//! rather than RID, which changes daily. `HistoricalStore` caps each pattern's ring at
//! MAX_SAMPLES entries (≈13 weeks of daily data) and enforces the cap on every insert.

use std::collections::VecDeque;

use chrono::{DateTime, Utc, Weekday};
use dashmap::DashMap;

pub const MAX_SAMPLES: usize = 90;

/// The stable identity of a recurring rail service — independent of the daily RID.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServicePattern {
    /// RTTI UID (e.g. "C12345") — identifies the service across operating days.
    pub uid: String,
    /// Day of the week the service runs.
    pub weekday: Weekday,
    /// CRS code of the origin station (e.g. "LEEDS").
    pub origin_crs: String,
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
        };
        let p2 = ServicePattern {
            uid: "C12345".to_string(),
            weekday: chrono::Weekday::Tue,
            origin_crs: "LDS".to_string(),
        };
        store.insert(p1, record(1));
        store.insert(p2, record(2));
        assert_eq!(store.pattern_count(), 2);
    }

    #[test]
    fn different_uids_are_distinct_patterns() {
        let store = HistoricalStore::new();
        let p1 = ServicePattern { uid: "A00001".to_string(), weekday: chrono::Weekday::Mon, origin_crs: "LDS".to_string() };
        let p2 = ServicePattern { uid: "A00002".to_string(), weekday: chrono::Weekday::Mon, origin_crs: "LDS".to_string() };
        store.insert(p1.clone(), record(3));
        store.insert(p2.clone(), record(7));
        assert_eq!(store.get_samples(&p1).unwrap(), vec![3]);
        assert_eq!(store.get_samples(&p2).unwrap(), vec![7]);
    }
}
