//! `PollManager` — global BinaryHeap-based poll scheduler.
//!
//! One task runs the manager loop. It sleeps until the next scheduled poll is due, then
//! fires. New trains can be registered mid-sleep via an `mpsc` sender; `tokio::select!`
//! races the sleep against the registration channel so the heap is re-evaluated immediately
//! when a sooner entry arrives.
//!
//! State changes are broadcast on a bounded `mpsc` channel to the notification service.
//!
//! ## Channel buffer size rationale
//! `STATE_CHANGE_BUFFER`: 256. The notification consumer (API/UI layer) is expected to be fast.
//! Under a mass-incident scenario, an entire corridor (~50–100 trains) might promote at once.
//! 256 gives headroom without unbounded growth; if the consumer falls >256 events behind,
//! `try_send` errors are logged and the event is dropped (last-writer-wins semantics are
//! acceptable here — the next poll tick will re-emit the current state anyway).
//!
//! `REGISTRATION_BUFFER`: 64. Train registrations arrive one-at-a-time from the ingestion
//! pipeline. 64 is generous; backpressure from a full buffer is acceptable (the STOMP
//! receiver can park briefly).

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{sleep_until, Instant};

use crate::types::TrainId;

use super::train_state::{PromotionReason, TrainState};

const REGISTRATION_BUFFER: usize = 64;

// ---------------------------------------------------------------------------
// Heap entry — min-heap by next_poll_at
// ---------------------------------------------------------------------------

/// An entry in the global poll heap. Ordered by `next_poll_at` ascending (min-heap).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PollEntry {
    pub next_poll_at: Instant,
    pub train_id: TrainId,
    pub state: TrainState,
}

impl Ord for PollEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse order so BinaryHeap (max-heap by default) gives us the soonest entry first.
        other.next_poll_at.cmp(&self.next_poll_at)
    }
}

impl PartialOrd for PollEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// ---------------------------------------------------------------------------
// State change notification
// ---------------------------------------------------------------------------

/// Emitted on the notification channel whenever a train changes state.
#[derive(Debug, Clone)]
pub struct StateChangeEvent {
    pub train_id: TrainId,
    pub old_state: TrainState,
    pub new_state: TrainState,
    pub reason: PromotionReason,
}

// ---------------------------------------------------------------------------
// Registration message
// ---------------------------------------------------------------------------

/// Sent to the manager to register a new train or update an existing one.
#[derive(Debug)]
pub struct RegistrationMsg {
    pub train_id: TrainId,
    pub state: TrainState,
    /// Absolute time when this train should first be polled (or re-polled).
    pub next_poll_at: Instant,
}

// ---------------------------------------------------------------------------
// PollManager
// ---------------------------------------------------------------------------

/// Handles for communicating with a running `PollManager`.
pub struct PollManagerHandles {
    /// Send here to register a new train or update an existing entry's next-poll time.
    pub register_tx: mpsc::Sender<RegistrationMsg>,
}

/// The global poll manager. Owns the heap and drives the polling loop.
pub struct PollManager {
    heap: BinaryHeap<PollEntry>,
    register_rx: mpsc::Receiver<RegistrationMsg>,
    state_change_tx: broadcast::Sender<StateChangeEvent>,
}

impl PollManager {
    /// Construct a new manager. The caller provides the broadcast sender so all writers
    /// (poll manager + ingestion pipeline) share one channel the API SSE layer subscribes to.
    pub fn new(state_change_tx: broadcast::Sender<StateChangeEvent>) -> (Self, PollManagerHandles) {
        let (register_tx, register_rx) = mpsc::channel(REGISTRATION_BUFFER);

        let manager = Self {
            heap: BinaryHeap::new(),
            register_rx,
            state_change_tx,
        };

        let handles = PollManagerHandles { register_tx };

        (manager, handles)
    }

    /// Run the poll loop. Call this inside `tokio::spawn`.
    ///
    /// The loop races two branches via `tokio::select!`:
    ///   1. Sleep until the next scheduled poll, then fire it.
    ///   2. Receive a new registration and push it onto the heap — which may make
    ///      the next-poll time sooner, so the sleep is re-evaluated on the next iteration.
    pub async fn run(mut self) {
        loop {
            match self.heap.peek() {
                None => {
                    // Heap is empty — wait for the first registration.
                    match self.register_rx.recv().await {
                        Some(msg) => self.push_registration(msg),
                        None => break, // all senders dropped; shut down
                    }
                }
                Some(next) => {
                    let deadline = next.next_poll_at;

                    tokio::select! {
                        // Branch 1: next poll is due.
                        _ = sleep_until(deadline) => {
                            if let Some(entry) = self.heap.pop() {
                                self.fire_poll(entry).await;
                            }
                        }

                        // Branch 2: new registration arrives — re-evaluate heap.
                        msg = self.register_rx.recv() => {
                            match msg {
                                Some(msg) => self.push_registration(msg),
                                None => break,
                            }
                        }
                    }
                }
            }
        }
    }

    fn push_registration(&mut self, msg: RegistrationMsg) {
        self.heap.push(PollEntry {
            next_poll_at: msg.next_poll_at,
            train_id: msg.train_id,
            state: msg.state,
        });
    }

    async fn fire_poll(&mut self, entry: PollEntry) {
        if entry.state == TrainState::Terminal {
            return;
        }

        // Re-queue for the next poll interval, if this state polls.
        if let Some(interval) = entry.state.poll_interval() {
            self.heap.push(PollEntry {
                next_poll_at: Instant::now() + interval,
                train_id: entry.train_id.clone(),
                state: entry.state,
            });
        }

        // Emit a state-change event (same state → same state counts as a "poll fired" signal
        // here; full transition logic lives in the registry writer task which acts on it).
        let event = StateChangeEvent {
            train_id: entry.train_id,
            old_state: entry.state,
            new_state: entry.state,
            reason: PromotionReason::TimeBased,
        };

        // Non-blocking broadcast send; drop the event if no receivers are active.
        let _ = self.state_change_tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Duration;

    #[test]
    fn poll_entry_min_heap_ordering() {
        let sooner = PollEntry {
            next_poll_at: Instant::now(),
            train_id: TrainId::rid("202404170000001").unwrap(),
            state: TrainState::Active,
        };
        let later = PollEntry {
            next_poll_at: Instant::now() + Duration::from_secs(60),
            train_id: TrainId::rid("202404170000002").unwrap(),
            state: TrainState::Active,
        };

        let mut heap = BinaryHeap::new();
        heap.push(later.clone());
        heap.push(sooner.clone());

        // Min-heap: soonest entry should come out first.
        let first = heap.pop().unwrap();
        assert_eq!(first.train_id, sooner.train_id);
    }

    #[tokio::test]
    async fn registration_reaches_manager() {
        let (sc_tx, mut sc_rx) = tokio::sync::broadcast::channel(256);
        let (manager, handles) = PollManager::new(sc_tx);
        tokio::spawn(manager.run());

        handles
            .register_tx
            .send(RegistrationMsg {
                train_id: TrainId::rid("202404170000001").unwrap(),
                state: TrainState::Active,
                next_poll_at: Instant::now() + Duration::from_millis(50),
            })
            .await
            .unwrap();

        // Wait for the poll to fire and emit a state-change event.
        let event = tokio::time::timeout(
            Duration::from_millis(500),
            sc_rx.recv(),
        )
        .await
        .expect("timed out waiting for state change event")
        .expect("channel closed unexpectedly");

        assert_eq!(event.train_id, TrainId::rid("202404170000001").unwrap());
    }
}
