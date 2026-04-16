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