//! State machine — the urgency vocabulary that classifies tracked trains.
//!
//! `TrainState` (Dormant → Monitored → Active → Critical → Terminal) determines how
//! aggressively a service should be polled. States are set inline by the ingestion
//! pipeline; `StateChangeEvent`s are broadcast to the API/SSE layer for live UI updates.
//! A scheduler that acts on these states will be (re)built with Tier C — see
//! `docs/tech-debt.md`.

pub mod train_state;

pub use train_state::{StateChangeEvent, TrainState};
