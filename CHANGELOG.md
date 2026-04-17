# Changelog

The current version and last worked on date should be noted at the top of this file below this line:

**version = "0.7.0" -- 17/04/2026**

---

## v0.7.0 — 17/04/2026 — Epic 6: Runtime Wiring & Observability

Completed all items in `TODOs/RuntimeWiring.md`. Destroyed that file on completion.

- Added `tracing`, `tracing-subscriber` (env-filter + json), `tokio-util`, `dotenvy` to `Cargo.toml`
- Added `[lib]` target (`railpredict`) alongside binary so `tests/` can import modules directly
- **`src/config.rs`** — `Config::from_env()` collects all missing required vars into a single `ConfigError`; `Config::for_testing()` for zero-env unit tests; full env var inventory documented in table
- **`src/lib.rs`** — re-exports all modules as the public library surface
- **`src/main.rs`** — `#[tokio::main]`, `dotenvy::dotenv()`, fail-fast config load, `init_tracing()` (pretty/JSON from `LOG_FORMAT`), shared `CancellationToken`, spawns `PollManager`, `IngestionPipeline`, eviction task (60s tick); graceful `ctrl_c` drain with `tokio::join!`; starts cleanly without Darwin credentials (logs a warning and waits for shutdown)
- **Tracing throughout ingestion** — replaced `eprintln!` placeholder with structured `tracing::info/warn/error/debug/trace` calls; `rid` field carried on key events
- **Bug fix**: first TS message for an unregistered train silently dropped platform/departure fields — restructured `process_frame` to register then always apply, regardless of whether the entry pre-existed
- **`tests/integration.rs`** — 5 integration tests against the real public wiring: smoke (T1→T2→T3→deactivated), ascending-TS registry state, stale reconnect replay, graceful shutdown, cancellation event propagation
- 83 tests total (78 unit + 5 integration), all passing

---

## v0.6.0 — 17/04/2026 — Epic 5: In-Memory Cache

Completed all items in the Epic 5 cache section of `TODO.md`.

- `src/cache/train_registry.rs` — `TrainRegistry` backed by `dashmap::DashMap<TrainId, Arc<RwLock<TrainStatus>>>` for sharded concurrent access
- `update()` closure API applies mutations without replacing the full entry
- `evict_departed()` removes trains beyond a 5-minute post-departure buffer window
- `warm()` loads a batch of Tier A static statuses at startup
- 7 unit tests covering upsert, get, update, remove, eviction, and warm-up

---

## v0.5.0 — 17/04/2026 — Epic 4: Data Ingestion

Completed all items in `TODOs/DataIngestion.md`. Destroyed that file on completion.

- Added `quick-xml = "0.37"` and `dashmap = "6"` to `Cargo.toml`
- **STOMP client** (`stomp_client.rs`): `StompClient` trait + `LiveStompClient` (raw STOMP framing over `tokio::net::TcpStream`, no dormant crate dependency) + `MockStompClient` for tests; `StompConfig` reads from env vars
- **Filter** (`filter.rs`): full Darwin message taxonomy (`TS`=KEEP, `deactivated`=KEEP, `schedule`/`SF`=CONDITIONAL, `OW`/`trainAlert`/`association`/`alarm`=DROP); region/route CRS filter; `SequenceGuard` per-TrainId timestamp check prevents stale overwrites and handles STOMP reconnect replays
- **Parser** (`parser.rs`): `quick-xml` event-based parser (chosen over `serde-xml-rs` for namespace robustness and zero-copy streaming); handles `TS` and `deactivated` messages; extracts RID, platform, estimated/actual departure, cancellation flag
- **Pipeline** (`mod.rs`): `IngestionPipeline` wires STOMP → filter (pre-parse) → sequence guard → parser → registry write; emergency promotions (cancellation, major delay) broadcast on Epic 2 `mpsc` channel — no direct cross-module function call; `INGESTION_BUFFER=512` with documented rationale
- 74 unit tests total (31 new), all passing

---

## v0.4.0 — 17/04/2026 — Epic 3: Networking Layer

Completed all items in `TODOs/Networking.md`. Destroyed that file on completion.

- Added `reqwest` (rustls-tls, json) and `async-trait` to `Cargo.toml`
- `GbrClient` trait — testable interface; `LiveGbrClient` reads `GBR_API_KEY` from env, never hardcodes credentials; `MockGbrClient` for unit tests
- GBR endpoint constants documented in `gbr_client.rs`; JSON response parsing deferred pending Darwin credentials
- `RateLimiter` — token-bucket; `MAX_REQUESTS_PER_SECOND=10`, `BURST_CAPACITY=20` with source rationale documented
- `CircuitBreaker` — `Closed/Open/HalfOpen` state machine; `FAILURE_THRESHOLD=3`, `COOL_DOWN_SECS=30`; transitions fully documented
- `Coalescer` — `oneshot` fan-out; single HTTP call for N concurrent callers on same `TrainId`; lock held only briefly, never across await; key design decision documented
- 43 unit tests total (15 new), all passing; coalescer test verifies exactly 1 HTTP call for 5 concurrent requests

---

## v0.3.0 — 17/04/2026 — Epic 2: State Machine

Completed all six items in `TODOs/StateMachine.md`. Destroyed that file on completion.

- Added `tokio` (features = ["full"]) to `Cargo.toml`
- `TrainState` enum: `Dormant`, `Monitored`, `Active`, `Critical`, `Terminal` — full transition diagram documented in source
- Time-based `from_departure()` rule set: single authoritative function covering all promotion, demotion, and terminal cases
- `PromotionReason` enum: `TimeBased`, `VolatilityTriggered`, `IncidentDetected` (last arm deferred to Epic 4)
- `emergency_promote()` for volatility/incident bypass of time thresholds
- `PollEntry` min-heap type with correct `Ord` impl for `BinaryHeap`
- `PollManager` with `tokio::select!` loop racing sleep against registration channel
- Bounded `mpsc` channels: `STATE_CHANGE_BUFFER=256`, `REGISTRATION_BUFFER=64` with rationale documented
- `StateChangeEvent` broadcast on each poll fire
- 28 unit tests total (11 new), all passing

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
