//! RailPredict — high-performance UK rail shadow system.
//!
//! This lib target exposes all modules so integration tests in `tests/` can import
//! them directly. The binary entry point (`src/main.rs`) wires everything together.

pub mod api;
pub mod cache;
pub mod config;
pub mod frontend;
pub mod ingestion;
pub mod networking;
pub mod prediction;
pub mod state_machine;
pub mod types;
