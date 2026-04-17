# Changelog

The current version and last worked on date should be noted at the top of this file below this line:

**version = "1.1.1" -- 17/04/2026**

---

## v1.1.1 — 17/04/2026 — FrontEndHardening epic complete

Completes all four phases of `TODOs/FrontEndHardening.md`.

**Phase 3 — SSE "Live Updates Paused" banner**
- `src/frontend/layout.rs` — existing hidden `#stale-banner` slot given `warning-banner` class and "⚠ Live updates paused — showing last known state" text; inline JS extended to listen to `htmx:sseError` (show banner + add `data-stale` to `#live-status`) and `htmx:sseOpen` (reverse on reconnect).
- `static/style.css` — `.warning-banner` (amber background, black text); `#live-status.data-stale` (40% grayscale + amber left border).

**Phase 1 — Stale data overlay**
- `src/api/types.rs` — `DepartureBoardEntry` gains `last_updated_secs_ago: Option<u64>` and `destination_name: Option<String>`.
- `src/frontend/search.rs` + `src/api/handlers.rs` — staleness computed from max `last_updated` across `actual_estimated_departure`, `reported_delay_mins`, `actual_platform`, `is_cancelled`; cards older than 120 s emit `data-stale="true"`.
- `static/style.css` — `.train-card[data-stale="true"]` gets 60% grayscale + 0.75 opacity + amber `"stale"` `::after` label.

**Phase 4 — Destination station on departure board cards**
- `src/types/train_status.rs` — `pub destination_crs: Option<String>` added; initialised to `None`.
- `src/ingestion/parser.rs` — Location handler overwrites `destination_crs` with each `tpl` so after all locations are processed it holds the final (destination) CRS.
- `src/frontend/search.rs` — `span .train-destination { "→ " (dest) }` rendered on card. DB name lookup stubbed (renders raw CRS) pending `AppState::db` wiring.

**Phase 2 — Optimistic departure board**
- `src/frontend/search.rs` — form trigger changed from `hx-trigger="submit"` to `hx-trigger="submit, every 30s"` with `hx-swap="innerHTML transition:true"`; board stays populated between refreshes.

---

## v1.1.0 — 17/04/2026 — Docker & Database epic complete (Phases 4–6)

Completes all six phases of `TODOs/Docker.md`. That file is now destroyed.

**Phase 4 — DB module**
- `src/db/mod.rs` — `connect(database_url)` creates a `PgPool` (max 10 connections) and runs `sqlx::migrate!` on startup. Exposes `type Db = PgPool`.
- `src/db/static_data.rs` — `Station`, `TimetableCall`, `Fare` structs with `sqlx::FromRow`; `get_station()`, `departures_from()`, `cheapest_fare()` async query functions.
- `src/db/history.rs` — `load_history(db)` uses a window function to rebuild `HistoricalStore` from the most recent MAX_SAMPLES rows per pattern on startup; `flush_history(db, store)` batch-inserts all records with `ON CONFLICT DO NOTHING` in 500-row chunks for idempotency.
- `PredictionEngine::with_store(Arc<HistoricalStore>)` constructor + `arc_store()` accessor added to support startup loading and shared flush.
- `main.rs` wired: DB connect → load history → `PredictionEngine::with_store` → 60s flush task → final flush on SIGTERM/Ctrl-C via `shutdown_signal()` (listens for both `SIGTERM` and Ctrl-C on Unix).

**Phase 5 — CLI + GTFS ingest**
- `src/cli.rs` — `Cli` struct (clap derive); `Commands::IngestStatic { source, url, file }` subcommand; `IngestSource` enum (`Gtfs`, `Cif`).
- `src/ingestion/gtfs.rs` — `GtfsStation` (serde-deserialised from `stops.txt`); `parse_stops(csv_bytes)` pure function (testable without I/O) filters to valid 3-letter uppercase CRS codes; `run_ingest(db, url)` downloads ZIP → extracts `stops.txt` → parses → upserts stations via `QueryBuilder`; `run_ingest_from_file` variant for local archives. CIF is a stub returning `unimplemented!()`.
- 5 unit tests for `parse_stops` and `is_valid_crs`.
- `main.rs` parses CLI args first; if a subcommand is present it runs and exits before starting the server.

**Phase 6 — Production hardening**
- `docker-compose.prod.yml` — added `read_only: true` on the `app` service; all writable state lives in the DB.
- `src/db/mod.rs` documents pool sizing (max 10 connections) inline.
- 150 tests total (145 unit + 5 integration), all passing.

---

## v1.0.1 — 17/04/2026 — Docker infrastructure + database foundations (Phases 1–3)

Partial progress on `TODOs/Docker.md` (Phases 1, 2, and 3 complete; Phases 4–6 pending).
Epic remains open — `TODOs/Docker.md` is not destroyed until all six phases are done.

**Phase 1 — Dockerfile**
- Multi-stage `Dockerfile` using `cargo-chef`: `planner` → `cacher` → `builder` → `runtime`. Dependency compilation is a cached layer; code-only rebuilds reuse it entirely.
- `SQLX_OFFLINE=true` set in the builder stage; `.sqlx/` snapshot directory to be committed after first `cargo sqlx prepare` run locally.
- Runtime stage: `debian:bookworm-slim` with `ca-certificates` + `libssl3`; non-root `railpredict` user; `HEALTHCHECK` wired to `GET /health`.
- `.dockerignore` excludes `target/`, `data/`, `.env`, `*.md`, `.git/`.

**Phase 2 — Docker Compose**
- `docker-compose.yml`: `db` (Postgres 16-alpine, named volume, healthcheck), `app` (waits on DB healthcheck, env from `.env`), `ingest` (profile-gated, exits after completion), `pgadmin` (profile-gated dev tool on `localhost:5050`).
- `docker-compose.prod.yml`: overrides `app` to pull a tagged registry image (`RAILPREDICT_IMAGE`), removes host-side DB port exposure, sets `stop_grace_period: 30s`, forces `LOG_FORMAT=json`.
- `.env.example` committed with every variable stubbed and commented.
- `.gitignore` confirmed to exclude `.env`.

**Phase 3 — Migrations**
- `migrations/` directory at repo root; five SQL files managed by `sqlx-migrate`.
- `20240417120000_create_stations` — `CHAR(3)` CRS primary key, GIN full-text index on name.
- `20240417120001_create_services` — UID + days bitmask (`SMALLINT`, 0–127), FKs to stations.
- `20240417120002_create_timetable_calls` — per (uid, date, location) call; indexed for departure-board and train-detail queries.
- `20240417120003_create_fares` — price in pence (no float), validity windows, composite PK.
- `20240417120004_create_delay_history` — Tier B persistence; unique composite index on `(uid, weekday, origin_crs, recorded_at)` prevents duplicate flush writes; no FK to services (intentional).

**Cargo.toml additions**
- `sqlx = "0.8"` with `runtime-tokio-rustls`, `postgres`, `migrate`, `chrono` features.
- `clap = "4"` with `derive` feature (for the ingest CLI subcommand in Phase 5).
- Project compiles cleanly with both new dependencies.

---

## v1.0.0 — 17/04/2026 — Unit test coverage pass

Added tests for all modules that were missing coverage. No logic changes.

- **`api/types.rs`** — 9 new tests: `ApiError` factory methods, HTTP status mapping (`NOT_FOUND`→404, `BAD_REQUEST`→400, `INTERNAL_ERROR`→500), JSON round-trips for `TrainSummary`, `DepartureBoardEntry`, `LiveUpdateEvent`.
- **`prediction/types.rs`** — 5 new tests: direct `HistoricalStore` coverage (unknown pattern returns `None`, insert/get round-trip, cap enforcement, oldest-eviction correctness, distinct patterns tracked independently).
- **`state_machine/train_state.rs`** — 9 new tests: boundary values at exactly 120/30/5 mins and one below each, `IncidentDetected` emergency promote for all base states, `Display` for all five variants. Boundary tests pin `now` to avoid sub-millisecond drift.
- **`ingestion/filter.rs`** — 7 new tests: SF/trainAlert/association/alarm classification, `forget` resets sequence guard, Drop message blocked on watched route, watched-route filter with no CRS.
- **`types/train_status.rs`** — 3 new tests: `Stamped::is_stale()` both directions, `Stamped::new` timestamp, `best_platform()` with both fields absent.
- **`ingestion/parser.rs`** — 5 new tests: multiple TS in one Pport, `at` attribute, missing `rid`/`ssd` → error, mixed TS+deactivated in one message.
- **`cache/train_registry.rs`** — 7 new tests: `update()` returns false for unregistered, `snapshot_all()`, empty snapshot, `warm()` empty iterator, `is_empty()`, eviction using `actual_estimated_departure`.
- **`types/train_id.rs`** — 7 new tests: headcode format rejections, equality within/across variants, `as_str` for all variants, UID display.
- 139 unit tests + 5 integration tests, all passing.

---

## v0.9.0 — 17/04/2026 — Epic 8: Tier B Prediction Engine

Completed all items in `TODOs/PredictionEngine.md`. Destroyed that file on completion.

- **`src/prediction/types.rs`** — `ServicePattern` struct keyed on `(uid, weekday, origin_crs)` — the stable recurring-service identity, not the daily-changing RID; `DelayRecord` (`delay_mins: i32, recorded_at: DateTime<Utc>`); `HistoricalStore` (`DashMap<ServicePattern, VecDeque<DelayRecord>>`, hard-capped at `MAX_SAMPLES = 90` per pattern — ~13 weeks of daily observations)
- **`src/prediction/engine.rs`** — `PredictionEngine` wraps `Arc<HistoricalStore>`, derives `Clone` for cheap task-boundary passing; `predict_and_update(&self, status: &mut TrainStatus)` derives the `ServicePattern`, computes trimmed-mean delay (drop top/bottom 10%) and confidence (`samples / MAX_SAMPLES`), writes `predicted_delay_mins` and `volatility.historical_reliability` — does nothing if fewer than 3 samples exist (no fabrication rule); `record_outcome()` appends a confirmed Darwin delay back into the store for incremental learning
- **`TrainStatus.uid`** — new field; populated from the first Darwin `TS` message carrying a `uid` attribute; `parser.rs` extracts it and propagates through `TsUpdate`
- **`ingestion/mod.rs`** — wires `PredictionEngine` into the pipeline: `record_outcome` then `predict_and_update` called on every TS update after `reported_delay_mins` is set; `PredictionEngine` passed into `IngestionPipeline` at construction time
- **`main.rs`** — `PredictionEngine::new()` constructed once at startup; `Arc` clone passed to `IngestionPipeline`
- 7 new unit tests (`predict_with_no_history_returns_none`, `predict_with_uniform_history_returns_that_value`, `trimmed_mean_ignores_outliers`, `record_outcome_caps_at_max_samples`, `confidence_scales_with_sample_count`, and two wiring tests); 90 tests total (85 unit + 5 integration), all passing

---

## v0.8.0 — 17/04/2026 — Epic 7: HTTP API + Rust/maud/htmx Frontend

Completed all items in `TODOs/UI.md`. Destroyed that file on completion.

**Sub-Epic A — axum HTTP API**

- **`src/api/types.rs`** — client-facing DTOs: `TrainSummary`, `DepartureBoardEntry`, `LiveUpdateEvent` (flat, serde-serialisable, no internal type leakage); `ApiError` with consistent `{ "error": "...", "code": "..." }` JSON envelope implementing `IntoResponse`; `HealthResponse` serving `CARGO_PKG_VERSION` at compile time
- **`GET /stations/{crs}/departures`** — Tier A: full registry snapshot filtered by origin CRS, sorted by scheduled departure, zero GBR calls
- **`GET /trains/{rid}`** — Tier B: registry lookup; `400` on invalid RID format, `404` if not found; `last_updated` derived from the most-recent field stamp on the `TrainStatus`
- **`GET /trains/{rid}/live`** — Tier C SSE: subscribes caller to the broadcast `state_change_tx`, filters for the requested RID, enriches each event with a live registry read, streams as `text/event-stream` JSON; 15s `KeepAlive` prevents proxy timeout; stream closes cleanly on `Terminal` state
- **`GET /health`** — always 200, returns version string
- `CorsLayer::permissive()` + `TraceLayer::new_for_http()` applied to all routes; API contract table documented in `src/api/mod.rs` header comment
- `AppState` carries `Arc<TrainRegistry>` and `broadcast::Sender<StateChangeEvent>`; passed as axum `State` extractor; no `Extension` indirection
- axum server spawned in `main.rs` topology alongside `PollManager`, `IngestionPipeline`, and eviction task; all share the `CancellationToken`

**Sub-Epic B — Rust/maud/htmx Frontend**

- **`src/frontend/layout.rs`** — base chrome: `<html>`, `<head>` (htmx CDN script tag, CSS link), `<body>` wrapper, stale-data banner slot (hidden by default, revealed by htmx SSE error handler)
- **`src/frontend/components.rs`** — `delay_badge(minutes, cancelled) -> Markup` (green / amber / red by value); `platform_chip(platform) -> Markup`
- **`src/frontend/search.rs`** — `/` full-page maud render with origin CRS `<input>` and `hx-get` departure board; `GET /ui/stations/departures?crs=XXX` htmx fragment handler returning `<ul>` of `DepartureBoardEntry` rows
- **`src/frontend/detail.rs`** — `/trains/:rid/view` full-page maud render; train summary card, SVG route diagram shell; `hx-ext="sse" sse-connect="/trains/:rid/live"` wired on the live section; `GET /ui/trains/:rid/live` SSE HTML fragment handler for htmx OOB swaps
- **`static/style.css`** — dark-first palette (`#1a1a1a` background, `#00c853` rail-green accent, amber/red for delay states); embedded at compile time via `rust-embed` — binary has zero filesystem dependency at runtime
- Page routes (`/`, `/trains/:rid/view`) and fragment routes (`/ui/...`) merged into the same axum `Router` as the JSON API routes; `rust-embed` static handler serves `/static/:path`

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
