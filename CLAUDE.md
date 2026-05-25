# Claude

The current version and last worked on date should be noted at the top of this file below this line:

**version = "1.8.0" -- 25/05/2026**

---

## Project Snapshot

RailPredict is a Rust-based high-performance shadow system for UK Rail data. Its core purpose is to act as an **Intelligent Buffer** between users and the GBR (Great British Railways) API — minimising expensive live calls by combining cached static data, local predictive logic, and state-machine-driven polling. The system should feel instant to the user: data is pre-warmed, delays are predicted locally, and live API calls only happen when transactionally unavoidable.

---

## Working Principles

- **Before writing any code**, check the relevant `TODOs/` file for that domain. Each file contains architectural decisions and recommendations that should be treated as constraints, not suggestions.
- **Before modifying a file**, read its immediate neighbours in the module tree to understand the data flow context.
- **If a file has not been touched in several sessions**, leave a short inline note at the top of the file (as a Rust doc comment `//!`) marking what the module does, what state it was last left in, and what the next expected change is.
- **Never make a live GBR API call inside a hot path.** All external calls go through the networking layer with rate limiting and circuit-breaker logic.
- **Concurrency model**: prefer message passing (`mpsc`, `oneshot`) over shared mutable state (`Arc<Mutex<T>>`). Use `Arc<RwLock<T>>` only for read-heavy shared state like the train registry.
- **Error handling**: use `thiserror` for domain errors, `anyhow` for application-level error propagation. Never `.unwrap()` in production paths.

---

## Versioning Protocol

This is strict — follow it exactly:

1. Each completed TODO point bumps the patch version: `x.x.1 → x.x.2`.
2. Each completed TODO *section/epic* bumps the minor version: `x.1.x → x.2.0`.
3. When bumping, update the version string in **all five** of these files simultaneously:
   - `CLAUDE.md`
   - `README.md`
   - `TODO.md`
   - `CHANGELOG.md`
   - `RailPredict/Cargo.toml`
4. Note the completion in `CHANGELOG.md` before moving on.
5. The version header format is always: `**version = "x.x.x" -- DD/MM/YYYY**`

---

## Directory Map

```
RailPredict/                        ← repo root
├── CLAUDE.md                       ← this file; Claude session seed + working reference
├── README.md                       ← project vision + current status table
├── TODO.md                         ← active sprint tracker + remaining epics
├── CHANGELOG.md                    ← completed epics log; updated on minor version bumps
├── SECURITY.md                     ← secrets rotation procedure + security posture
├── deny.toml                       ← cargo-deny advisory/license config
├── prometheus.yml                  ← Prometheus scrape config (used by docker-compose monitoring profile)
├── migrations/                     ← sqlx SQL migrations (run at startup via sqlx::migrate!)
├── .github/workflows/ci.yml        ← GitHub Actions CI (deny → clippy → test → release build)
├── TODOs/
│   └── Improvements.md             ← full item index + cross-reference; all epics complete at v1.7.0
└── RailPredict/                    ← Rust crate root
    ├── Cargo.toml                  ← crate manifest; version must match project version
    ├── Cargo.lock                  ← committed; this is a binary application not a library
    ├── tests/
    │   ├── integration.rs          ← end-to-end wiring tests (real pipeline, in-memory)
    │   └── db_integration.rs       ← sqlx::test DB integration tests (11 functions)
    └── src/
        ├── main.rs                 ← tokio runtime init, CLI dispatch, top-level wiring
        ├── lib.rs                  ← re-exports all modules as public library surface
        ├── config.rs               ← Config::from_env(); all env vars documented here
        ├── cli.rs                  ← clap CLI: `ingest-static` subcommand
        ├── types/
        │   ├── mod.rs
        │   ├── train_id.rs         ← TrainId enum (Rid / Uid / Headcode) with validated constructors
        │   ├── train_status.rs     ← TrainStatus (single source of truth); Stamped<T> per-field timestamps
        │   └── volatility.rs       ← VolatilityContext; CorrelationSignal for Tier B
        ├── state_machine/
        │   ├── mod.rs
        │   ├── train_state.rs      ← TrainState enum + transition logic + emergency_promote
        │   └── poll_manager.rs     ← BinaryHeap-based global poll loop; mpsc STATE_CHANGE_BUFFER=256
        ├── networking/
        │   ├── mod.rs
        │   ├── gbr_client.rs       ← reqwest GBR REST wrapper; gbr_api_latency_ms histogram
        │   ├── coalescer.rs        ← oneshot fan-out; one HTTP call for N concurrent callers
        │   ├── rate_limiter.rs     ← token bucket; MAX_REQUESTS_PER_SECOND=10, BURST_CAPACITY=20
        │   └── circuit_breaker.rs  ← Closed/Open/HalfOpen; FAILURE_THRESHOLD=3, COOL_DOWN_SECS=30
        ├── ingestion/
        │   ├── mod.rs              ← IngestionPipeline; PipelineContext (Arc-shared, survives reconnect)
        │   ├── stomp_client.rs     ← Darwin STOMP over TLS (tokio-rustls); BoxReader abstraction
        │   ├── filter.rs           ← message taxonomy; SequenceGuard; check_tiploc_cascade
        │   ├── parser.rs           ← quick-xml event parser; TS + deactivated messages
        │   └── gtfs.rs             ← GTFS stops.txt → stations upsert; CIF stub
        ├── cache/
        │   ├── mod.rs
        │   └── train_registry.rs   ← DashMap registry; tiploc_index; eviction; cascade helpers
        ├── prediction/
        │   ├── mod.rs
        │   ├── engine.rs           ← PredictionEngine; trimmed-mean; correlation; confidence decay
        │   └── types.rs            ← ServicePattern (uid+weekday+origin+hour); HistoricalStore; DelayRecord
        ├── db/
        │   ├── mod.rs              ← connect(); type Db = PgPool; runs migrations at startup
        │   ├── static_data.rs      ← get_station, departures_from, cheapest_fare
        │   └── history.rs          ← load_history (window fn); flush_history (500-row chunks, idempotent)
        ├── weather/
        │   └── mod.rs              ← VolatilityStore; WeatherAnchor; fetch_wind_mph (Open-Meteo); run_weather_task (10min)
        ├── api/
        │   ├── mod.rs              ← axum Router; AppState; CorsLayer; GovernorLayer; infra_router
        │   ├── handlers.rs         ← departures_handler, journey_handler, train_handler, health_handler; validate_crs
        │   ├── sse.rs              ← /trains/:rid/live SSE stream; 15s KeepAlive
        │   └── types.rs            ← ApiError; DTOs: TrainSummary, DepartureBoardEntry, HealthResponse
        └── frontend/
            ├── mod.rs
            ├── layout.rs           ← base chrome; SSE error/reconnect banner JS
            ├── components.rs       ← delay_badge, platform_chip
            ├── search.rs           ← departure board; stale overlay; 30s auto-refresh
            └── detail.rs           ← train detail page; htmx SSE live section
```

---

## Architecture Quick Reference

### Three-Tier Data Flow

| Tier | Name          | Data                              | Source                        | Cost          |
|:-----|:--------------|:----------------------------------|:------------------------------|:--------------|
| A    | Static        | Timetables, station names, fares  | Weekly GTFS/CIF download      | Zero (local)  |
| B    | Predictive    | Likely delay, typical platform    | Historical data + weather     | Local compute |
| C    | Transactional | Live location, seat availability  | GBR Darwin / Retail API       | High          |

The rule: serve from the lowest tier possible. Only escalate to Tier C when the user is at checkout or when a Tier B prediction confidence falls below threshold.

### State Machine

| State       | Trigger Condition                          | Polling Behaviour               |
|:------------|:-------------------------------------------|:--------------------------------|
| `Dormant`   | Departure > 2 hours away                   | No live calls. Tier A only.     |
| `Monitored` | 120 > departure > 30 mins                  | Poll every 10 mins.             |
| `Active`    | 30 > departure > 0 mins                    | Poll every 30–60s.              |
| `Critical`  | Departure < 5 mins OR volatility triggered | Push-port stream or 10s polling |

**Volatility promotions** (bypass normal time-based transitions):
- Wind > 50mph on a route → force all trains on that route to `Active`
- Major incident detected (news/social scraper) → force affected corridor to `Critical`

---

## Key Patterns (Per Domain)

### Types (`src/types/`)
- `TrainId` is an enum wrapping `Rid`, `Uid`, and `Headcode` — all lookups go through it; never pass raw strings for train identifiers across module boundaries
- `TrainStatus` is the single source of truth; `Stamped<T>` gives every field its own `last_updated` timestamp for stale-data detection
- Use `chrono` for all timestamps; always distinguish `scheduled_departure`, `public_departure`, `actual_estimated_departure`

### State Machine (`src/state_machine/`)
- `enum TrainState { Dormant, Monitored, Active, Critical, Terminal }`
- Single global `PollManager` with a `BinaryHeap` ordered by next-poll time — never one `tokio::spawn` per train
- State changes broadcast via bounded `mpsc` (`STATE_CHANGE_BUFFER=256`); the API/UI layer subscribes — never locks the registry to check for changes

### Networking (`src/networking/`)
- Request coalescing (`coalescer.rs`): if a request for a `TrainId` is already in-flight, register a `oneshot` sender and wait; first responder fans the result to all waiters
- Rate limiter (`rate_limiter.rs`) sits in front of all outbound GBR calls; token bucket, `MAX_REQUESTS_PER_SECOND=10`, `BURST_CAPACITY=20`
- Circuit breaker (`circuit_breaker.rs`): on a 503 from GBR, enter "Cache Only" mode; `FAILURE_THRESHOLD=3`, `COOL_DOWN_SECS=30`; `#[cold]` on `record_failure`

### Data Ingestion (`src/ingestion/`)
- Apply region/route filter as **step one** in the pipeline — drop irrelevant messages before any parsing
- `SequenceGuard` in `filter.rs` prevents stale overwrites and handles STOMP reconnect replays; never overwrite a newer update with a late-arriving older one
- `PipelineContext` holds `Arc`-backed shared state (registry, broadcast tx, prediction engine, filter) so shared state survives STOMP reconnects; only the STOMP client is replaced
- `check_tiploc_cascade` in `filter.rs` detects knock-on delays via the TIPLOC index in `train_registry.rs` — wire into `ingestion/mod.rs` when Tier C is active

### Prediction Engine (`src/prediction/`)
- `ServicePattern` is keyed on `(uid, weekday, origin_crs, departure_hour)` — stable recurring-service identity, not the daily-changing RID
- `HistoricalStore` is hard-capped at `MAX_SAMPLES=90` per pattern; trimmed-mean drops top/bottom 10%; no prediction emitted with fewer than 3 samples
- `predict_and_update_with_correlation` blends in preceding-service delay (0.6/0.4 weight) when a service shares `origin_crs` in a ±20-min window
- Confidence decay: `confidence * exp(-days_since / 21.0)` applied when last `DelayRecord` is older than 21 days

### Database (`src/db/`)
- `load_history` uses a window function to reconstruct `HistoricalStore` from the most-recent `MAX_SAMPLES` rows per pattern — avoids full table scan
- `flush_history` inserts in 500-row chunks with `ON CONFLICT DO NOTHING` — idempotent; safe to call repeatedly
- `departures_from` is the hot path for the departure board; indexed on `(location_crs, operating_date, scheduled_departure)`

---

## References from HFT-Engine (`data/HFT-Engine/`)

A separate Rust project (gitignored under `data/`) that solved similar concurrency and ingestion problems at nanosecond scale. RailPredict does not need that level of latency, but the structural patterns are proven and directly portable. Do not copy inline assembly, PRFM prefetch hints, or NEON/AVX2 signal logic — those are HFT-specific. Everything below is domain-agnostic Rust.

### High-value direct ports

**1. SPSC lock-free ring buffer — `data/HFT-Engine/src/models.rs`: `RingBuffer` + `TradeLog`**
The pattern: `UnsafeCell<[T; N]>` for the backing array, `AtomicU64` write cursor, `Ordering::Release` on commit and `Ordering::Acquire` on read. Writer fills all struct fields first, then `fetch_add(1, Release)` to make the entry visible — never the other way around. This maps directly onto the Darwin ingestion pipeline: the STOMP receiver (writer) fills a parsed `TrainUpdate` slot, then commits; the state machine (reader) polls the cursor.

**2. Sequence gap detection + dirty flag — `data/HFT-Engine/src/engine.rs`: `run_ingestor`**
The ingestor tracks `last_ingest_seq` and on each received packet checks `recv_seq != last_ingest_seq + 1`. On a gap it sets a `dirty: AtomicBool` flag and increments `gap_count`. The consumer (trading strategy) skips processing while dirty and only clears it after `N` consecutive clean sequences. This is **exactly** the Darwin out-of-order / late-arrival problem described in `TODOs/DataIngestion.md`. Port this pattern verbatim into `src/ingestion/filter.rs`.

**3. `LatencyHistogram` — `data/HFT-Engine/src/models.rs`**
Fixed-bucket histogram covering 0–10,000 µs (one `u64` per bucket), overflow counter for values above the range, and an O(n) `percentile()` walk that requires zero allocation. Single-writer semantics (`UnsafeCell` + no lock). Directly useful for monitoring Darwin XML parse latency and state-machine poll timing. Copy this struct as-is into a `src/diagnostics/` module.

**4. Versioned JSON run log + `unix_to_date_time` — `data/HFT-Engine/src/engine.rs`: `write_log` + `unix_to_date_time`**
Writes structured logs to `logs/v{version}/{YYYY-MM-DD}/{HH-MM-SS}.json`. Version is read from `Cargo.toml` at compile time via `env!("CARGO_PKG_VERSION")` — stays in sync with the project version automatically. The `unix_to_date_time` function is a stdlib-only Gregorian calendar implementation (no `chrono`) for log path generation. Use this pattern for RailPredict's run and diagnostic logs.

**5. Pre-allocated flat instrument registry — `data/HFT-Engine/src/models.rs`: `InstrumentId` + `InstrumentBuffers`**
Instead of a `HashMap<InstrumentId, Arc<RingBuffer>>`, a compact `u8`-indexed newtype (`InstrumentId(pub u8)`) is used as an array index into a pre-allocated flat `[RingBuffer; MAX_INSTRUMENTS]`. O(1) lookup with zero heap allocation on the hot path. The equivalent in RailPredict is the `TrainID` → `TrainStatus` registry inside `src/cache/train_registry.rs`. A flat array keyed by a compact train index (populated from a startup lookup table) is faster and simpler than a `dashmap` if the active-train count is bounded. Use `dashmap` for the full registry; use a flat pre-allocated array for the subset of trains in `Active` or `Critical` state where lookup is on the hot polling path.

### Medium-value: adapt with judgement

**6. Spin-based watchdog — `data/HFT-Engine/src/engine.rs`: `run_watchdog`**
The watchdog checks elapsed time every 2^24 iterations to amortise the timer call cost, avoiding OS sleep/wakeup cycles that could preempt critical threads. RailPredict does not have that thread-preemption concern, but the structural pattern — a dedicated watchdog task monitoring connection health, with configurable idle and no-feed timeout thresholds — maps directly to monitoring the Darwin STOMP connection. Adapt into a `tokio::spawn` task rather than a spin loop (tokio's async sleep is fine for RailPredict's ms-level timing requirements).

**7. Buffer pre-touch with `write_volatile` — `data/HFT-Engine/src/main.rs`**
On macOS (and Linux with overcommit), `std::mem::zeroed()` on a heap allocation does not commit physical pages — they are zero-fill-on-demand. The first write to each page causes a demand-paging fault. In HFT this is catastrophic (~3–5µs). In RailPredict it matters less, but pre-touching the `TrainStatus` registry at startup (before any polling threads run) gives consistent first-write latency. Do this in `main.rs` before spawning any tokio tasks.

**8. `#[cold]` on rare/error paths — `data/HFT-Engine/src/engine.rs`: `halt_trading`**
`#[cold]` on a function biases the branch predictor in the caller toward the not-taken (non-error) direction after the first few calls. Apply this to the circuit breaker's `enter_cache_only_mode()` function and to any error handler called from a polling hot path.

**9. Thread priority — `data/HFT-Engine/src/engine.rs`: `set_qos_interactive`**
Uses `pthread_set_qos_class_self_np(0x21, 0)` on macOS to set `QOS_USER_INTERACTIVE`, biasing the thread toward P-cores. On Linux uses `SCHED_FIFO` via raw syscall. Apply to the Darwin STOMP ingestion thread and the state machine poll manager thread to reduce OS-scheduling jitter on those two latency-sensitive paths.

### What NOT to port

- Inline assembly (`asm!`, NEON, AVX2, PRFM) — RailPredict has no sub-microsecond latency requirement.
- `mach_absolute_time()` timing discussion — use `chrono` or `tokio::time` normally.
- The trading signal logic, `fake-exchange`, `market-simulator` — entirely different domain.
- `collect_memory_stats` (getrusage + sysctl) — not needed unless you add a diagnostics endpoint later.

---

## Self-Check Notes

When opening a file or module that has not been recently active, ask:

1. **What does this module own?** — single responsibility check.
2. **What does it read from, and what does it write to?** — data flow check.
3. **Does it make any network calls?** — if yes, verify they go through `networking/` not directly.
4. **Are there any `.unwrap()` or `todo!()` calls left in?** — flag before adding new logic.
5. **Is the module's `//!` doc comment still accurate?** — update it if the module has drifted.

When starting a new session on this project, the recommended warm-up order is:
1. Read `TODO.md` → understand current sprint
2. Read this file (`CLAUDE.md`) → reload architecture context
3. Check `git log --oneline -10` → see what changed recently
4. Read any `TODOs/*.md` relevant to today's work
