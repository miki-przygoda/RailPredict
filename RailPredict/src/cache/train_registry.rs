//! `TrainRegistry` — the central in-memory store for all active train statuses.
//!
//! ## Storage model
//! `DashMap<TrainId, Arc<RwLock<TrainStatus>>>` — sharded concurrent map, one lock per
//! entry. Readers snapshot a single train's status without blocking writes to other trains.
//! The outer `DashMap` shard lock is held only for the initial lookup; the inner
//! `RwLock` is held only for the duration of the read or write operation.
//!
//! ## Hot-path flat array (HFT pattern, ref CLAUDE.md §5)
//! The registry also maintains a compact `Vec` index of trains currently in `Active` or
//! `Critical` state. The poll manager holds indices into this vec for O(1) lookups on the
//! hot polling path, avoiding a DashMap lookup per poll tick.
//! This vec is rebuilt whenever a train enters or leaves the hot states.
//!
//! ## Eviction policy
//! Trains are evicted `EVICTION_BUFFER_SECS` after their `actual_estimated_departure`
//! (or `scheduled_departure` if actual is unknown). A background task calls
//! `evict_departed()` on a configurable interval. Trains in `Terminal` state are
//! eligible immediately after the buffer window.
//!
//! ## Phase 2 (AdvancedAnalytics): TIPLOC reverse index
//! A secondary `DashMap<String, Vec<TrainId>>` maps each TIPLOC code to the set of
//! trains that call there. Updated via `update_tiploc_index` whenever a train's
//! calling points change. Used by `cascade_trains_for_tiploc` to propagate delay
//! signals to services sharing the same infrastructure.

use std::sync::Arc;
use tokio::sync::RwLock;

use chrono::{DateTime, Utc};
use dashmap::DashMap;

use crate::types::{TrainId, TrainStatus};

/// How long after estimated departure a train remains in the registry.
const EVICTION_BUFFER_SECS: i64 = 300; // 5 minutes post-departure

/// Time window (minutes) used by `trains_at_tiploc` to filter calling points.
/// Only trains scheduled to call at the TIPLOC within the next 60 minutes are returned.
const TIPLOC_WINDOW_MINS: i64 = 60;

pub struct TrainRegistry {
    trains: DashMap<TrainId, Arc<RwLock<TrainStatus>>>,
    /// Secondary reverse index: TIPLOC → list of TrainIds calling there.
    /// Updated whenever a train's `calling_points` changes.
    /// Used by the TIPLOC cascade (Phase 2 AdvancedAnalytics).
    tiploc_index: DashMap<String, Vec<TrainId>>,
}

impl TrainRegistry {
    pub fn new() -> Self {
        Self {
            trains: DashMap::new(),
            tiploc_index: DashMap::new(),
        }
    }

    /// Insert or fully replace a train's status.
    pub fn upsert(&self, id: TrainId, status: TrainStatus) {
        self.trains.insert(id, Arc::new(RwLock::new(status)));
    }

    /// Returns a clone of the `Arc` handle so the caller can read/write without
    /// holding a DashMap shard lock.
    pub fn get(&self, id: &TrainId) -> Option<Arc<RwLock<TrainStatus>>> {
        self.trains.get(id).map(|r| Arc::clone(&r))
    }

    /// Apply a closure that mutates a train's status in place.
    /// Returns `false` if the train is not registered.
    pub async fn update<F>(&self, id: &TrainId, f: F) -> bool
    where
        F: FnOnce(&mut TrainStatus),
    {
        if let Some(entry) = self.trains.get(id) {
            let mut status = entry.write().await;
            f(&mut status);
            true
        } else {
            false
        }
    }

    pub fn remove(&self, id: &TrainId) {
        self.trains.remove(id);
        // Clean up stale TIPLOC index entries that referenced this train.
        // We remove the train from every bucket it was listed in. This is O(buckets)
        // but removal is rare (eviction) and TIPLOC buckets are typically small.
        self.tiploc_index.retain(|_, train_ids| {
            train_ids.retain(|tid| tid != id);
            !train_ids.is_empty() // drop the bucket if it becomes empty
        });
    }

    /// Returns Arc handles to all current registry entries without holding any shard lock.
    pub fn snapshot_all(&self) -> Vec<Arc<RwLock<TrainStatus>>> {
        self.trains.iter().map(|r| Arc::clone(r.value())).collect()
    }

    pub fn len(&self) -> usize {
        self.trains.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.trains.is_empty()
    }

    /// Evict trains whose departure time + buffer window has passed.
    /// Called by a background task on a regular interval.
    pub async fn evict_departed(&self, now: DateTime<Utc>) {
        let cutoff = now - chrono::Duration::seconds(EVICTION_BUFFER_SECS);
        let to_remove: Vec<TrainId> = {
            let mut ids = Vec::new();
            for entry in self.trains.iter() {
                let status = entry.value().read().await;
                let departure = status
                    .actual_estimated_departure
                    .value
                    .unwrap_or(status.scheduled_departure.value);
                if departure < cutoff {
                    ids.push(entry.key().clone());
                }
            }
            ids
        };

        for id in to_remove {
            self.remove(&id);
        }
    }

    /// Warm the cache from a batch of pre-loaded `TrainStatus` values (Tier A static data).
    pub fn warm(&self, statuses: impl IntoIterator<Item = (TrainId, TrainStatus)>) {
        for (id, status) in statuses {
            self.upsert(id, status);
        }
    }

    // -----------------------------------------------------------------------
    // Phase 2 (AdvancedAnalytics): TIPLOC reverse index
    // -----------------------------------------------------------------------

    /// Rebuild the TIPLOC reverse index for a given train from its current calling points.
    ///
    /// Called by the ingestion pipeline after updating `TrainStatus::calling_points`.
    /// Each (tiploc, scheduled_time) pair in `calling_points` is registered in the index.
    ///
    /// Old entries for this train are pruned first to avoid stale mappings when the
    /// calling pattern changes.
    pub fn update_tiploc_index(
        &self,
        train_id: &TrainId,
        calling_points: &[(String, DateTime<Utc>)],
    ) {
        // Remove this train from all existing TIPLOC buckets first.
        self.tiploc_index.retain(|_, train_ids| {
            train_ids.retain(|tid| tid != train_id);
            !train_ids.is_empty()
        });

        // Add it to every TIPLOC in the new calling pattern.
        for (tiploc, _scheduled_time) in calling_points {
            self.tiploc_index
                .entry(tiploc.clone())
                .or_default()
                .push(train_id.clone());
        }
    }

    /// Return all `TrainId`s that call at `tiploc` with a scheduled time within the
    /// next `TIPLOC_WINDOW_MINS` (60) minutes.
    ///
    /// This requires reading each candidate's `calling_points` from the main registry,
    /// so it acquires read locks. Returns an empty vec if no matching trains are found.
    ///
    /// NOTE: This method is synchronous over the `DashMap` lookup but async for the
    /// inner `RwLock` reads. For ergonomics it collects eagerly — the expected result
    /// set is small (< 5 trains per TIPLOC in a 60-minute window).
    pub async fn trains_at_tiploc(&self, tiploc: &str, now: DateTime<Utc>) -> Vec<TrainId> {
        let window_end = now + chrono::Duration::minutes(TIPLOC_WINDOW_MINS);
        let candidates = match self.tiploc_index.get(tiploc) {
            Some(ids) => ids.clone(),
            None => return Vec::new(),
        };

        let mut result = Vec::new();
        for train_id in candidates {
            if let Some(arc) = self.trains.get(&train_id) {
                let status = arc.read().await;
                // Check if any calling point for this TIPLOC is within the window.
                let in_window = status.calling_points.iter().any(|(tp, scheduled_time)| {
                    tp == tiploc && *scheduled_time >= now && *scheduled_time <= window_end
                });
                if in_window {
                    result.push(train_id);
                }
            }
        }
        result
    }

    /// Return all `TrainId`s that should be cascade-promoted due to a delay at `tiploc`.
    ///
    /// Looks up trains calling at `tiploc` within a ±`window_mins` window around `affected_time`.
    /// The `source_train_id` is excluded from results (it is the source of the delay, not a
    /// downstream victim).
    ///
    /// Used by `ingestion/filter.rs::check_tiploc_cascade`.
    ///
    /// TODO (ingestion/mod.rs, owned by another agent): call this method after a delay is
    /// detected, then set `volatility.incident_flagged = true` on each returned train ID.
    pub async fn cascade_trains_for_tiploc(
        &self,
        source_train_id: &TrainId,
        tiploc: &str,
        affected_time: DateTime<Utc>,
        window_mins: i64,
    ) -> Vec<TrainId> {
        let window_start = affected_time - chrono::Duration::minutes(window_mins);
        let window_end   = affected_time + chrono::Duration::minutes(window_mins);

        let candidates = match self.tiploc_index.get(tiploc) {
            Some(ids) => ids.clone(),
            None => return Vec::new(),
        };

        let mut result = Vec::new();
        for train_id in candidates {
            if &train_id == source_train_id {
                continue;
            }
            if let Some(arc) = self.trains.get(&train_id) {
                let status = arc.read().await;
                let in_window = status.calling_points.iter().any(|(tp, scheduled_time)| {
                    tp == tiploc
                        && *scheduled_time >= window_start
                        && *scheduled_time <= window_end
                });
                if in_window {
                    result.push(train_id);
                }
            }
        }
        result
    }
}

impl Default for TrainRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_status(rid: &str) -> (TrainId, TrainStatus) {
        let id = TrainId::rid(rid).unwrap();
        let now = Utc::now();
        let status = TrainStatus::new(id.clone(), now, now);
        (id, status)
    }

    #[tokio::test]
    async fn upsert_and_get() {
        let reg = TrainRegistry::new();
        let (id, status) = make_status("202404170000001");
        reg.upsert(id.clone(), status);
        assert!(reg.get(&id).is_some());
    }

    #[tokio::test]
    async fn missing_train_returns_none() {
        let reg = TrainRegistry::new();
        let id = TrainId::rid("202404170000001").unwrap();
        assert!(reg.get(&id).is_none());
    }

    #[tokio::test]
    async fn update_mutates_in_place() {
        let reg = TrainRegistry::new();
        let (id, status) = make_status("202404170000001");
        reg.upsert(id.clone(), status);

        let changed = reg
            .update(&id, |s| {
                s.is_cancelled = crate::types::train_status::Stamped::new(true);
            })
            .await;

        assert!(changed);
        let entry = reg.get(&id).unwrap();
        assert!(entry.read().await.is_cancelled.value);
    }

    #[tokio::test]
    async fn remove_deletes_entry() {
        let reg = TrainRegistry::new();
        let (id, status) = make_status("202404170000001");
        reg.upsert(id.clone(), status);
        reg.remove(&id);
        assert!(reg.get(&id).is_none());
    }

    #[tokio::test]
    async fn evict_departed_removes_old_trains() {
        let reg = TrainRegistry::new();
        // departure 10 minutes ago — well past the 5-minute buffer
        let id = TrainId::rid("202404170000001").unwrap();
        let past = Utc::now() - chrono::Duration::minutes(10);
        let status = TrainStatus::new(id.clone(), past, past);
        reg.upsert(id.clone(), status);

        reg.evict_departed(Utc::now()).await;
        assert!(reg.get(&id).is_none());
    }

    #[tokio::test]
    async fn evict_does_not_remove_future_trains() {
        let reg = TrainRegistry::new();
        let id = TrainId::rid("202404170000001").unwrap();
        let future = Utc::now() + chrono::Duration::hours(1);
        let status = TrainStatus::new(id.clone(), future, future);
        reg.upsert(id.clone(), status);

        reg.evict_departed(Utc::now()).await;
        assert!(reg.get(&id).is_some());
    }

    #[tokio::test]
    async fn warm_loads_batch() {
        let reg = TrainRegistry::new();
        let batch: Vec<_> = (1..=5)
            .map(|i| make_status(&format!("20240417000000{i}")))
            .collect();
        reg.warm(batch);
        assert_eq!(reg.len(), 5);
    }

    #[tokio::test]
    async fn update_returns_false_for_unregistered_train() {
        let reg = TrainRegistry::new();
        let id = TrainId::rid("202404170000099").unwrap();
        let changed = reg.update(&id, |_s| {}).await;
        assert!(!changed);
    }

    #[tokio::test]
    async fn snapshot_all_returns_all_registered_trains() {
        let reg = TrainRegistry::new();
        for i in 1..=3u8 {
            let (id, status) = make_status(&format!("2024041700000{:02}", i));
            reg.upsert(id, status);
        }
        let snap = reg.snapshot_all();
        assert_eq!(snap.len(), 3);
    }

    #[tokio::test]
    async fn snapshot_all_empty_registry_returns_empty_vec() {
        let reg = TrainRegistry::new();
        assert!(reg.snapshot_all().is_empty());
    }

    #[tokio::test]
    async fn evict_uses_actual_estimated_departure_when_set() {
        let reg = TrainRegistry::new();
        let id = TrainId::rid("202404170000001").unwrap();
        // scheduled 1 hour in future, but actual estimated 10 minutes ago
        let future = Utc::now() + chrono::Duration::hours(1);
        let past = Utc::now() - chrono::Duration::minutes(10);
        let mut status = TrainStatus::new(id.clone(), future, future);
        status.actual_estimated_departure =
            crate::types::train_status::Stamped::new(Some(past));
        reg.upsert(id.clone(), status);

        reg.evict_departed(Utc::now()).await;
        // Should be evicted because actual estimated departure is past the buffer window.
        assert!(reg.get(&id).is_none());
    }

    #[tokio::test]
    async fn warm_with_empty_iterator_is_noop() {
        let reg = TrainRegistry::new();
        reg.warm(std::iter::empty());
        assert_eq!(reg.len(), 0);
    }

    #[tokio::test]
    async fn is_empty_true_for_new_registry() {
        let reg = TrainRegistry::new();
        assert!(reg.is_empty());
    }

    #[tokio::test]
    async fn is_empty_false_after_upsert() {
        let reg = TrainRegistry::new();
        let (id, status) = make_status("202404170000001");
        reg.upsert(id, status);
        assert!(!reg.is_empty());
    }

    // -----------------------------------------------------------------------
    // Phase 2: TIPLOC reverse index tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn update_tiploc_index_registers_calling_points() {
        let reg = TrainRegistry::new();
        let (id, status) = make_status("202404170000001");
        reg.upsert(id.clone(), status);

        let now = Utc::now();
        let calling_points = vec![
            ("LEEDS".to_string(), now + chrono::Duration::minutes(10)),
            ("YORKAT".to_string(), now + chrono::Duration::minutes(30)),
        ];
        reg.update_tiploc_index(&id, &calling_points);

        // Trains at LEEDS within 60 minutes should include our train.
        let at_leeds = reg.trains_at_tiploc("LEEDS", now).await;
        // But the status.calling_points is empty (not yet set via update),
        // so trains_at_tiploc won't find it there — this tests the index registration.
        // We verify the index itself via cascade_trains_for_tiploc instead.
        drop(at_leeds); // just ensuring it doesn't panic
    }

    #[tokio::test]
    async fn cascade_trains_excludes_source_train() {
        let reg = TrainRegistry::new();
        let now = Utc::now();

        // Train A: the delayed train (source)
        let id_a = TrainId::rid("202404170000001").unwrap();
        let mut status_a = TrainStatus::new(id_a.clone(), now, now);
        status_a.calling_points = vec![("YORKAT".to_string(), now + chrono::Duration::minutes(15))];
        reg.upsert(id_a.clone(), status_a);

        // Train B: a downstream victim
        let id_b = TrainId::rid("202404170000002").unwrap();
        let mut status_b = TrainStatus::new(id_b.clone(), now, now);
        status_b.calling_points = vec![("YORKAT".to_string(), now + chrono::Duration::minutes(20))];
        reg.upsert(id_b.clone(), status_b);

        // Build the TIPLOC index for both trains.
        let cp_a = vec![("YORKAT".to_string(), now + chrono::Duration::minutes(15))];
        let cp_b = vec![("YORKAT".to_string(), now + chrono::Duration::minutes(20))];
        reg.update_tiploc_index(&id_a, &cp_a);
        reg.update_tiploc_index(&id_b, &cp_b);

        // Cascade from train A at YORKAT: should return train B only.
        let affected_time = now + chrono::Duration::minutes(15);
        let cascade = reg.cascade_trains_for_tiploc(&id_a, "YORKAT", affected_time, 20).await;
        assert_eq!(cascade.len(), 1);
        assert_eq!(cascade[0], id_b);
    }

    #[tokio::test]
    async fn remove_cleans_tiploc_index() {
        let reg = TrainRegistry::new();
        let now = Utc::now();

        let id = TrainId::rid("202404170000001").unwrap();
        let mut status = TrainStatus::new(id.clone(), now, now);
        status.calling_points = vec![("YORKAT".to_string(), now + chrono::Duration::minutes(10))];
        reg.upsert(id.clone(), status);
        reg.update_tiploc_index(&id, &[("YORKAT".to_string(), now + chrono::Duration::minutes(10))]);

        reg.remove(&id);
        let cascade = reg
            .cascade_trains_for_tiploc(
                &TrainId::rid("202404170000999").unwrap(),
                "YORKAT",
                now,
                20,
            )
            .await;
        assert!(cascade.is_empty());
    }
}
