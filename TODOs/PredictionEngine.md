# TODOs/PredictionEngine.md — Tier B Prediction Engine

Implements the local prediction logic that populates `predicted_delay_mins` and
`historical_reliability` on `TrainStatus` / `VolatilityContext`. No external calls;
all inference runs from an in-memory historical dataset seeded at startup.

---

## Key Constraints

1. **No live calls on this path.** The prediction engine is pure Tier B — it reads from
   its own internal historical store and writes to `TrainStatus`. It must never touch the
   networking layer.

2. **Fallback, not replacement.** `predicted_delay_mins` is only consumed by
   `best_delay_mins()` when `reported_delay_mins` is `None` or stale. The prediction
   engine writes its value and steps aside — it never overwrites live Darwin data.

3. **Incremental learning.** Each time a Darwin `reported_delay_mins` arrives for a
   train that already has a prediction, the engine feeds the outcome back into its
   historical store for that service pattern. Predictions improve over runtime without
   a cold-start rebuild.

4. **Keyed on service pattern, not RID.** RIDs change daily. Historical data must be
   keyed on `(uid, weekday, origin_crs)` — the stable identity of a recurring service.

---

## Pre-work Subtasks

- [ ] **Define `ServicePattern`** — a key struct `(uid: String, weekday: chrono::Weekday,
  origin_crs: String)`. Implements `Hash + Eq` so it can be used as a `DashMap` key.
  Lives in `src/prediction/types.rs`.

- [ ] **Define `DelayRecord`** — a minimal struct `{ delay_mins: i32, recorded_at:
  DateTime<Utc> }`. The historical store holds a fixed-length ring of these per
  `ServicePattern` (cap at 90 samples — ~13 weeks of daily data). Lives alongside
  `ServicePattern` in `src/prediction/types.rs`.

- [ ] **Decide the prediction algorithm** — the simplest correct approach is a
  trimmed mean of the last N `DelayRecord` values (drop the top and bottom 10% to
  resist outliers). Confidence is `samples / MAX_SAMPLES` capped at 1.0. Document
  this choice as a comment at the top of `src/prediction/engine.rs` so it can be
  replaced without archaeology.

- [ ] **Design the write-back contract** — the engine exposes one method:
  `fn predict_and_update(&self, status: &mut TrainStatus)`. It reads `status.id`
  and `status.scheduled_departure` to derive the `ServicePattern`, computes the
  prediction, and writes `predicted_delay_mins` and `volatility.historical_reliability`
  in one atomic pass. No return value needed — callers check the fields directly.

- [ ] **Create module skeleton** — `src/prediction/mod.rs`, `src/prediction/types.rs`,
  `src/prediction/engine.rs`. Compile clean on stubs before adding logic.

---

## Implementation Tasks

- [ ] **`src/prediction/types.rs`** — `ServicePattern`, `DelayRecord`, and
  `HistoricalStore` (a `DashMap<ServicePattern, VecDeque<DelayRecord>>` with a
  `MAX_SAMPLES: usize = 90` cap enforced on insert).

- [ ] **`src/prediction/engine.rs` — `PredictionEngine` struct** — wraps an
  `Arc<HistoricalStore>`. Constructor takes no arguments (store starts empty; data
  accumulates at runtime). Derives `Clone` so it can be passed into async tasks without
  lifetime friction.

- [ ] **`predict_and_update` method** — derives `ServicePattern` from the status,
  looks up the store, computes trimmed mean delay and confidence, writes
  `predicted_delay_mins` and `historical_reliability`. If no history exists for the
  pattern, writes `None` / `None` and returns immediately — never fabricates a
  prediction from zero data.

- [ ] **`record_outcome` method** — called by the ingestion pipeline after a Darwin
  `reported_delay_mins` update lands on a `TrainStatus`. Derives the `ServicePattern`,
  appends a `DelayRecord` to the store, and trims to `MAX_SAMPLES` if over capacity.
  This is the incremental learning path.

- [ ] **Wire into `main.rs`** — construct one `PredictionEngine` at startup; pass an
  `Arc` clone to the ingestion pipeline (for `record_outcome` calls) and to the poll
  manager (to call `predict_and_update` after each REST poll updates a `TrainStatus`).

- [ ] **Wire into `TrainRegistry::update`** — after any closure that sets
  `reported_delay_mins`, call `engine.record_outcome(&status)` so the store is kept
  current with no extra call sites needed.

- [ ] **Unit tests** — in `src/prediction/engine.rs`:
  - `predict_with_no_history_returns_none`
  - `predict_with_uniform_history_returns_that_value`
  - `trimmed_mean_ignores_outliers`
  - `record_outcome_caps_at_max_samples`
  - `confidence_scales_with_sample_count`
