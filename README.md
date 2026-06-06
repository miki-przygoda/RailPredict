# RailPredict

**v1.17.3 — June 2026**

A UK rail data engine written in Rust. RailPredict subscribes directly to the **Darwin Push Port** — National Rail's STOMP-based firehose of every train movement in the country — and uses that stream to build an intelligent buffer between users and the Great British Railways API. The vast majority of queries are answered from local state, in-memory cache, and statistical prediction; the only call that ever hits GBR directly is the one that genuinely requires it: final ticket purchase.

---

## The Problem

The GBR API is slow, rate-limited, and expensive to call repeatedly. A naive rail app that hits the live API for every search, every page load, and every status check will feel sluggish and burn through its quota the moment traffic spikes.

Most rail apps are dumb mirrors: ask GBR, show the result, repeat. RailPredict is built on the premise that the vast majority of what a user needs during a journey search can be answered locally — without a single outbound request.

---

## Architecture: Three-Tier Data Model

Rail data is separated into tiers based on how frequently it actually needs to change:

| Tier  | Data                                               | Staleness tolerance | Source                    |
|:------|:---------------------------------------------------|:--------------------|:--------------------------|
| **A** | Timetables, station names, base fares              | Days–weeks          | GTFS static feed          |
| **B** | Predicted delay, likely platform, fare range       | Minutes–hours       | Local history + inference |
| **C** | Live position, seat availability, final price lock | Seconds             | GBR Retail API (live)     |

The goal: by the time a user reaches checkout, all Tier A and Tier B data is already loaded. The single Tier C call happens only when they confirm payment.

---

## How It Stays Efficient

### Darwin Push Port (zero-poll reads)
Instead of polling GBR for train positions, RailPredict subscribes to the **Darwin Push Port** — National Rail's STOMP-based firehose of every train movement in the UK. Incoming XML messages are parsed, filtered, and written to an in-memory registry keyed by RID. Read-only queries are served entirely from that cache.

### Urgency State Machine
Not every train deserves the same attention. Each train in the registry is assigned a state that controls how aggressively the system monitors it:

```
Dormant → Monitored → Active → Critical → Terminal
```

| State         | Condition                      | Behaviour                       |
|:--------------|:-------------------------------|:--------------------------------|
| **Dormant**   | Departure > 2 h                | Static data only, no live calls |
| **Monitored** | 30–120 min out                 | Darwin stream, history building |
| **Active**    | 0–30 min out                   | High-frequency updates          |
| **Critical**  | < 5 min or disruption detected | Real-time stream, push alerts   |
| **Terminal**  | Departed                       | Evicted from active monitoring  |

External events — signal failures, weather alerts, route-level disruption flags — can force emergency promotion regardless of departure time.

### Request Coalescing
If 50 users are watching the same train, one outbound call is made and the result is fanned out to all 50. The coalescer deduplicates concurrent in-flight requests by key and wires late arrivals directly onto the pending future.

### Circuit Breaker
If GBR starts returning errors, the system enters **Cache Only** mode and stops sending requests until GBR recovers. Callers see cached data; GBR sees no additional load.

### Statistical Prediction Engine (Tier B)
Per-service delay history is stored in Postgres and loaded into memory at startup. The engine computes:
- **Weighted delay distribution** by weekday and departure hour
- **Confidence decay** — recent observations outweigh older ones
- **On-time probability** — exposed directly in the departure board UI

---

## What's Built

| Component                                                               | Status   |
|:------------------------------------------------------------------------|:---------|
| Core types (`TrainId`, `TrainStatus`, `Stamped<T>`)                     | Complete |
| Urgency state machine + poll manager                                    | Complete |
| Darwin Push Port ingestion (STOMP/TLS, XML parse, filter)               | Complete |
| In-memory train registry (`DashMap`, concurrent)                        | Complete |
| Request coalescer + rate limiter                                        | Complete |
| Circuit breaker (threshold, cool-down, state transitions)               | Complete |
| Prediction engine (Tier B — delay probability, confidence)              | Complete |
| Postgres schema + migrations (sqlx, compile-time checked)               | Complete |
| Delay history flush (`ON CONFLICT DO UPDATE`)                           | Complete |
| GTFS timetable ingest (Tier A — stations, services, calls, fares)       | Complete |
| REST API (`/stations`, `/trains`, `/journeys`, `/health`)               | Complete |
| SSE live update stream (`/trains/:rid/live`)                            | Complete |
| Frontend — departure board, train detail, search autocomplete           | Complete |
| Developer Console — registry probe, event monitor, ingest UI            | Complete |
| Ticket purchase demo (simulated end-to-end booking flow)                | Complete |
| Prometheus metrics (`/metrics`, ingestion counters, latency histograms) | Complete |
| Rate limiting (`tower_governor`, 60 req/s per IP)                       | Complete |
| Weather volatility promotions (Open-Meteo, configurable anchors)        | Complete |
| Push notifications (ntfy.sh, fires on Critical state promotions)        | Complete |
| Full-journey capture (`journeys` + `journey_calls`, cancellation capture)| Complete |
| Operator league + drill-down (`/operators`, real per-TOC coverage)      | Complete |
| Journey reliability surfaces (detail trajectory, station reliability, arrival/recovery KPIs) | Complete |
| Live Board + server-free Replay (`/live`, `static/replay.html`)         | Complete |
| Tier C wiring (live GBR purchase API)                                   | Pending  |

---

## Tech Stack

| Concern                 | Crate                                                    |
|:------------------------|:---------------------------------------------------------|
| Async runtime           | `tokio`                                                  |
| HTTP framework          | `axum` 0.7                                               |
| HTTP server-sent events | `axum` SSE + `async-stream`                              |
| Darwin STOMP client     | Custom `tokio`-based                                     |
| Darwin XML parsing      | `quick-xml`                                              |
| HTTP client (GBR REST)  | `reqwest` (rustls-tls)                                   |
| In-memory cache         | `dashmap` (lock-free concurrent)                         |
| Database                | `sqlx` 0.8, Postgres, async, compile-time query checking |
| HTML templating         | `maud` (compile-time checked)                            |
| Frontend interactivity  | `htmx` 2 + SSE extension                                 |
| Serialisation           | `serde` + `serde_json`                                   |
| Time                    | `chrono`                                                 |
| Observability           | `tracing` + `metrics` + `metrics-exporter-prometheus`    |
| Rate limiting           | `tower_governor`                                         |
| TLS                     | `tokio-rustls` + `rustls-native-certs`                   |
| CLI                     | `clap` 4 (derive)                                        |
| Error handling          | `thiserror` (domain) + `anyhow` (application)            |

---

## Running Locally

**Prerequisites:** Rust stable, PostgreSQL, Darwin Push Port credentials (National Rail developer programme).

```bash
# Clone and build
git clone https://github.com/miki-przygoda/RailPredict.git
cd RailPredict
cargo build --release

# Configure environment
cp .env.example .env
# Set DATABASE_URL, DARWIN_HOST, DARWIN_USERNAME, DARWIN_PASSWORD, GBR_API_KEY

# Run migrations and start
cargo run --release

# (Optional) Seed the timetable database from a GTFS feed
cargo run --release -- ingest-static --url https://your-gtfs-feed-url.zip
```

The server starts on `0.0.0.0:3000` by default. Visit:

| URL        | Description                                                     |
|:-----------|:----------------------------------------------------------------|
| `/`        | Dashboard — live system health, navigation                      |
| `/search`  | Departure board — station autocomplete, live trains             |
| `/dev`     | Diagnostics — registry probe, event monitor, ingest UI          |
| `/metrics` | Prometheus metrics endpoint                                     |
| `/health`  | DB health probe                                                 |

---

## Project Structure

```
src/
├── api/            REST handlers, SSE endpoint, response types
├── cache/          TrainRegistry (DashMap-backed, concurrent)
├── db/             sqlx queries — history flush, static data, timetables
├── frontend/       maud page handlers (dashboard, search, detail, dev)
├── ingestion/      Darwin STOMP client, XML parser, GTFS ingest
├── networking/     Coalescer, circuit breaker, rate limiter, GBR client
├── prediction/     Tier B engine — delay probability, confidence scoring
├── state_machine/  Urgency states, poll manager, state transitions
├── types/          TrainId, TrainStatus, Stamped<T>, shared domain types
└── weather/        Weather anchor polling (ntfy integration, optional)

migrations/         sqlx Postgres migrations (versioned, checksum-locked)
static/             style.css (single-file design system, ~1 400 lines)
scripts/            Python ML training, data export, and DB seeding utilities
```

---

## Docs

| File | Contents |
|:-----|:---------|
| [`docs/improvements.md`](docs/improvements.md) | Full index of architectural decisions made across all epics. Treat as constraints before touching any module. |
| [`docs/model-performance.md`](docs/model-performance.md) | ML model accuracy breakdown — MAE, tier distribution, feature importance. |
| [`docs/model-improvement-plan.md`](docs/model-improvement-plan.md) | Rationale behind the v1.12.0 LightGBM improvements (bias correction, feature fixes, hyperparameter scaling). |
| [`CLAUDE.md`](CLAUDE.md) | Architecture reference — full module map, key patterns, and engineering conventions. |
| [`SECURITY.md`](SECURITY.md) | Secrets inventory and rotation procedure. |

---

## What Isn't Here Yet

**Live ticket purchase (Tier C).** The GBR Retail API purchase endpoint (`POST /bookings`) is not wired; the product surfaces a "Ticketing — coming soon" stub in its place. The blocker is API tier access: the GBR sandbox grants read access freely, but write (purchase) access requires a separate commercial agreement. The circuit breaker, idempotency layer, and `purchase_attempts` table are already built and ready to connect.

---

## Design Principles

**Avoid the live API.** Every architectural decision is evaluated against whether it reduces the number of outbound GBR calls. The API is the bottleneck; everything else is designed around it.

**Compile-time correctness.** SQL queries are checked against the live schema at compile time via `sqlx`. HTML templates are type-checked by `maud`. There is no stringly-typed layer between the application and its data.

**No JavaScript frameworks.** The frontend is `maud` (server-side HTML) + `htmx` for partial updates + a small amount of vanilla JS for the autocomplete event delegation and SSE event feed. No build step, no bundler, no hydration.

**Predictability over cleverness.** The state machine, circuit breaker, and coalescer all have explicit, observable state. Every transition is logged. The system is designed to be debuggable at runtime through `/dev`, `/metrics`, and the live event monitor.

**Dependency hygiene.** `cargo deny` enforces licence compatibility and blocks known-vulnerable crate versions on every build. Secrets are documented with rotation cadence in `SECURITY.md`; none are committed or logged.

---

## License

**Proprietary — © 2026 Mikolaj Mikuliszyn. All rights reserved.**

RailPredict is closed-source commercial software. The source code, models, and
datasets are confidential and may not be used, copied, modified, or distributed
without prior written permission. See [`LICENSE`](LICENSE) for the full terms.

Underlying UK rail data is sourced from the National Rail Darwin Push Port feed
under Network Rail's data-feed terms and remains subject to its own licence.
