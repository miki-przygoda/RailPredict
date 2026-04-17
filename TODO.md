# TODO

The current version and last worked on date should be noted at the top of this file below this line:

**version = "0.2.0" -- 17/04/2026**

---

## How This File Works

- This is the active sprint tracker. It reflects what needs to be done **right now**.
- Larger epics live in `TODOs/` as their own files. Link to them from here when active.
- **Versioning rules (strict):**
  - Each completed TODO point → patch bump: `x.x.1 → x.x.2`
  - Each completed section/epic → minor bump: `x.1.x → x.2.0`
  - On any version bump, update the version string in: `CLAUDE.md`, `README.md`, `TODO.md`, `CHANGELOG.md`, `RailPredict/Cargo.toml` — all five, simultaneously.
  - Note the completion in `CHANGELOG.md` before moving on.
- `TODOs/` files are created when an epic starts and **destroyed** when it is complete. On destruction: summarise into `CHANGELOG.md`, bump the minor version, update all five files.
- The version header format and this section header are **permanent** — do not modify them.

---

## Current Epics

### ~~Epic 1 — Core Data Types~~ — COMPLETE (v0.2.0)

### Epic 2 — State Machine (`TODOs/StateMachine.md`)
The polling pulse of the system. Depends on Epic 1 types being in place.

- [ ] Create `src/state_machine/` module
- [ ] Implement `TrainState` enum: `Dormant`, `Monitored`, `Active`, `Critical`
- [ ] Implement time-based transition logic (promotion and demotion)
- [ ] Implement emergency promotion logic (volatility triggers)
- [ ] Build global poll manager using `BinaryHeap` ordered by next-poll time
- [ ] Wire up `mpsc` broadcast channel for state change notifications

### Epic 3 — Networking Layer (`TODOs/Networking.md`)
The GBR API interface with all safeguards. Depends on Epic 1 types.

- [ ] Create `src/networking/` module
- [ ] Implement `gbr_client.rs` — `reqwest`-based GBR REST wrapper
- [ ] Implement request coalescer (`oneshot` fan-out pattern)
- [ ] Implement rate limiter (token bucket; configurable threshold)
- [ ] Implement circuit breaker (503 detection → Cache Only mode → cool-down)

### Epic 4 — Data Ingestion (`TODOs/DataIngestion.md`)
Darwin STOMP firehose connection and processing pipeline. Depends on Epic 1 types.

- [ ] Create `src/ingestion/` module
- [ ] Implement STOMP client connecting to Darwin Push Port
- [ ] Implement region/route filter (applied **before** any parsing)
- [ ] Implement sequence-aware update logic (never overwrite newer with older)
- [ ] Implement Darwin XML parser using `serde-xml-rs` → internal types

### Epic 5 — In-Memory Cache
The fast local store that all read queries hit. Depends on Epics 1 and 4.

- [ ] Create `src/cache/` module
- [ ] Implement `train_registry.rs` using `moka` or `dashmap`
- [ ] Cache eviction policy (trains that have departed + buffer window)
- [ ] Cache warm-up from Tier A static data on startup

---

## Strategic Notes (Carry-Forward)

These are cross-cutting design decisions to keep in mind across all epics:

- **The Identifier Problem:** Use `TrainID` everywhere. Never pass raw strings for train identifiers across module boundaries.
- **The Waiter Pattern:** The coalescer in Epic 3 is the most impactful single piece of work for latency. Prioritise it.
- **Backpressure:** GBR will revoke API keys for aggressive polling. The rate limiter and circuit breaker are not optional.
- **The Ingestion Filter:** Apply the region/route filter as step one of ingestion. CPU cost compounds fast on an unfiltered firehose.
