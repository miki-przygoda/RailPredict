# Changelog

The current version and last worked on date should be noted at the top of this file below this line:

**version = "0.2.0" -- 17/04/2026**

---

## v0.2.0 — 17/04/2026 — Epic 1: Core Data Types

Completed all six items in `TODOs/Structs.md`. Destroyed that file on completion.

- Created `src/types/` module with ownership model documented in `mod.rs`
- `TrainId` enum: `Rid` (15-char), `Uid` (6-char), `Headcode` (digit-letter-digit-digit) with validated constructors and `thiserror` error types
- `TrainStatus` struct: single source of truth; `Stamped<T>` wrapper provides per-field `last_updated` timestamps for stale-data detection
- Timestamp fields: `scheduled_departure`, `public_departure`, `actual_estimated_departure` (all `chrono::DateTime<Utc>`)
- `UpdateSource` provenance enum: `RestPoll`, `StompFirehose`, `PredictionEngine`
- `VolatilityContext` struct: `wind_speed_mph`, `historical_reliability`, `incident_flagged` — all optional pending live feed integration
- 17 unit tests, all passing
