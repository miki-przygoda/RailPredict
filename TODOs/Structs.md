# TODOs/Structs.md – The Data Architecture

In a high-concurrency system, you need to decide between Shared State (using `Arc<Mutex<T>>`) or Message Passing (using channels). For a rail engine, a hybrid approach is best.

### Key considerations for your Structs:

1. **The "Normalized" Train**:
    - Define a `TrainStatus` struct that acts as the "Single Source of Truth."
    - It must be able to ingest updates from three distinct sources: the REST API (polling), the STOMP Firehose (push), and your Prediction Engine (ML/Stats).

2. **The Identifier Problem**:
    - UK rail uses `RID` (Reference ID), `UID` (User ID), and `Headcode` (e.g., 1A23).
    - Your struct needs a robust mapping logic so that an update for `RID 20240417...` correctly updates the same object as `Headcode 1A23`.

3. **Temporal Precision**:
    - Use the `chrono` crate.
    - Distinguish between `ScheduledDeparture`, `PublicDeparture` (the time passengers see), and `ActualEstimatedDeparture`.
    - Consider storing `LastUpdated` timestamps on every field to handle "Stale Data" logic.

4. **The Volatility Score**:
    - Add a field for a `VolatilityContext` struct. This should hold metadata like current weather at the train's location and historical reliability coefficients.

---

### Before Starting: Pre-work Subtasks

These must be done before writing any implementation code for this epic.

- [ ] **Cargo.toml: add all type-layer dependencies** — `chrono` (with `serde` feature), `serde` + `serde_derive`, `thiserror`, `anyhow`. Pin versions and verify they compile together before writing a single struct.

- [ ] **Decide the ownership model upfront** — will `TrainStatus` be owned behind an `Arc<RwLock<TrainStatus>>` in a shared registry, or will updates flow through channels as owned values? This decision determines whether `TrainStatus` needs to be `Clone`, and whether its fields can hold non-`Send` types. Write the decision down as a comment in `src/types/mod.rs` before anything else.

- [ ] **Read the Darwin XML schema for real identifier formats** — RID is a 15-character string with date prefix (e.g., `202404170123456`). UID is a 6-character alphanumeric. Headcodes are 4-character (letter-digit-digit-digit). Understand these before designing the `TrainID` enum so no variant is under-specified.

- [ ] **Sketch the `TrainID` mapping table** — in real Darwin messages, a single train appears under different identifiers in different message types. Map which message type uses which identifier before writing the enum conversion logic. A wrong assumption here is the most expensive thing to refactor later.

- [ ] **Create the module skeleton before any implementation** — create `src/types/mod.rs`, `src/types/train_id.rs`, `src/types/train_status.rs`, `src/types/volatility.rs` as empty files with `//! TODO` doc comments. Declare the modules in `main.rs`. Ensure the project compiles on an empty skeleton before adding any logic.

- [ ] **Set up the test file structure from day one** — add a `#[cfg(test)]` module to each type file immediately. Write at least one placeholder test per file. The types layer is the thing everything else mocks against; catching regressions early here is worth more than anywhere else.

- [ ] **Note: `VolatilityContext` data sources are deferred** — the struct can be defined now (fields for weather reading and reliability coefficient), but the live weather feed integration is a separate concern. Define the struct with `Option<T>` fields where live data will eventually go. Do not block type design on having a weather API key.
