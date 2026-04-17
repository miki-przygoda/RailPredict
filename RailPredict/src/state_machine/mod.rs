//! State machine — the polling pulse of RailPredict.
//!
//! One global `PollManager` owns a `BinaryHeap` of scheduled poll entries, ordered by
//! next-poll time. State transitions are applied locally and broadcast via `mpsc` to the
//! notification service. No per-train `tokio::spawn` tasks.

pub mod poll_manager;
pub mod train_state;

#[allow(unused_imports)]
pub use poll_manager::PollManager;
#[allow(unused_imports)]
pub use train_state::{PromotionReason, TrainState};
