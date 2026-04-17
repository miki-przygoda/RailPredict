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
