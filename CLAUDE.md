# Claude

The current version and last worked on date should be noted at the top of this file below this line:

**version = "0.1.0" -- 17/04/2026**

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
├── README.md                       ← project vision at idea level; human-readable overview
├── TODO.md                         ← current sprint todos; versioning instructions
├── CHANGELOG.md                    ← completed epics log; updated on minor version bumps
├── TODOs/
│   ├── Structs.md                  ← data architecture: TrainStatus, TrainID, VolatilityContext
│   ├── StateMachine.md             ← polling logic: TrainState enum, BinaryHeap manager
│   ├── Networking.md               ← GBR API layer: coalescing, rate limiting, circuit breaker
│   └── DataIngestion.md            ← Darwin STOMP firehose: filter, sequencing, XML parsing
└── RailPredict/                    ← Rust crate root
    ├── Cargo.toml                  ← crate manifest; version must match project version
    ├── Cargo.lock                  ← committed; this is a binary application not a library
    └── src/
        └── main.rs                 ← entry point; currently a stub (Hello, world!)
```

As `src/` grows, this map should be updated to reflect new modules. Expected near-term additions:

```
src/
├── main.rs                         ← tokio runtime init, top-level wiring
├── types/
│   ├── mod.rs
│   ├── train_id.rs                 ← TrainID enum (RID / UID / Headcode)
│   ├── train_status.rs             ← TrainStatus struct (single source of truth)
│   └── volatility.rs               ← VolatilityContext struct
├── state_machine/
│   ├── mod.rs
│   ├── train_state.rs              ← TrainState enum + transition logic
│   └── poll_manager.rs             ← global BinaryHeap-based polling loop
├── networking/
│   ├── mod.rs
│   ├── gbr_client.rs               ← reqwest wrapper for GBR REST API
│   ├── coalescer.rs                ← request collapser (oneshot fan-out)
│   ├── rate_limiter.rs             ← token bucket or leaky bucket impl
│   └── circuit_breaker.rs          ← 503 detection + cool-down mode
├── ingestion/
│   ├── mod.rs
│   ├── stomp_client.rs             ← Darwin STOMP firehose connection
│   ├── filter.rs                   ← region/route filter; drop irrelevant messages early
│   └── parser.rs                   ← serde-xml-rs Darwin XML → internal types
└── cache/
    ├── mod.rs
    └── train_registry.rs           ← moka or dashmap-backed in-memory store
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

### Structs (`TODOs/Structs.md`)
- `TrainID` is an enum wrapping `RID`, `UID`, and `Headcode` — all lookups go through it
- `TrainStatus` is the single source of truth; it must accept updates from REST polling, STOMP firehose, and the prediction engine
- Use `chrono` for all timestamps; always distinguish `ScheduledDeparture`, `PublicDeparture`, `ActualEstimatedDeparture`
- Every field on `TrainStatus` should carry a `LastUpdated` timestamp for stale-data detection

### State Machine (`TODOs/StateMachine.md`)
- `enum TrainState { Dormant, Monitored, Active, Critical }`
- Do **not** give each train its own `tokio::spawn` polling task — use a single global manager with a `BinaryHeap` ordered by next-poll time
- State changes are broadcast via `mpsc` to a notification service; the API/UI layer subscribes — never locks the registry to check for changes

### Networking (`TODOs/Networking.md`)
- Request coalescing: if a request for a given `TrainID` is already in-flight, register a `tokio::sync::oneshot` sender and wait; the first responder fans the result out to all waiters
- Rate limiter sits in front of all outbound GBR calls; it queues requests if the threshold is exceeded
- Circuit breaker: on a 503 from GBR, enter "Cache Only" mode and stop sending requests for a configurable cool-down window

### Data Ingestion (`TODOs/DataIngestion.md`)
- Apply region/route filter **as the first step** in the ingestion pipeline — drop irrelevant messages before any parsing
- Always check `sequence_id` or `timestamp` before writing to `TrainStatus`; never overwrite a newer update with a late-arriving older one
- Use `serde-xml-rs` (or equivalent) for Darwin XML; parsing speed is critical to the "instant" feel

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
