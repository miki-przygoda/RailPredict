# Improvements, Next Steps & Must-Haves

_A full audit of the codebase as of v1.1.0. Organized by priority, not by module._

---

## Status — Breakdown into Agent Epics (v1.4.0)

This file has been broken down into focused agent-sized epic files. Each item below is
now tracked in its respective epic file. Do not start work directly from this file —
use the epic files instead.

| Epic file           | Items covered                     | Status                                 |
|---------------------|-----------------------------------|----------------------------------------|
| ProductionHardening | 1.1, 1.2, 1.4, 1.5, 3.1, 3.2, 3.3 | **COMPLETE — v1.2.0** (deleted)        |
| CI_DevEx            | 1.3, 4.1, 4.2, 4.3, 4.4           | **COMPLETE — v1.3.0** (deleted)        |
| TierADataLayer      | 2.3r, 2.4, 2.5, 6.1, 6.2          | **COMPLETE — v1.5.0** (deleted)        |
| TierCWiring         | 2.1, 2.2, 2.6, 8.1, 4.5           | **COMPLETE — v1.6.0** (deleted)        |
| Docker + Darwin     | Dockerfile, DNS, STOMP, gzip      | **COMPLETE — v1.4.1–v1.4.3**           |
| AgentA              | 5.3, 8.2                          | **COMPLETE — v1.7.0** (deleted)        |
| AgentB              | 5.1                               | **COMPLETE — v1.7.0** (deleted)        |
| AgentC              | 5.4, 5.5                          | **COMPLETE — v1.7.0** (deleted)        |
| _(pending)_         | 8.3, 5.6, 6.4, 2.8                | **NOT STARTED** — new items added post-v1.7.0 |

### Already completed (do not re-implement)
- **1.1** (STOMP TLS) — completed in ProductionHardening v1.2.0.
- **1.2** (STOMP auto-reconnect) — completed in ProductionHardening v1.2.0.
- **1.3** (`.sqlx/` snapshot + CI sqlx-check) — CI step wired in v1.3.0; snapshot generation is a pending user action.
- **1.4** (CORS tightening) — completed in ProductionHardening v1.2.0.
- **1.5** (HTTP rate limiting) — completed in ProductionHardening v1.2.0.
- **2.3** (DB in AppState) — completed as Observability prereq in v1.1.2. Remaining
  work (merge DB + registry in handlers) is in `TODOs/TierADataLayer.md`.
- **2.7** — same as 2.3, duplicate entry.
- **3.1** (CRS validation) — completed in ProductionHardening v1.2.0.
- **3.2** (health endpoint DB probe) — completed in ProductionHardening v1.2.0.
- **3.3** (SECURITY.md) — completed in ProductionHardening v1.2.0.
- **4.1** (CI pipeline) — completed in CI_DevEx v1.3.0.
- **4.2** (cargo-deny) — completed in CI_DevEx v1.3.0.
- **4.3** (DB integration tests) — completed in CI_DevEx v1.3.0.
- **4.4** (README tech stack) — completed in CI_DevEx v1.3.0.
- **5.2** (destination on departure cards) — completed in FrontEndHardening v1.1.1.
- **6.3** (HTMX SSE error banner) — completed in FrontEndHardening v1.1.1.
- **7.1** (departure board sequential lock acquisitions) — completed in TechnicalDebt v1.4.0.
- **7.2** (Prometheus /metrics endpoint) — completed in Observability v1.1.2.
- **7.3** (DB pool hardcoded at 10) — completed in TechnicalDebt v1.4.0.
- **9.1** (`is_cancelled` → `Stamped<Option<bool>>`) — completed in TechnicalDebt v1.4.0.
- **9.2** (`best_delay_mins`/`best_platform` undocumented) — completed in TechnicalDebt v1.4.0.
- **9.3** (STOMP byte-by-byte read) — completed in TechnicalDebt v1.4.0.
- **9.4** (departure sort by string) — completed in TechnicalDebt v1.4.0.
- **9.5** (CIF `unimplemented!()` panic) — completed in TechnicalDebt v1.4.0.

### Suggested epic execution order (post-v1.7.0)
1. **8.3** — Cross-source write ordering (correctness fix; no new features required, small scope)
2. **6.4** — Journey handler query optimization (unblock 5.1 scaling; can be done in isolation)
3. **5.6** — Planned platform in search results (product polish; requires 2.5 complete ✓)
4. **2.8** — GBR Purchase API / Tier C checkout (the transactional "final boss"; do last, most risk)

---

## 1. Production Blockers — Fix Before Any Live Deployment

These will cause silent failures or security holes the moment the app is exposed publicly.

### 1.1 STOMP uses plain TCP — Darwin requires TLS ✓ COMPLETED v1.2.0
`LiveStompClient::subscribe` calls `TcpStream::connect`. The real Darwin Push Port broker
requires a TLS connection (port 61613 with STARTTLS or port 61614 direct TLS).
Connecting over plain TCP will be silently refused.
**Fix:** wrap the TCP stream in `tokio-rustls` using a `TlsConnector` before the STOMP
CONNECT frame is sent. Add `DARWIN_TLS=true` env var (default on) with an escape hatch
for local mock brokers.

### 1.2 No STOMP auto-reconnect ✓ COMPLETED v1.2.0
`IngestionPipeline::run` exits as soon as the STOMP stream closes. Darwin disconnects
clients roughly every 30 minutes (ActiveMQ session timeout). After that, the pipeline
stops permanently and the in-memory registry freezes. Nothing in `main.rs` restarts it.
**Fix:** wrap the `pipeline.run()` call in a retry loop with exponential backoff (start 2s,
cap 120s). The `SequenceGuard` already handles duplicate/replayed messages on reconnect.

### 1.3 `.sqlx/` offline snapshot not committed — Docker build fails ⚠ CI WIRED v1.3.0 — USER ACTION STILL REQUIRED
`Dockerfile` sets `SQLX_OFFLINE=true` in the builder stage, but no `.sqlx/` directory
exists in the repo. `cargo build --release` will fail at the macro expansion stage because
sqlx cannot verify queries at compile time without either a live DB or the snapshot.
**Fix:** run `cargo sqlx prepare` locally against a dev DB, commit the generated
`.sqlx/` directory, add a CI step that runs `cargo sqlx prepare --check` to catch drift.

### 1.4 CORS is permanently permissive ✓ COMPLETED v1.2.0
`CorsLayer::permissive()` allows any origin, any method, any header. The comment says
"Tighten for production" but there is no production config for it.
**Fix:** add `CORS_ALLOWED_ORIGINS` env var (comma-separated). In `router()`, use
`CorsLayer::new().allow_origin(...)` with the parsed origins. Default to `permissive()`
only if the var is unset and `LOG_LEVEL=debug` (i.e. dev mode).

### 1.5 No HTTP API rate limiting ✓ COMPLETED v1.2.0
The public HTTP endpoints have no request throttling. A single client can exhaust the
server by hammering `/stations/{crs}/departures` (which acquires a read lock on every
registry entry).
**Fix:** add `tower_governor` or a simple `tower::ServiceBuilder` middleware that limits
requests per IP. 60 req/s per IP is a reasonable ceiling for a public-facing API.

---

## 2. Architecture Completions — The App Is Partially Wired

These are subsystems that exist but are not connected to each other. The app builds
and some tests pass, but several advertised features do nothing at runtime.

### 2.1 PollManager fires but nobody calls GBR

> **Status (v1.21.1): built, not wired.** The poll consumer was added to `main.rs`, but
> `PollManager` was later removed (`be012f7`), so nothing emits the same-state events the
> consumer waits for. Coalescer, rate limiter and circuit breaker have no live traffic, and
> `gbr_client.rs` must not be wired until its contract is reconciled with the real upstream.
`PollManager::fire_poll` emits a `StateChangeEvent` on the broadcast channel, but
**nothing consumes that event to trigger an actual GBR API call**. The entire
networking layer — `Coalescer`, `RateLimiter`, `CircuitBreaker`, `LiveGbrClient` —
is built and tested in isolation but is never instantiated or called from `main.rs`.
Tier C is completely non-functional at runtime.
**Fix:** spawn a dedicated "poll consumer" task in `main.rs` that subscribes to
`state_change_tx`, filters for `old_state == new_state` (i.e. "poll fired" events
rather than real promotions), and calls `Coalescer::get(train_id)` for each, then
writes the result back into the registry. Thread the `CircuitBreaker` and
`RateLimiter` into that path.

### 2.2 GBR JSON response is never parsed
`LiveGbrClient::get_train_status` returns `Err(GbrClientError::NotFound)` for all 200
responses, with a TODO comment. No `serde` structs exist for the GBR REST API response
schema.
**Fix:** define `GbrTrainStatusResponse` and `GbrDeparture` structs in `gbr_client.rs`
matching the RTT API v1 schema, deserialize the 200 body, and map into `TrainStatus`.
This unblocks the entire Tier C path.

### 2.3 Departure board reads from live registry, ignores Tier A DB data

> ⚠️ COMPLETED AS PREREQ — implemented as prerequisite for Observability epic (cache hit ratio metric).
> Added `pub db: Db` to AppState and wired in main.rs. Full tier-routing logic (merging DB + registry
> in departures_handler) remains to be done as part of the Improvements epic.

`GET /stations/{crs}/departures` and `/ui/stations/departures` scan `TrainRegistry`.
If Darwin is not connected (all dev/CI environments), they return empty.
The `timetable_calls` table and `departures_from()` query exist but are never called
from any handler. `AppState` does not carry a DB pool, making it unreachable from handlers.
**Fix (remaining):** In `departures_handler`, merge DB rows (`departures_from`) with
registry live data: start from the DB timetable for the day, then overlay any live
`TrainStatus` fields (estimated departure, platform, delay) where a matching RID exists
in the registry. The `pub db: Db` field is now in AppState — handlers can reach the DB.

### 2.4 Station name search not possible — only CRS codes work
The search page `<input>` says "Station code (e.g. KGX)". Normal users type names.
The `stations` table has a GIN full-text index on `name`, but it is never queried.
There is no autocomplete endpoint.
**Fix:** add `GET /stations/search?q=<text>` JSON endpoint backed by
`to_tsvector('english', name) @@ plainto_tsquery(...)`. Add an `hx-get` autocomplete
input on the search page that fires after 2+ characters and swaps a dropdown of
`{crs, name}` pairs.

### 2.5 GTFS ingest only populates `stations` — `timetable_calls` and `services` are empty
`gtfs.rs` parses `stops.txt` only. `trips.txt` + `stop_times.txt` → `timetable_calls`
was marked "pending" in Phase 5 but is unimplemented. `services` is also never written.
The `timetable_calls` table exists but will always be empty until this is added.
**Fix:** extend `run_ingest` to also parse `trips.txt` (uid + days bitmask) and
`stop_times.txt` (call order, times per trip per stop), upsert into `services` then
`timetable_calls`. Network Rail GTFS names differ slightly from NR-open-data CIF — test
both formats.

### 2.6 State machine transitions never happen after registration
`TrainState::from_departure` computes the correct state from departure time, but it is
only called at registration. Once a train is in the registry, its state never changes
unless a Darwin cancellation/delay message forces a Critical promotion.
Trains never naturally progress from `Dormant → Monitored → Active`.
**Fix:** in the "poll consumer" task (2.1), after updating the registry entry, call
`TrainState::from_departure` with the current departure time and the current `Utc::now()`,
compare with the stored state, and emit a real state-change event + re-queue at the
new interval if the state has changed.

### 2.8 GBR Purchase API — Tier C checkout flow is unimplemented

The GBR REST client (`src/networking/gbr_client.rs`) currently only supports read operations
(train status polling). The advertised Tier C functionality includes ticket purchase, but
`LiveGbrClient` has no method for sending purchase commands. This is the only part of the
system that is transactionally irreversible: once a purchase command is sent, it cannot be
retried blindly (idempotency is not guaranteed by the GBR Retail API).

**Why this is different from the rest of Tier C:**
All existing Tier C calls (status polling) are safe to retry — a duplicate poll is harmless.
A duplicate purchase command could charge the user twice. The circuit breaker and rate limiter
must therefore be configured *more aggressively* here than for polling: lower failure threshold,
shorter cool-down window, and a mandatory idempotency key on every request.

**Architecture:**
1. **Add `GbrPurchaseRequest` / `GbrPurchaseResponse` serde structs** to `gbr_client.rs`.
   Model the GBR Retail API v1 `/bookings` endpoint. Key fields: `journey_uid`, `passenger_count`,
   `fare_id` (from `cheapest_fare` in `db/static_data.rs`), `idempotency_key` (UUID generated
   client-side, stored before the call, never reused). Map the 201 response to a
   `BookingConfirmation { booking_ref: String, total_price_pence: u32 }`.

2. **Introduce a dedicated `PurchaseCircuitBreaker`** separate from the polling circuit breaker
   in `src/networking/circuit_breaker.rs`. Config: `PURCHASE_FAILURE_THRESHOLD=1` (fail on first
   error), `PURCHASE_COOL_DOWN_SECS=60`. Rationale: a slow/erroring purchase endpoint should
   immediately stop all purchase attempts so the user gets a fast "try again later" rather than
   hanging. Never let a purchase call block the polling path — they must run on separate circuits.

3. **Add `POST /journeys/{uid}/purchase` HTTP handler** in `src/api/handlers.rs`. Handler flow:
   a. Validate `uid` format (same pattern as `validate_crs` but for service UIDs).
   b. Look up the fare via `db::static_data::cheapest_fare`.
   c. Generate a UUID idempotency key and persist it to a new `purchase_attempts` table
      (migration required) *before* calling GBR — so a server crash mid-call doesn't lose the key.
   d. Call `LiveGbrClient::purchase(request)` through the `PurchaseCircuitBreaker`.
   e. On 201: write the `BookingConfirmation` to `purchase_attempts`, return confirmation JSON.
   f. On any error: update `purchase_attempts` row to `failed`, return a structured `ApiError`
      with a user-facing message. Never expose raw GBR error bodies to the client.

4. **Add migration** `migrations/YYYYMMDDHHMMSS_create_purchase_attempts.sql`:
   ```sql
   CREATE TABLE purchase_attempts (
       id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
       idempotency_key UUID NOT NULL UNIQUE,
       journey_uid   TEXT NOT NULL,
       fare_id       TEXT NOT NULL,
       status        TEXT NOT NULL DEFAULT 'pending',  -- pending | confirmed | failed
       booking_ref   TEXT,
       created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
       updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
   );
   CREATE INDEX pa_journey_idx ON purchase_attempts(journey_uid);
   ```

5. **Test strategy:** `tests/db_integration.rs` should cover the idempotency key uniqueness
   constraint (duplicate key → `ON CONFLICT DO NOTHING` or explicit error). Integration tests
   for the handler should use a mock `GbrClient` that returns a 503 on first call and a 201 on
   second, asserting that the circuit breaker fires on the first failure (threshold=1) and
   that the `purchase_attempts` row is marked `failed` — not retried automatically.

**Risk note:** Do not implement retry logic for failed purchases. Retrying is the user's
explicit action (re-submitting the form). Automatic retries on purchase commands are a
double-charge footgun. The circuit breaker's cool-down is the only "retry gate" here.

### 2.7 `AppState` missing DB — static data queries are unreachable from handlers
`departures_from`, `get_station`, `cheapest_fare` all exist in `src/db/static_data.rs`
but `AppState` only carries `registry` and `state_change_tx`. There is no path from
any HTTP handler to the database.
**Fix:** add `pub db: Db` to `AppState`, wire it in `main.rs` where `AppState` is
constructed.

---

## 3. Security Hardening

### 3.1 Validate CRS code format in all handlers ✓ COMPLETED v1.2.0
`/stations/{crs}/departures` accepts any string and uppercases it. A 500-character CRS
would pass through to the DB query. Add a CRS format check (exactly 3 ASCII letters)
at the handler boundary before any DB or registry access.

### 3.2 Health endpoint should probe DB connectivity ✓ COMPLETED v1.2.0
`GET /health` always returns 200. An operator restarting a crashed DB would see a
"healthy" service silently serving stale data.
**Fix:** include a lightweight `SELECT 1` ping in the health handler (with a 1s timeout)
and return 503 if it fails.

### 3.3 Secrets in environment — document rotation procedure ✓ COMPLETED v1.2.0
`.env.example` lists `GBR_API_KEY`, `DARWIN_PASSWORD`, `DB_PASSWORD`. There is no
documented procedure for rotating these in production (re-deploy with new env, vs
live reload). Add a `SECURITY.md` or a section in README covering this.

---

## 4. Developer Experience & CI

### 4.1 No CI pipeline ✓ COMPLETED v1.3.0
There is no `.github/workflows/` directory. Nothing automatically runs `cargo test`,
`cargo clippy`, or `cargo sqlx prepare --check` on push.
**Must add:** a GitHub Actions workflow that runs on every push to main and on PRs:
- `cargo clippy -- -D warnings`
- `cargo test`
- `cargo sqlx prepare --check` (needs a Postgres service container)
- `cargo build --release` (catches proc-macro failures that unit tests miss)

### 4.2 No `cargo-deny` or dependency audit ✓ COMPLETED v1.3.0
No `deny.toml` and no audit step. The dependency tree includes network-facing crates
(`reqwest`, `rustls`, `sqlx`). A supply chain advisory on any of them won't be caught.
**Fix:** add `cargo deny check` to CI. Commit a permissive `deny.toml` to start,
tighten over time.

### 4.3 No integration test against a real database ✓ COMPLETED v1.3.0
All tests run against in-memory state or mocks. `src/db/` functions are tested only
at compile time. A `docker-compose.yml` service for tests exists (`db` service) but
no test harness uses it.
**Fix:** add a `tests/db_integration.rs` that uses `sqlx::test` (the sqlx macro that
spins up a schema-migrated test database per test function) to cover `load_history`,
`flush_history`, `get_station`, `departures_from`, `cheapest_fare`.

### 4.4 README tech stack table is out of date ✓ COMPLETED v1.3.0
README lists `moka` (not in Cargo.toml) and `polars` (not in Cargo.toml) as
dependencies. The actual cache is `dashmap`. This will confuse anyone reading the
project for the first time.
**Fix:** update the table to reflect the actual Cargo.toml dependencies.

### 4.5 `#[allow(dead_code)]` is masking real wiring gaps
Eleven uses of `#[allow(dead_code)]` across `networking/`, `ingestion/stomp_client.rs`,
and `api/mod.rs`. These suppress warnings that would otherwise flag un-called code.
Most of these are legitimate (the networking layer built in isolation, issue 2.1).
Once the wiring is done (items 2.1–2.3), remove the `allow` attributes and let the
compiler catch any remaining stragglers.

---

## 5. Product Features (Medium-Term)

### 5.1 Journey search (A→B, not just departures from A)
The README envisions searching for trains between two stations. The current UI only
shows all departures from a single origin. A journey search would filter
`timetable_calls` for services that call both origin and destination in order, sorted
by departure time.

### 5.2 Destination station shown on departure board ✓ COMPLETED v1.1.1
Each card on the departure board shows the RID and platform but not the destination
station. Users need to know where the train is going.
**Fix:** populate `destination_crs` on `TrainStatus` from the final call in the Darwin
`TS` `<Location>` list. Surface it in `DepartureBoardEntry` and in the HTML card.

### 5.3 Fare display on train detail page
`cheapest_fare` exists but is never shown in the UI. The detail page could show the
current cheapest fare for the origin→destination pair if both are known.

### 5.4 Weather-driven volatility promotions
`VolatilityContext` has a `wind_speed_mph` field and the CLAUDE.md mentions
"Wind > 50mph → force Active". There is no weather API integration and no code that
reads `wind_speed_mph` to trigger a promotion.
**Fix:** integrate Met Office DataPoint or Open-Meteo API; periodically fetch wind speed
for the bounding boxes of active routes; write into a shared `VolatilityStore`; have
the poll consumer check it before computing next state.

### 5.6 Planned platform display from GTFS stop_times (Tier A prediction)

After item 2.5 completed the GTFS `trips.txt` + `stop_times.txt` ingest into `timetable_calls`,
the planned platform data is now in the database but is not surfaced in any UI or API response.
The departure board and journey search results show live platform only when Tier C data is
available — meaning platform is blank during `Dormant` and `Monitored` states, which is the
majority of the train's lifecycle from a user's perspective.

**The core insight:** Platform assignments rarely change between the timetable and reality for
well-run services. Showing "Usually Platform 4" from the Tier A timetable is strictly better
than showing nothing, and it's free — the data is already in `timetable_calls.platform`.

**What needs to change:**

1. **Confirm `timetable_calls.platform` is populated.** After the 2.5 ingest, verify that
   `platform` in `stop_times.txt` is mapped to the `timetable_calls.platform` column during
   `run_ingest` in `src/ingestion/gtfs.rs`. Network Rail GTFS uses `stop_times.stop_id`
   format `{crs}_{platform}` (e.g. `KGX_4`) — parse the suffix as the platform label.
   Add a DB integration test in `tests/db_integration.rs` asserting that after a known
   fixture ingest, `timetable_calls.platform` is non-null for stops that have platform data.

2. **Expose planned platform in `departures_from`.** In `src/db/static_data.rs`, the
   `departures_from` query should already return the `platform` column. If it doesn't,
   add it to the `SELECT`. The returned `DepartureBoardEntry` (in `src/api/types.rs`) has
   a `platform` field — if it's `None` after the DB merge and no live Tier C platform is
   available, populate it from `timetable_calls.platform` as the fallback.

3. **Merge logic in `departures_handler`.** The handler in `src/api/handlers.rs` merges
   DB timetable rows with live `TrainStatus` overlay. The merge priority for platform must be:
   - **First:** `TrainStatus.actual_platform` (Tier C confirmed live platform)
   - **Second:** `TrainStatus.scheduled_platform` (Tier C scheduled but not confirmed)
   - **Third:** `timetable_calls.platform` (Tier A planned; label it as "Planned" in UI)
   - **Fourth:** `None` (omit the chip entirely)

4. **UI label.** In `src/frontend/components.rs`, the `platform_chip` component should
   accept an optional `is_planned: bool` flag. When true, render the chip with a different
   style (e.g. grey border instead of solid fill, tooltip "Planned — live platform TBC").
   This distinguishes "Platform 4 (confirmed)" from "Platform 4 (from timetable)" so
   users aren't confused if the train moves.

5. **Journey search results** (`src/frontend/search.rs`) should also apply the same
   three-tier platform resolution. Journey result cards currently show no platform at all.
   Even a planned platform is useful context when choosing which end of the train to board.

**Effort:** Small. The data is already ingested. This is a plumbing + UI label change.
**Impact:** High perceived quality. Platform is one of the first things passengers look for.

### 5.5 Push notifications on Critical promotions
The SSE stream is good for open browser tabs but not for users who have closed the tab.
Web Push (via `web_push` crate or a service like Ntfy) would let users subscribe to
alerts for specific trains.

---

## 6. Operational Maintenance

### 6.1 No cleanup job for `timetable_calls` rows
`timetable_calls` accumulates ~500k rows per weekly GTFS import with no expiry
mechanism. After a month, the table holds 2M+ rows for operating dates that have
already passed. The departure-board query `WHERE location_crs = $1 AND operating_date = $2`
uses the `tc_location_date_idx` index so reads stay fast, but the table grows unboundedly
and `VACUUM` time increases with size.
**Fix:** add a scheduled cleanup task in `main.rs` (or a separate CLI subcommand
`railpredict prune`) that runs `DELETE FROM timetable_calls WHERE operating_date < CURRENT_DATE - INTERVAL '7 days'`
once per day. Note: the column is `operating_date DATE`, not `departure_time` — there
is no timestamp column on this table, so `NOW()` comparisons require `CURRENT_DATE`. Wire it as a tokio task with a 24h interval, or invoke it manually
before each GTFS refresh. Similarly prune `delay_history` rows older than
`MAX_SAMPLES` × 7 days per pattern.

### 6.2 PollManager cannot proactively wake Dormant trains from Tier A
Currently the only way a train enters the registry is via a Darwin STOMP message
(`IngestionPipeline` registers it on first TS message receipt). A train departing in
90 minutes (which should be `Monitored`) will not enter the registry until Darwin
mentions it — which may not happen until 30 minutes before departure. Until then, the
departure board entry comes from the DB timetable with no live state attached.
**Fix:** at startup (after `load_history`), query `timetable_calls WHERE operating_date = today
AND scheduled_departure > NOW()` and call `registry.warm()` with a `TrainStatus` for
each future departure, setting the state via `TrainState::from_departure`. Register
each with `PollManager` at the appropriate interval. This closes the gap between Tier A
(timetable data) and the live state machine without waiting for Darwin to mention the train.

### 6.4 `timetable_calls` God Table — journey handler self-join will degrade

The journey search feature added in v1.7.0 (`journey_handler` in `src/api/handlers.rs`)
finds trains serving both origin and destination by doing a **self-join** on `timetable_calls`:

```sql
SELECT t1.uid, t1.scheduled_departure, t2.scheduled_arrival
FROM timetable_calls t1
JOIN timetable_calls t2
  ON t1.uid = t2.uid AND t1.operating_date = t2.operating_date
WHERE t1.location_crs = $1   -- origin
  AND t2.location_crs = $2   -- destination
  AND t1.seq < t2.seq        -- origin must come before destination
  AND t1.operating_date = $3
ORDER BY t1.scheduled_departure;
```

This is a two-side index scan plus a hash join. With the existing `tc_location_date_idx`
on `(location_crs, operating_date, scheduled_departure)`, each side of the join is fast
in isolation. But as `timetable_calls` crosses 1M rows (approximately 2 full weekly GTFS
imports without pruning), the hash join materialises a large intermediate result set,
and `EXPLAIN ANALYZE` will show it switching to a sequential scan on the inner side.
At 5M rows the query crawls past 500ms.

**Context:** the 6.1 cleanup job prunes rows older than 7 days. But one weekly GTFS
import alone can add ~500k rows (approx. 2,500 services × 20 stops average). Two
overlapping imports (old data not yet pruned, new data just loaded) push the table
to ~1M rows transiently during the refresh window. This is the "God Table" problem:
a table that holds both hot (today's departures) and cold (yesterday's that haven't
been pruned yet) data, making index selectivity poor.

**Fix — three-layer approach (implement in order):**

1. **Verify index coverage for the self-join.** The existing `tc_location_date_idx`
   covers `(location_crs, operating_date, scheduled_departure)`. Add a second index:
   ```sql
   CREATE INDEX IF NOT EXISTS tc_uid_date_seq_idx
       ON timetable_calls(uid, operating_date, seq);
   ```
   This lets the join on `(uid, operating_date)` use an index scan on the inner side
   rather than a hash. Add this as a new migration.

2. **Add a `connections` materialised view** (or a summary table) refreshed nightly
   that pre-computes origin→destination pairs for each service UID:
   ```sql
   CREATE MATERIALIZED VIEW service_connections AS
   SELECT
       t1.uid,
       t1.operating_date,
       t1.location_crs AS origin_crs,
       t2.location_crs AS destination_crs,
       t1.scheduled_departure,
       t2.scheduled_arrival,
       t1.seq AS origin_seq,
       t2.seq AS destination_seq
   FROM timetable_calls t1
   JOIN timetable_calls t2
     ON t1.uid = t2.uid
    AND t1.operating_date = t2.operating_date
    AND t1.seq < t2.seq;

   CREATE INDEX sc_origin_dest_date_idx
       ON service_connections(origin_crs, destination_crs, operating_date, scheduled_departure);
   ```
   `journey_handler` queries `service_connections` instead of `timetable_calls` directly.
   The self-join cost is paid once per day, not on every HTTP request.

3. **Refresh strategy.** Add a `tokio::spawn` task in `main.rs` (alongside the existing
   24h cleanup task from 6.1) that calls `REFRESH MATERIALIZED VIEW CONCURRENTLY service_connections`
   once per day, after the cleanup job runs. `CONCURRENTLY` means the view is not locked
   during refresh — queries continue to use the old version until the refresh completes.
   Requires a unique index on the view (add `uid, operating_date, origin_seq, destination_seq`
   as the uniqueness constraint).

**Effort:** Medium (two migrations + handler query swap + refresh task).
**When to prioritise:** Before a full national GTFS import. Regional scope (~50k rows/week)
is fine without this. Flag it when `timetable_calls` row count crosses 500k in production.

### 6.3 HTMX SSE client has no error handling — freezes silently on disconnect ✓ COMPLETED v1.1.1
`detail.rs` handles `RecvError::Lagged` and `RecvError::Closed` on the server side, but
the client HTML has no `htmx:sseError` event listener. When the SSE connection drops
(network interruption, server restart, circuit breaker entering Cache Only mode), the
`div#live-status` simply freezes on whatever the last update was — no banner, no retry
indicator, no visual difference from "healthy".
**Fix:** in `detail.rs`'s `base()` layout or in the live section, attach a JavaScript
`htmx.on("htmx:sseError", ...)` handler that reveals a hidden "Live updates paused"
banner (the banner slot already exists in `layout.rs`) and sets a CSS class on the
live section. On `htmx:sseOpen` (reconnect), hide the banner again. This gives users
an honest signal when they are seeing stale data, consistent with the Circuit Breaker's
"Cache Only" state on the backend.

---

## 7. Performance & Observability

### 7.1 Departure board handler holds many read locks sequentially ✓ COMPLETED v1.4.0
`departures_handler` and `departures_fragment` both iterate `snapshot_all()` and call
`arc.read().await` inside a loop. With 500 active trains, this is 500 sequential async
lock acquisitions per page load. `snapshot_all` returns `Arc<RwLock<TrainStatus>>` clones
— this is fine for correctness but slow.
**Fix (simple):** add a `departure_snapshot(crs: &str)` method to `TrainRegistry` that
acquires each lock and copies the needed fields in one pass, returning plain structs.
Handlers get a `Vec<DepartureBoardEntry>` directly without touching `AppState` guts.

### 7.2 No Prometheus metrics endpoint ✓ COMPLETED v1.1.2

> `/metrics` exists with ingestion counters, `registry_train_count`, `db_flush_duration_ms`,
> `prediction_error_mins` and (v1.21.1) `darwin_feed_lag_seconds`. There are **no per-route
> HTTP latency histograms**, and `gbr_api_latency_ms` only records on the unwired GBR path.
`tracing` is wired but there is no `/metrics` endpoint. Latency histograms per route,
registry size, flush counts, STOMP reconnect counts, circuit breaker state — none are
observable without reading logs.
**Fix:** add `prometheus` + `axum-prometheus` crate; expose `GET /metrics`.

### 7.3 DB pool is hardcoded at 10 — no tuning path ✓ COMPLETED v1.4.0
`PgPoolOptions::new().max_connections(10)` is a guess. The correct value depends on the
Postgres `max_connections` setting and the number of concurrent flush + query tasks.
**Fix:** read `DB_MAX_CONNECTIONS` env var (default 10); document the calculation in
`.env.example`.

---

## 8. Blind Spots — Subtle Correctness Risks

### 8.1 "Last writer wins" race between Darwin push and GBR poll
`TrainRegistry` stores `Arc<RwLock<TrainStatus>>`. The `SequenceGuard` in `filter.rs`
prevents a stale **Darwin** message from overwriting a newer one, but it only guards
messages coming from the STOMP firehose. Once the networking layer is wired (Improvements.md
item 2.1), a PollManager-triggered GBR fetch (Tier C) will also write to `TrainStatus`.
If a Darwin TS message arrives 50ms into a 200ms GBR round-trip, both writes race to the
same `RwLock`. The RwLock serialises them correctly, but **the GBR response will overwrite
the Darwin update** with data that was fetched before the Darwin message was processed.
Darwin is always more current than a polled GBR response.

**Fix:** before applying any GBR poll result to `TrainStatus`, compare the GBR response's
embedded timestamp (once response parsing is implemented — see item 2.2) against the
`Stamped::last_updated` on the fields being written. Only apply if the GBR data is newer.
This mirrors the same "never overwrite with older data" principle the `SequenceGuard`
already enforces for Darwin messages.

### 8.3 Cross-source write ordering — `SequenceGuard` only guards Darwin→Darwin races

**Background (read 8.1 first):** Item 8.1 (completed in TierCWiring v1.6.0) added a
per-field timestamp comparison before any GBR poll result is applied to `TrainStatus`.
This prevents a slow GBR round-trip from overwriting a fresher Darwin update.

**The gap still open:** The existing `SequenceGuard` (in `src/ingestion/filter.rs`) only
tracks message sequence numbers for the Darwin STOMP firehose. It guarantees Darwin messages
are applied in monotonic order. It does not participate in writes from the GBR poll path
(which goes through `src/networking/gbr_client.rs` and writes directly to the registry).

The current fix (8.1) is a "check before write" pattern: before applying the GBR result,
compare timestamps. This is correct under sequential logic, but there is still a narrow
TOCTOU (time-of-check / time-of-use) window:

```
Thread A (Darwin): read lock → check timestamp → ... [preempted] ...
Thread B (GBR):    write lock → apply GBR data (newer timestamp) → release
Thread A (Darwin): resumes   → acquires write lock → applies Darwin data
                               ↑ Darwin check was done before B wrote; now overwrites newer GBR data
```

This is a low-probability race — it requires preemption at exactly the right instruction.
But the "Stale Data Overlay" (the visual indicator that a field is stale) will flicker
incorrectly if it fires, which is the user-visible symptom. Under high Darwin message
rates (400 msg/s at peak) this race fires at measurable frequency.

**Fix — monotonic version counter per TrainStatus field:**

1. **Add a `version: u64` field to `Stamped<T>`** in `src/types/train_status.rs`.
   This is an atomically-incrementing sequence number, separate from `last_updated`
   (which is wall-clock time and can repeat if two events land in the same millisecond).
   Keep `last_updated` for UI display ("updated 3s ago"); use `version` for write ordering.

   ```rust
   pub struct Stamped<T> {
       pub value: T,
       pub last_updated: DateTime<Utc>,
       pub version: u64,  // monotonic; 0 = never written
   }
   ```

2. **Write helper: `Stamped::apply_if_newer`.**
   ```rust
   impl<T: Clone> Stamped<T> {
       pub fn apply_if_newer(&mut self, incoming: &Stamped<T>) -> bool {
           if incoming.version > self.version {
               *self = incoming.clone();
               true
           } else {
               false
           }
       }
   }
   ```
   Every write to `TrainStatus` — from any source (Darwin, GBR poll, prediction engine) —
   must go through `apply_if_newer` rather than directly assigning fields. The caller that
   constructs the `Stamped` value must assign an appropriate `version`:
   - Darwin messages: use the STOMP `sequence` number from the `SequenceGuard` (already
     tracked in `filter.rs`; thread it into the `Stamped` constructor).
   - GBR poll results: use a global monotonic counter (`AtomicU64` in `AppState`,
     incremented on each successful GBR response) as the version.
   - Prediction engine writes: use the same global counter (it's writing estimated values,
     not observed ones; Darwin observations should always beat predictions).

3. **Assign version priority correctly.** Darwin observations must always beat prediction
   engine outputs. GBR poll results sit between the two: they are real observations but
   have higher latency than Darwin push. A clean ordering:
   - Darwin STOMP: version space `[1_000_000_000, ∞)` — use the sequence number directly
     (Darwin sequences are large integers, already monotonic).
   - GBR poll: version space `[1, 999_999_999]` — use `AtomicU64` per-response counter.
   - Prediction engine: version `0` — predictions never beat observed data.

   This means a Darwin update at sequence 1_000_000_005 will always beat a GBR poll
   response with version 2, even if the GBR response arrived later in wall time.

4. **Remove the TOCTOU window.** With `apply_if_newer` inside the write lock, the
   check and the write are atomic with respect to the `RwLock`. The preemption scenario
   described above is closed: Thread A cannot observe a stale version and then apply
   it later, because by the time it acquires the write lock, `self.version` reflects
   Thread B's write, and `incoming.version > self.version` will be false.

5. **Update `SequenceGuard`.** The guard in `filter.rs` currently compares STOMP sequence
   numbers and drops messages where `seq <= last_seen`. After this change, the guard's role
   becomes: construct the `Stamped<T>` with the correct `version` set to the STOMP sequence,
   and pass it to the registry. The registry itself, via `apply_if_newer`, handles the
   final comparison. The guard no longer needs to duplicate the comparison logic.

**Effort:** Medium (touches `Stamped<T>`, `TrainStatus`, `filter.rs`, registry write paths,
and GBR client response mapping). Best done as a single focused PR — touching these together
avoids a half-migrated state where some writes use `apply_if_newer` and others don't.
**When to prioritise:** Before any production deployment where Darwin and GBR polling
are both active simultaneously (i.e. after Tier C is live).

### 8.2 `delay_history` will become a write-heavy bottleneck at UK-network scale
The 60s flush writes up to ~24,000 rows per flush at Darwin's peak (400 msg/s × 60s,
assuming one observation per message). Each insert checks the `UNIQUE INDEX` on
`(uid, weekday, origin_crs, recorded_at)` for conflict. The `ON CONFLICT DO NOTHING`
is correct for idempotency but still triggers index lookups for every row.
At full UK-network scale (~3,000 active services), `delay_history` grows by ~12M rows
per day if not pruned. After one year without pruning: ~4 billion rows.

This is not a crisis today (dev environment, regional scope) but should be addressed
before a production deployment covering the full national timetable.

**Options in order of complexity:**
1. The prune job in item 6.1 keeps the table bounded — sufficient for regional scope.
2. Postgres range partitioning on `recorded_at` (quarterly partitions) keeps each
   partition small so index maintenance stays fast. Add this as a migration before
   data volume grows. The comment in `20240417120002_create_timetable_calls.sql`
   mentions this for timetable_calls — apply the same thinking to delay_history.
3. TimescaleDB — purpose-built for time-series append workloads, with automatic chunk
   management and compression. Relevant if the full national network is ever targeted.

---

## 9. Technical Debt

### 9.1 `Stamped<bool>` for `is_cancelled` should be `Stamped<Option<bool>>` ✓ COMPLETED v1.4.0
`is_cancelled` defaults to `Stamped::new(false)`, meaning a train that has never
received a Darwin TS message is represented as "definitely not cancelled" rather than
"cancellation status unknown". If a handler serves this train before any Darwin data
arrives, it silently asserts the train is running.
**Fix:** change to `Stamped<Option<bool>>` and update all callers to treat `None` as
"unknown".

### 9.2 `best_delay_mins` and `best_platform` are not documented ✓ COMPLETED v1.4.0
These helper methods on `TrainStatus` make a policy choice (predicted vs reported for
delay; actual vs scheduled for platform) that every UI caller relies on. The choice
should be explicit and tested.

### 9.3 STOMP frame body is read byte-by-byte ✓ COMPLETED v1.4.0

> ⚠️ **Performance note — do not underestimate this fix.** The original description
> classified this as "Trivial / Performance." The actual impact is significant:
> on an unbuffered TCP stream, each `read_exact(&mut buf[..1])` call is a syscall.
> At Darwin's peak of ~400 msg/s with 2–10 KB messages, this is 800k–4M syscalls per
> second on a single thread. Syscall overhead on Linux/macOS is ~100–300 ns each;
> at 4M/s that is 400–1200ms of pure kernel overhead per second — enough to saturate
> an entire core just on reads. In practice, the kernel's TCP receive buffer batches
> some of this, but the async wakeup-per-byte cost in tokio is still measurable.
> Switching to `BufReader` + `read_until(0, &mut body)` (from `AsyncBufReadExt`)
> collapses all per-byte wakeups into one syscall per message body, which should
> reduce Darwin ingestion thread CPU usage by **50–80%** on a loaded instance.
> The fix is indeed small to write, but treat it as high-impact, not housekeeping.

`read_frame` reads the NULL-terminated body one byte at a time in a loop. At Darwin's
peak of ~400 msg/s with typical message sizes of 2–10 KB, this is up to 4 million
single-byte async reads per second. Use `read_until(0, &mut body)` from `AsyncBufReadExt`
with a `BufReader` wrapper on the underlying stream instead — one syscall per message body.

**Implementation detail:** `stomp_client.rs` uses the `BoxReader` abstraction over the
TLS stream. Wrap it in `tokio::io::BufReader` before passing to `read_frame`. The buffer
size should be at least `16 KB` (`BufReader::with_capacity(16_384, stream)`) — large
enough to hold a full Darwin TS message in one read. Verify with `strace`/`dtruss` or
tokio's built-in metrics that the syscall count drops after the change.

### 9.4 `departures_handler` sorts by stringified RFC3339 timestamp ✓ COMPLETED v1.4.0
`entries.sort_by_key(|e| e.scheduled_departure.clone())` sorts by the string
representation of the timestamp. RFC3339 strings sort correctly only if all are in the
same timezone offset. Since all times are UTC (ending in `Z`), this is safe today, but
it's a hidden invariant.
**Fix:** sort by the parsed `chrono::DateTime<Utc>` value in `scheduled_departure.value`
before converting to string, or define an explicit sort key on `DepartureBoardEntry`.

### 9.5 `IngestSource::Cif` panics with `unimplemented!()` ✓ COMPLETED v1.4.0
If a user runs `railpredict ingest-static --source cif`, the process panics.
**Fix:** return a proper `anyhow::bail!("CIF ingest is not yet implemented")` error
so the process exits cleanly with an error message rather than a panic backtrace.

---

## Priority Summary

| #   | Item                                         | Effort  | Impact                       |
|-----|----------------------------------------------|---------|------------------------------|
| 1.1 | STOMP TLS                                    | Medium  | Production-blocking          |
| 1.2 | STOMP auto-reconnect                         | Small   | Production-blocking          |
| 1.3 | `.sqlx/` snapshot + CI                       | Small   | Docker build broken          |
| 2.3 | Add DB to AppState                           | Small   | Tier A unusable              |
| 2.1 | Wire networking → PollManager                | Large   | Tier C non-functional        |
| 2.2 | Parse GBR JSON response                      | Medium  | Tier C non-functional        |
| 6.2 | DB-driven proactive train wake-up            | Medium  | State machine gap            |
| 6.3 | HTMX SSE client error handling               | Small   | UX correctness               |
| 2.4 | Station name autocomplete                    | Medium  | UX blocker for real users    |
| 4.1 | CI pipeline                                  | Small   | Foundational                 |
| 2.5 | GTFS trips + stop_times ingest               | Medium  | Tier A boarding data missing |
| 2.6 | State transitions after registration         | Medium  | State machine incomplete     |
| 6.1 | `timetable_calls` cleanup job                | Small   | DB hygiene                   |
| 1.4 | CORS tightening                              | Small   | Security                     |
| 1.5 | HTTP rate limiting                           | Small   | Security                     |
| 8.1 | Darwin/GBR last-writer-wins race             | Small   | Correctness (after 2.1 done) |
| 9.3 | STOMP byte-by-byte read fix                  | Small   | **High** — 50–80% CPU reduction on Darwin thread |
| 9.1 | `is_cancelled` → `Option<bool>`              | Small   | Correctness                  |
| 8.2 | `delay_history` partitioning plan            | Medium  | Scale readiness              |
| 4.4 | Fix README tech stack table                  | Trivial | Housekeeping                 |
| 8.3 | Cross-source write ordering (`Stamped` version counter) | Medium | Correctness under concurrent Darwin+GBR |
| 5.6 | Planned platform display from GTFS stop_times | Small  | UX quality (data already ingested)   |
| 6.4 | `timetable_calls` self-join optimisation + `service_connections` view | Medium | Journey search scaling |
| 2.8 | GBR Purchase API / Tier C checkout flow       | Large   | Transactional completeness (highest risk) |
