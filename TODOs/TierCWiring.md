# Tier C Wiring — Making GBR Live Calls Actually Work

_The networking layer (Coalescer, RateLimiter, CircuitBreaker, LiveGbrClient) is fully
built and tested in isolation but is never called at runtime. Tier C is completely
non-functional. This epic wires it up end-to-end and fixes the correctness issues that
only become relevant once live writes are happening._

_This is the highest-impact single epic in the backlog. AdvancedAnalytics Phase 1
(preceding-service correlation) and state-machine natural transitions (2.6) both become
fully effective once this is complete._

_Prereqs: None. But complete this before TechnicalDebt item 4.5 (remove dead_code
suppression) and AdvancedAnalytics Phase 1 reaching its full runtime potential._

---

## Items from Improvements.md

### 2.1 — Wire networking → PollManager (LARGE)
`PollManager::fire_poll` emits a `StateChangeEvent` on the broadcast channel, but nothing
consumes that event to trigger an actual GBR API call.

**Fix — spawn a "poll consumer" task in `main.rs`:**

1. Subscribe to `state_change_rx` (clone from the broadcast sender).
2. Filter for events where `old_state == new_state` — these are "poll fired" events, not
   real state promotions.
3. For each, call `Coalescer::get(train_id)`. The Coalescer fans concurrent callers for
   the same TrainID into a single HTTP call.
4. Write the result back into the registry via `registry.update(id, |status| { ... })`.
5. Thread `CircuitBreaker` and `RateLimiter` into this path:
   - Check `circuit_breaker.is_open()` before calling Coalescer; if open, skip and
     increment the `circuit_breaker_blocked_total` metric (already defined).
   - `RateLimiter::acquire()` before each outbound call.
6. Instantiate `LiveGbrClient`, `Coalescer`, `RateLimiter`, `CircuitBreaker` in `main.rs`
   before spawning tasks. Pass `Arc` clones where needed.
7. Use `Arc<RwLock<CircuitBreaker>>` — it is already `Arc`-wrapped in the networking module.

**Concurrency note:** The poll consumer task is a single `tokio::spawn` with an async
loop — do not spawn one task per poll event, as that would create unbounded parallelism.
Use the Coalescer to absorb concurrent GBR calls for the same train.

---

### 2.2 — GBR JSON response is never parsed
`LiveGbrClient::get_train_status` returns `Err(GbrClientError::NotFound)` for all 200
responses. No serde structs exist for the RTT API v1 response schema.

**Fix:**
- Define `GbrTrainStatusResponse` and `GbrDeparture` in `src/networking/gbr_client.rs`
  matching the RTT API v1 schema. The RTT endpoint is:
  `GET /api/json/search/{station}/{date}` and `GET /api/json/service/{uid}/{date}`.
  Key fields: `serviceUid`, `locationDetail.realtimeArrival`, `locationDetail.realtimeDeparture`,
  `locationDetail.platform`, `isPassengerTrain`, `serviceType`, `trainIdentity` (headcode).
- Deserialise the 200 body with `serde_json`.
- Map into `TrainStatus` fields: `reported_delay_mins` (difference between realtime and
  scheduled departure), `actual_platform`, `is_cancelled`.
- Embed the RTT response's `realtimeDeparture` timestamp so 2.1's poll consumer can
  compare it against `Stamped::last_updated` for the last-writer-wins fix (item 8.1).

---

### 2.6 — State machine transitions never happen after registration
`TrainState::from_departure` is only called at registration. Trains never naturally
progress from `Dormant → Monitored → Active` — they stay in their initial state forever.

**Fix (depends on 2.1 poll consumer existing):**
- In the poll consumer task (2.1), after writing the GBR result back into the registry:
  1. Read the train's current `scheduled_departure` and `actual_estimated_departure`.
  2. Call `TrainState::from_departure(departure_time, Utc::now())`.
  3. Compare with the stored `TrainState` on the status.
  4. If the state has changed, emit a real `StateChangeEvent` with the new state and
     re-queue the train in `PollManager` at the new interval.
- This means the poll consumer both reads from and writes to the state machine — that is
  intentional and documented in `TODOs/StateMachine.md`.

---

### 8.1 — "Last writer wins" race between Darwin push and GBR poll
Once 2.1 is wired, Darwin STOMP updates and GBR poll writes will race on the same
`Arc<RwLock<TrainStatus>>`. The RwLock serialises them but the GBR response (fetched
before the Darwin message arrived) can overwrite the Darwin update.

Darwin is always more current than a polled GBR response.

**Fix (depends on 2.2 response timestamps being parsed):**
- Before applying any GBR poll result in the poll consumer (2.1), compare the RTT
  response's embedded timestamp against `Stamped::last_updated` on the fields being
  written.
- Only apply a field update if the GBR timestamp is newer than the stored `last_updated`.
- This mirrors the `SequenceGuard` pattern already enforced for Darwin messages in
  `src/ingestion/filter.rs` — use the same "never overwrite with older data" principle.

---

### 4.5 — Remove `#[allow(dead_code)]` suppressors (do this last)
Eleven uses of `#[allow(dead_code)]` across `networking/`, `ingestion/stomp_client.rs`,
and `api/mod.rs` suppress warnings that flag un-called code. Most are legitimate today
because the networking layer is built in isolation.

**Fix (only after 2.1–2.3 are complete):**
- Remove all `#[allow(dead_code)]` attributes across the codebase.
- Compile with `cargo clippy -- -D warnings` and address any remaining dead-code warnings.
- Any code that is truly unreachable after wiring should be deleted, not suppressed.

---

## Priority Order

1. 2.2 (parse GBR JSON) — unlocks everything downstream; can be done in parallel with 2.1
2. 2.1 (wire networking → PollManager) — the core wiring
3. 2.6 (state transitions) — natural follow-on once poll consumer exists
4. 8.1 (last-writer race) — correctness fix once both 2.1 and 2.2 are done
5. 4.5 (dead_code cleanup) — final cleanup pass

---

## Files Expected to Change

- `RailPredict/src/main.rs` (spawn poll consumer task, instantiate networking stack)
- `RailPredict/src/networking/gbr_client.rs` (GBR response structs + parsing)
- `RailPredict/src/networking/coalescer.rs` (verify wiring, no logic change expected)
- `RailPredict/src/networking/circuit_breaker.rs` (call-site integration)
- `RailPredict/src/networking/rate_limiter.rs` (call-site integration)
- `RailPredict/src/state_machine/poll_manager.rs` (re-queue on state change)
- Any file with `#[allow(dead_code)]` in `networking/` or `ingestion/` (cleanup pass)
