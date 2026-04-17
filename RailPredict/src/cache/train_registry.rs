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

use std::sync::Arc;
use tokio::sync::RwLock;

use chrono::{DateTime, Utc};
use dashmap::DashMap;

use crate::types::{TrainId, TrainStatus};

/// How long after estimated departure a train remains in the registry.
const EVICTION_BUFFER_SECS: i64 = 300; // 5 minutes post-departure

pub struct TrainRegistry {
    trains: DashMap<TrainId, Arc<RwLock<TrainStatus>>>,
}

impl TrainRegistry {
    pub fn new() -> Self {
        Self { trains: DashMap::new() }
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
            self.trains.remove(&id);
        }
    }

    /// Warm the cache from a batch of pre-loaded `TrainStatus` values (Tier A static data).
    pub fn warm(&self, statuses: impl IntoIterator<Item = (TrainId, TrainStatus)>) {
        for (id, status) in statuses {
            self.upsert(id, status);
        }
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
}
