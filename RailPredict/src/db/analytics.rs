//! Read-only prediction-accuracy analytics for the `/predictions` explorer.
//!
//! Reads finalised rows from `prediction_outcomes` and the per-event
//! `prediction_snapshots` timeline. All queries are windowed on a rolling
//! `hours` bound and apply the same delay sanity filter used elsewhere
//! (`final_delay_mins BETWEEN -120 AND 600`) so the severe-delay tail of the
//! Darwin feed can't distort calibration and error figures.
//!
//! Populated by Phase 4 (prediction-explorer data layer).
