//! Tier B prediction engine — local delay inference from historical Darwin data.
//!
//! Populates `TrainStatus::predicted_delay_mins` and `VolatilityContext::historical_reliability`
//! without any external API calls. History accumulates at runtime from confirmed Darwin
//! `reported_delay_mins` values via `PredictionEngine::record_outcome`.

pub mod engine;
pub mod onnx_engine;
pub mod types;

pub use engine::PredictionEngine;
pub use onnx_engine::OnnxEngine;
