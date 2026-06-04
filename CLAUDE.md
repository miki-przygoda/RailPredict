# Claude

The current version and last worked on date should be noted at the top of this file below this line:

**version = "1.12.9" -- 04/06/2026**

---

## Project Snapshot

RailPredict is a Rust-based high-performance shadow system for UK Rail data. Its core purpose is to act as an **Intelligent Buffer** between users and the GBR (Great British Railways) API — minimising expensive live calls by combining cached static data, local predictive logic, and state-machine-driven polling. The system should feel instant to the user: data is pre-warmed, delays are predicted locally, and live API calls only happen when transactionally unavoidable.

**Current state (v1.12.0):** The full Tier A/B pipeline is live — Darwin stream ingestion, GTFS timetable loading, LightGBM ONNX inference (day-ahead and real-time models), prediction outcome tracking, and the complete departure board and detail UI. Tier C (live GBR purchase API) is the only remaining unconnected piece; the plumbing is already built.

---

## Working Principles

- **Before modifying a file**, read its immediate neighbours in the module tree to understand the data flow context.
- **Check `docs/improvements.md`** before starting work in any domain — it captures all architectural decisions made across the project's epics and should be treated as constraints, not suggestions.
- **Never make a live GBR API call inside a hot path.** All external calls go through the networking layer with rate limiting and circuit-breaker logic.
- **Concurrency model**: prefer message passing (`mpsc`, `oneshot`) over shared mutable state (`Arc<Mutex<T>>`). Use `Arc<RwLock<T>>` only for read-heavy shared state like the train registry.
- **Error handling**: use `thiserror` for domain errors, `anyhow` for application-level error propagation. Never `.unwrap()` in production paths.
- **No JavaScript frameworks** in the frontend — `maud` + `htmx` + minimal vanilla JS only. No build step.

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
├── CLAUDE.md                       ← this file; AI session seed + architecture reference
├── CONTRIBUTING.md                 ← contributor guide; read before opening a PR
├── README.md                       ← project vision + current status table
├── TODO.md                         ← active sprint tracker + remaining epics
├── CHANGELOG.md                    ← completed epics log; updated on minor version bumps
├── SECURITY.md                     ← secrets rotation procedure + security posture
├── deny.toml                       ← cargo-deny advisory/license config
├── migrations/                     ← sqlx SQL migrations (run at startup via sqlx::migrate!)
├── .github/workflows/ci.yml        ← GitHub Actions CI (deny → clippy → test → release build)
├── deploy/
│   ├── prometheus.yml              ← Prometheus scrape config (docker compose --profile monitoring)
│   └── docker-compose.prod.yml    ← production overrides (no exposed DB port, stricter limits)
├── docs/
│   ├── improvements.md             ← full item index + architectural decisions (all epics done at v1.7.0)
│   ├── model-performance.md        ← ML model accuracy breakdown and evaluation data
│   ├── model-improvement-plan.md   ← ML v3 improvement rationale (complete, v1.12.0)
│   └── index.html                  ← generated static snapshot (make export); not hand-edited
├── scripts/
│   ├── compare_models.py           ← LightGBM training + ONNX export; run to retrain models
│   ├── export_dataset.py           ← export delay_history to Parquet + HuggingFace README
│   ├── fetch_hsp_history.py        ← bulk HSP historical delay fetch (route-based O-D pairs)
│   ├── run_hsp_fetch.sh            ← launcher for 4 parallel HSP shards
│   ├── seed_history.py             ← synthetic delay backfill (use before live data exists)
│   ├── seed_stations.py            ← populate stations table from OpenStreetMap Overpass
│   ├── train_models.py             ← standalone training script (older; prefer compare_models.py)
│   └── requirements.txt            ← Python deps for all scripts
└── RailPredict/                    ← Rust crate root
    ├── Cargo.toml                  ← crate manifest; version must match project version
    ├── Cargo.lock                  ← committed; this is a binary application not a library
    ├── .env.example                ← template; copy to .env and fill in credentials
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
        │   └── train_state.rs      ← TrainState urgency vocabulary + StateChangeEvent; states set inline by ingestion
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
        │   ├── train_registry.rs   ← DashMap registry; tiploc_index; eviction; cascade helpers
        │   └── station_index.rs    ← in-memory word-prefix index for station autocomplete (no DB hit)
        ├── prediction/
        │   ├── mod.rs
        │   ├── engine.rs           ← PredictionEngine; trimmed-mean; ONNX inference; correlation; confidence decay
        │   ├── onnx_engine.rs      ← OnnxEngine; day-ahead + real-time LightGBM models via ort
        │   └── types.rs            ← ServicePattern, HistoricalStore, DelayRecord; RollingStats, LiveFeatures
        ├── db/
        │   ├── mod.rs              ← connect(); type Db = PgPool; runs migrations at startup
        │   ├── static_data.rs      ← get_station, departures_from, cheapest_fare
        │   ├── history.rs          ← load_history (window fn); flush_history (500-row chunks, idempotent)
        │   └── predictions.rs      ← per-RID prediction_outcomes ledger; insert/finalise/recent/by-rid
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
            ├── components.rs       ← delay_badge, platform_chip, prediction_chip
            ├── dashboard.rs        ← /  — hero + metrics grid + nav cards (with ML accuracy)
            ├── demo.rs             ← /demo — Feature Lab, Purchase Demo, Predictions ledger
            ├── search.rs           ← departure board + journey search; uses station_index
            ├── detail.rs           ← train detail page + prediction card; htmx SSE live section
            └── predictions.rs      ← /predictions — public ML accuracy analytics page
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

> **Current status (deferred):** This transition table and the `PollManager` describe the *intended* design. In production the rule engine (`from_departure`/`emergency_promote`) and the `PollManager` registration channel are **bypassed** — ingestion sets `TrainState` inline (`ingestion/mod.rs`) and the spawned `PollManager` loops on an empty heap. Slated for a cut; see `docs/tech-debt.md` §A1.

| State       | Trigger Condition                          | Polling Behaviour               |
|:------------|:-------------------------------------------|:--------------------------------|
| `Dormant`   | Departure > 2 hours away                   | No live calls. Tier A only.     |
| `Monitored` | 120 > departure > 30 mins                  | Poll every 10 mins.             |
| `Active`    | 30 > departure > 0 mins                    | Poll every 30–60s.              |
| `Critical`  | Departure < 5 mins OR volatility triggered | Push-port stream or 10s polling |

**Volatility promotions** (bypass normal time-based transitions):
- Wind > 50mph on a route → force all trains on that route to `Active`
- Major incident detected → force affected corridor to `Critical`

---

## Key Patterns (Per Domain)

### Types (`src/types/`)
- `TrainId` is an enum wrapping `Rid`, `Uid`, and `Headcode` — all lookups go through it; never pass raw strings for train identifiers across module boundaries
- `TrainStatus` is the single source of truth; `Stamped<T>` gives every field its own `last_updated` timestamp for stale-data detection
- Use `chrono` for all timestamps; always distinguish `scheduled_departure`, `public_departure`, `actual_estimated_departure`

### State Machine (`src/state_machine/`)
> **Current status (deferred):** the rule engine + `PollManager` registration are production-dead (ingestion sets states inline); slated for a cut — see `docs/tech-debt.md` §A1.
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
- `check_tiploc_cascade` in `filter.rs` detects knock-on delays via the TIPLOC index in `train_registry.rs` — wire into `ingestion/mod.rs` when Tier C is active. **Current status:** staged/inert (no production caller); see `docs/tech-debt.md` §C2.
- **NP-association path is currently inert:** the filter taxonomy needle is lowercase `b"association"` but real Darwin frames are `<Association>`, so association frames are dropped before the parser and the predecessor-delay signal never runs. Pending a case-fix + wire — see `docs/tech-debt.md` §A2.

### Prediction Engine (`src/prediction/`)
- `ServicePattern` is keyed on `(uid, weekday, origin_crs, departure_hour)` — stable recurring-service identity, not the daily-changing RID
- `HistoricalStore` is hard-capped at `MAX_SAMPLES=90` per pattern; trimmed-mean drops top/bottom 10%; no prediction emitted with fewer than 3 samples
- `predict_and_update_with_correlation` blends in preceding-service delay (0.6/0.4 weight) when a service shares `origin_crs` in a ±20-min window
- Confidence decay: `confidence * exp(-days_since / 21.0)` applied when last `DelayRecord` is older than 21 days
- ONNX models live in `models/` — retrain with `scripts/compare_models.py`, which exports `day_ahead.onnx` and `realtime.onnx` directly

### Database (`src/db/`)
- `load_history` uses a window function to reconstruct `HistoricalStore` from the most-recent `MAX_SAMPLES` rows per pattern — avoids full table scan
- `flush_history` inserts in 500-row chunks with `ON CONFLICT DO NOTHING` — idempotent; safe to call repeatedly
- `departures_from` is the hot path for the departure board; indexed on `(location_crs, operating_date, scheduled_departure)`

### ML Pipeline (`scripts/`)
- `compare_models.py` trains day-ahead and real-time LightGBM models, applies equal-tier sample weighting to correct the severe-delay bias in the Darwin feed, and exports to ONNX
- Feature count is fixed: 14 for day-ahead, 22 for real-time (= day-ahead + 8 live signals) — changing this requires matching updates to `onnx_engine.rs` (`N_DAY_FEATURES`, `N_RT_FEATURES`)
- `export_dataset.py` regenerates `dataset/delay_history.parquet` and the HuggingFace README from the live DB; run after each retrain

---

## Concurrency & Performance Patterns

These are the key patterns worth preserving as the codebase grows:

**Single global scheduler, not per-train tasks.** `PollManager` uses a `BinaryHeap<(Instant, TrainId)>` so the number of tokio tasks stays O(1) regardless of how many trains are tracked. Never spawn a dedicated `tokio::spawn` per train for polling.

**Fan-out via oneshot channels.** The coalescer in `networking/coalescer.rs` deduplicates concurrent in-flight requests: the first caller drives the HTTP request, late arrivals attach a `oneshot::Receiver`. This prevents N identical outbound calls when N users load the same train page simultaneously.

**STOMP reconnect without state loss.** `PipelineContext` holds all shared state behind `Arc`. On a STOMP disconnect, only the TCP client is replaced — the registry, prediction engine, and filter retain their state across reconnects.

**Sequence guard for late arrivals.** `SequenceGuard` in `filter.rs` tracks the last-seen sequence per train and drops messages that arrive out of order. This handles the Darwin reconnect-replay problem where old messages flood in after a reconnect.

**`#[cold]` on error paths.** Applied to `circuit_breaker::enter_cache_only_mode` and similar rare paths — biases the branch predictor toward the happy path.

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
4. Read `docs/improvements.md` if working in a domain with prior architectural decisions
