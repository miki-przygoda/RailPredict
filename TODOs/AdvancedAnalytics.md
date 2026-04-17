# Advanced Analytics — Contextual & Correlative Predictions

_Prerequisite: Improvements.md items 2.1–2.3 (networking wired, GBR response parsed,_
_DB in AppState) must be complete before these are worth building._

> ⚠️ IMPLEMENTATION STATUS NOTE (added during v1.1.x work):
> All 4 phases have been implemented in code. However, the prediction engine is not yet
> exercised on live data at runtime because Improvements.md items 2.1 (wire networking →
> PollManager) and 2.2 (parse GBR JSON response) are not yet complete. The prediction
> engine IS called by the ingestion pipeline on Darwin messages, so Tier B predictions
> do accumulate from STOMP data. Full Tier C integration (GBR poll → predict → update)
> awaits Improvements 2.1–2.2. Item 2.3 (DB in AppState) has been completed separately
> as a prereq for the Observability epic.

---

## Background

The current Tier B engine computes a trimmed mean over the last 90 delay observations for
a `(uid, weekday, origin_crs)` pattern. This is a solid baseline but is **context-blind**:
it gives the same prediction on a sunny Tuesday at 09:00 as on a Tuesday with a signal
failure at the preceding junction. The goal here is to enrich predictions with signals
that are knowable at prediction time.

---

## Phase 1 — Preceding Service Correlation

### The problem
If the train scheduled 10 minutes ahead of yours at the same station is running 15 minutes
late, it almost certainly means yours will be too — the platform is occupied, the path is
blocked. No amount of historical averaging captures this because it is a real-time
condition, not a recurring pattern.

### How to implement

**Step 1: Detect the preceding service.**
The simplest approach requires no new DB schema. At prediction time, scan the current
registry for services sharing the same `origin_crs` whose `scheduled_departure` is in the
window `[departure - 20 mins, departure - 1 min]`. The one with the latest `scheduled_departure`
before the target train is the preceding service. If it has a `reported_delay_mins > 5`,
treat it as a correlation signal.

**Step 2: Weight the prediction.**
In `PredictionEngine::predict_and_update`, after computing the trimmed mean, check if a
preceding-service delay was found. If so, apply a weighted blend:
```
adjusted = (trimmed_mean * 0.6) + (preceding_delay * 0.4)
```
The 0.6/0.4 split is a starting guess — it should be tunable via config and validated
against historical data once enough is accumulated. Surface the blend weights in the
prediction output so callers can reason about it.

**Step 3: Add a `correlation_signal` field to `VolatilityContext`.**
```rust
pub correlation_signal: Option<CorrelationSignal>,
```
```rust
pub struct CorrelationSignal {
    pub preceding_rid: TrainId,
    pub preceding_delay_mins: i32,
    pub weight: f32,
}
```
This makes the signal auditable — the UI can show "Based partly on the preceding
service (RID …) which is currently 15 mins late."

### Note on Darwin `association` messages
Darwin sends `<association>` messages for coupled/split services. These are currently
`DROP`ped in `filter.rs`. They contain explicit linking between service IDs and would
give a more precise preceding-service relationship than the time-window scan above.
Before implementing the time-window approach, consider whether processing `association`
messages and storing the links would be worth the added complexity.

### Implementation Status — COMPLETE
Changed files:
- `src/types/volatility.rs`: Added `CorrelationSignal` struct and `correlation_signal: Option<CorrelationSignal>` field to `VolatilityContext`
- `src/prediction/engine.rs`: `predict_and_update` now accepts an optional registry snapshot slice, scans for preceding services sharing `origin_crs`, and applies 0.6/0.4 weighted blend when preceding service has `reported_delay_mins > 5`. Blend weights defined as named constants `PRECEDING_WEIGHT` (0.4) and `HISTORY_WEIGHT` (0.6).

TODO (awaiting Improvements 2.1–2.2): The caller of `predict_and_update` must pass a registry snapshot slice for the correlation scan to have any effect. Until then, no preceding-service correlation occurs but the code compiles and is ready to wire.

---

## Phase 2 — TIPLOC Cascade (Knock-on Delay Propagation)

### The problem
A delay at one calling point propagates forward: if the 12:00 from Leeds is late into
York, everything connecting into York or departing York on the same track in the next
20 minutes is affected. This is a corridor effect, not just a per-train effect.

### What TIPLOC gives you
Darwin uses TIPLOC (Timing Point Location) codes — the `tpl` attribute on each
`<Location>` in a TS message. Two services sharing a TIPLOC in overlapping time windows
are physically competing for the same infrastructure.

### How to implement

**Step 1: Store per-call TIPLOCs.**
Currently `TrainStatus` stores `origin_crs` and `uid` but not the full calling pattern.
Add a `calling_points: Vec<(String, DateTime<Utc>)>` field (TIPLOC + scheduled time)
populated from Darwin TS `<Location>` elements as they arrive.

**Step 2: Build a reverse index.**
In `TrainRegistry`, maintain a secondary `DashMap<String, Vec<TrainId>>` mapping each
active TIPLOC to the trains that will call there within the next 60 minutes. Update it
whenever calling points change.

**Step 3: Propagate on delay signal.**
In `IngestionPipeline::process_frame`, after a delay is detected for train A, look up
A's next TIPLOC in the reverse index and emit a "cascade" signal to all other trains
scheduled through that TIPLOC within ±20 minutes. The cascade signal boosts their
`VolatilityContext::incident_flagged` and forces a `TrainState::Active` promotion
regardless of departure time.

### Implementation Status — COMPLETE
Changed files:
- `src/types/train_status.rs`: Added `calling_points: Vec<(String, DateTime<Utc>)>` field, initialised as empty vec
- `src/cache/train_registry.rs`: Added `tiploc_index: DashMap<String, Vec<TrainId>>` secondary index; added `update_tiploc_index`, `trains_at_tiploc`, and `cascade_trains_for_tiploc` methods
- `src/ingestion/filter.rs`: Added `check_tiploc_cascade` free function that returns `Vec<TrainId>` to promote to Active

TODO (awaiting wiring): `ingestion/mod.rs` (owned by another agent) must call `check_tiploc_cascade` after delay detection and set `incident_flagged = true` on returned train IDs. A TODO comment is in place in `filter.rs`.

---

## Phase 3 — Historical Reliability per Day-of-Week + Hour

### The problem
The current `ServicePattern` key is `(uid, weekday, origin_crs)`. This collapses all
departures across the day: a service that's reliably on-time at 14:00 and reliably late
at 08:00 rush hour gets the same prediction for both.

### Fix
Add a `departure_hour: u8` field to `ServicePattern`. This splits the history ring
into 24 sub-patterns per service-day. You need enough data per bucket to reach the
3-sample minimum (defined in `engine.rs`), so reduce `MAX_SAMPLES` per bucket or raise
the minimum sample count accordingly.

**Schema impact:** `delay_history` grows a `departure_hour SMALLINT NOT NULL` column,
and the unique index becomes `(uid, weekday, origin_crs, departure_hour, recorded_at)`.
Write a migration before implementing.

### Implementation Status — COMPLETE
Changed files:
- `migrations/20240417120005_add_departure_hour.sql`: New migration adding `departure_hour SMALLINT NOT NULL DEFAULT 0`, dropping old unique index, creating new one on `(uid, weekday, origin_crs, departure_hour, recorded_at)`
- `src/prediction/types.rs`: Added `departure_hour: u8` to `ServicePattern`
- `src/prediction/engine.rs`: `derive_pattern` now populates `departure_hour` from `status.scheduled_departure.value.hour() as u8`
- `src/db/history.rs`: Updated `DelayRow`, `load_history` query, and `flush_history` INSERT/CONFLICT clause to include `departure_hour`

---

## Phase 4 — Confidence Decay for Stale History

### The problem
A `ServicePattern` that hasn't seen a new `DelayRecord` in 6 weeks (e.g. a seasonal
service, or a temporarily suspended route) still returns its last-known confidence. That
confidence number is misleading.

### Fix
In `PredictionEngine::predict_and_update`, before returning the prediction, compute the
age of the most recent `DelayRecord.recorded_at` for the pattern. If it is more than
`STALENESS_THRESHOLD` (suggested: 21 days), multiply the confidence by a decay factor:
```
decayed_confidence = confidence * exp(-days_since_last_record / 21.0)
```
This causes confidence to approach zero as history ages. Surface `decayed_confidence`
in the output alongside the raw confidence.

### Implementation Status — COMPLETE
Changed files:
- `src/prediction/types.rs`: Added `most_recent_recorded_at` method to `HistoricalStore`
- `src/prediction/engine.rs`: After computing confidence, fetches most-recent `recorded_at` for the pattern; applies exponential decay if older than `STALENESS_THRESHOLD_DAYS` (21). `historical_reliability` in `VolatilityContext` now stores the decayed value. Raw confidence is preserved in the local variable for transparency (logged at TRACE level).

---

## Priority Order

1. Phase 1 (preceding service correlation) — highest impact, low schema cost, actionable with current data
2. Phase 4 (confidence decay) — small change, improves prediction honesty significantly
3. Phase 3 (hour-of-day buckets) — requires migration, meaningful accuracy gain
4. Phase 2 (TIPLOC cascade) — highest engineering cost, highest potential impact
