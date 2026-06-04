//! Read-only per-station / per-route analytics for the `/stations/:crs` explorer.
//!
//! Aggregates `delay_history` filtered to a single `origin_crs`, joining
//! `services` (on `uid`) where route O–D pairs are needed. All queries apply
//! the standard delay sanity filter (`delay_mins BETWEEN -120 AND 600`).
//!
//! Populated by Phase 5 (station-explorer data layer).
