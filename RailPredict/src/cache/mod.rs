//! In-memory cache layer — the fast local store all read queries hit.
//!
//! `TrainRegistry` is the single shared store, backed by `dashmap` for concurrent reads
//! without a global lock. The `Arc<RwLock<TrainStatus>>` per entry means readers can
//! snapshot a train's status without blocking writers on other trains.

pub mod location_names;
pub mod location_coords;
pub mod rail_graph;
pub mod station_index;
pub mod train_registry;

pub use train_registry::{LiveService, TrainRegistry};
pub use station_index::StationIndex;
