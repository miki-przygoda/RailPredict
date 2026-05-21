//! Core data types for RailPredict.
//!
//! Ownership model decision: `TrainStatus` is owned behind `Arc<RwLock<TrainStatus>>` in the
//! shared `TrainRegistry`. Updates from REST polling, STOMP firehose, and the prediction engine
//! all arrive as owned `TrainStatusUpdate` messages via `mpsc`, applied by a single writer task.
//! This means `TrainStatus` does NOT need to be `Clone` for hot-path use, but it IS `Clone`
//! to allow snapshot reads without holding the lock. All fields must be `Send + Sync`.

pub mod train_id;
pub mod train_status;
pub mod volatility;

pub use train_id::TrainId;
#[allow(unused_imports)]
pub use train_status::{Stamped, TrainStatus, UpdateSource};
pub use volatility::VolatilityContext;
