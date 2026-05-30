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

use chrono::{DateTime, Duration, Utc, Weekday};
use dashmap::DashMap;

pub const MAX_SAMPLES: usize = 90;

/// Rolling statistics for a service pattern, computed over both 7-day and 14-day windows.
///
/// All fields default to 0.0 when no recent history exists — this maps cleanly to
/// "no prior signal" in the ML feature vector without Option unwrapping at call sites.
#[derive(Debug, Clone, Default)]
pub struct RollingStats {
    /// Mean delay in minutes over the last 7 days.
    pub mean_delay: f32,
    /// Standard deviation of delay in minutes over the last 7 days.
    pub std_delay: f32,
    /// Percentage of trains that arrived on time (delay ≤ 0) over the last 7 days.
    pub on_time_pct: f32,
    /// log1p(n) where n = number of observations in the last 7 days.
    pub sample_count_log: f32,
    /// Mean delay in minutes over the last 14 days.
    pub mean_delay_14d: f32,
    /// Standard deviation of delay in minutes over the last 14 days.
    pub std_delay_14d: f32,
}

/// Live Darwin / weather signals available when a train is in Active or Critical state.
/// Used as the 6 extra features for the real-time ONNX model.
#[derive(Debug, Clone, Default)]
pub struct LiveFeatures {
    /// Latest reported delay from Darwin (0.0 if unknown).
    pub current_delay_mins: f32,
    /// Delay of the preceding service at the same origin (0.0 if no signal).
    pub preceding_delay_mins: f32,
    /// Wind speed in mph from VolatilityStore (0.0 if weather polling is off).
    pub wind_mph: f32,
    /// Encoded volatility level: 0.0 = none, 1.0 = wind critical, 2.0 = incident, 3.0 = both.
    pub volatility_score: f32,
    /// Minutes until scheduled departure (negative = en-route).
    pub mins_until_departure: f32,
    /// Mean delay of all other trains at the same origin CRS in the last 30 min (0.0 if unknown).
    pub station_congestion_30m: f32,
    /// Mean delay of all other trains from the same operator (UID prefix) in the last 60 min.
    /// Zero if fewer than 3 other trains are in the window.
    pub operator_cascade_delay: f32,
    /// Delay of the predecessor service (same physical train set, previous trip).
    /// Sourced from Darwin `Association` messages (category NP).  Zero if unknown.
    pub predecessor_train_delay: f32,
    /// Schedule performance allowance in minutes (ptd - wtd). Zero if unknown or no margin.
    /// Positive = slack built in; train can absorb this many minutes of delay and still arrive on time.
    pub schedule_margin_mins: f32,
}

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
    /// What the engine predicted just before this observation was recorded.
    /// `None` for records written before the prediction-capture feature was added.
    pub predicted_delay_mins: Option<i32>,
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

    /// Return the `(delay_mins, recorded_at)` of the most recent record, or `None` if empty.
    /// Used by `PredictionEngine::record_outcome` to apply the write-throttle.
    pub fn last_record(&self, pattern: &ServicePattern) -> Option<(i32, DateTime<Utc>)> {
        let entry = self.inner.get(pattern)?;
        entry.back().map(|r| (r.delay_mins, r.recorded_at))
    }

    /// Rolling statistics for `pattern` over the last 7 and 14 days, computed in one pass.
    ///
    /// Returns `RollingStats::default()` (all zeros) when no 7-day data exists, so
    /// callers can always build a complete ML feature vector without Option handling.
    /// The 14d fields will be zero only if no records exist in the 14-day window.
    pub fn rolling_stats_7d(&self, pattern: &ServicePattern) -> RollingStats {
        let Some(entry) = self.inner.get(pattern) else {
            return RollingStats::default();
        };
        let now = Utc::now();
        let cutoff_7d  = now - Duration::days(7);
        let cutoff_14d = now - Duration::days(14);

        let mut v7: Vec<i32> = Vec::new();
        let mut v14: Vec<i32> = Vec::new();
        for r in entry.iter() {
            if r.recorded_at >= cutoff_7d {
                v7.push(r.delay_mins);
                v14.push(r.delay_mins);
            } else if r.recorded_at >= cutoff_14d {
                v14.push(r.delay_mins);
            }
        }

        if v7.is_empty() {
            return RollingStats::default();
        }

        let stats = |v: &[i32]| -> (f32, f32) {
            let n = v.len() as f64;
            let mean = v.iter().map(|&x| x as f64).sum::<f64>() / n;
            let var  = v.iter().map(|&x| (x as f64 - mean).powi(2)).sum::<f64>() / n;
            (mean as f32, var.sqrt() as f32)
        };

        let n7 = v7.len() as f64;
        let (mean_7d, std_7d)   = stats(&v7);
        let (mean_14d, std_14d) = stats(&v14);
        let on_time = v7.iter().filter(|&&x| x <= 0).count() as f64 / n7 * 100.0;

        RollingStats {
            mean_delay:       mean_7d,
            std_delay:        std_7d,
            on_time_pct:      on_time as f32,
            sample_count_log: (n7 + 1.0).ln() as f32,
            mean_delay_14d:   mean_14d,
            std_delay_14d:    std_14d,
        }
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
        DelayRecord { delay_mins, predicted_delay_mins: None, recorded_at: Utc::now() }
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
        store.insert(p.clone(), DelayRecord { delay_mins: 5, predicted_delay_mins: None, recorded_at: older });
        store.insert(p.clone(), DelayRecord { delay_mins: 3, predicted_delay_mins: None, recorded_at: newer });
        let most_recent = store.most_recent_recorded_at(&p).unwrap();
        // Within a millisecond of `newer` (clock precision in tests)
        assert!((most_recent - newer).num_milliseconds().abs() < 100);
    }
}
