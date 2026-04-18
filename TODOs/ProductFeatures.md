# Product Features — User-Facing Enhancements

_Medium-term features that expand what users can do with the system. None are production
blockers but all are listed in the README vision._

_Prereqs:_
- _5.1 (journey search) needs `timetable_calls` populated — complete TierADataLayer 2.5 first._
- _5.3 (fare display) needs `AppState::db` wired — already done (Improvements 2.3 prereq)._
- _5.4 (weather volatility) is a new external integration — no codebase prereqs._
- _5.5 (push notifications) is a new integration — no codebase prereqs._
- _8.2 (delay_history partitioning) is a DB migration — no codebase prereqs but best done_
  _before data volume grows._

---

## Items from Improvements.md

### 5.1 — Journey search (A→B, not just departures from A)
The README envisions searching between two stations. The current UI shows all departures
from a single origin.

**Fix:**
- **Backend:** Add `GET /journeys?from=CRS&to=CRS&date=YYYY-MM-DD` JSON endpoint.
  - Query: filter `timetable_calls` for services that call both origin and destination
    in order (`call_order` at origin < `call_order` at destination).
  - Return the matching `DepartureBoardEntry` rows sorted by scheduled departure.
  - Apply CRS validation on both parameters.
- **Frontend:** Add a second `<input>` field "To" on the search page. When both fields
  are populated, point `hx-get` at `/ui/journeys` rather than `/ui/stations/departures`.
  - The `/ui/journeys` fragment handler renders the same departure card list, but
    filtered to through-services only.
- **Require:** `timetable_calls` must be populated (TierADataLayer 2.5).

---

### 5.3 — Fare display on train detail page
`db::static_data::cheapest_fare` exists but is never called. The detail page could show
the current cheapest fare for the origin→destination pair when both are known.

**Fix:**
- In `src/frontend/detail.rs`, after rendering the train summary, call
  `db::static_data::cheapest_fare(&state.db, origin_crs, destination_crs)`.
- Render the result as a `span .fare-chip { "From £" (pence_to_pounds(fare)) }` on the
  detail card.
- `cheapest_fare` returns `Option<i32>` (pence) — render nothing if `None`.
- Add a `pence_to_pounds(pence: i32) -> String` formatter (e.g. `"£12.50"`).

---

### 5.4 — Weather-driven volatility promotions
`VolatilityContext` has a `wind_speed_mph: Option<f32>` field and CLAUDE.md specifies
"Wind > 50 mph → force all trains on that route to `Active`". Nothing reads
`wind_speed_mph` or triggers promotions from it.

**Fix — new `src/weather/` module:**

1. **Integration:** Use Open-Meteo API (free, no key required):
   `https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}&hourly=windspeed_10m`
   - Fetch wind speed for route bounding boxes. Define bounding boxes per watched route
     in config (or hardcode a small set for the initial implementation).

2. **Storage:** Add `src/weather/mod.rs` with a `VolatilityStore` (`DashMap<String, f32>`
   mapping route_id → current wind speed mph). Shared via `Arc<VolatilityStore>` in `AppState`.

3. **Polling:** Spawn a tokio task in `main.rs` that refreshes wind speed every 10 minutes.

4. **Promotion trigger:** In `main.rs` or the poll consumer task, after updating the
   weather store, iterate the registry and call `registry.update(id, |status| {
       status.volatility.wind_speed_mph = Some(speed);
       if speed > 50.0 { status.volatility.incident_flagged = true; }
   })` for trains on the affected route.
   Use `emergency_promote` on the `TrainState` if wind exceeds threshold.

---

### 5.5 — Push notifications on Critical promotions
The SSE stream only works for open browser tabs. Users who have closed the tab miss
Critical promotions.

**Fix (scoped to a simple first implementation):**
- Integrate [ntfy.sh](https://ntfy.sh) (open-source, self-hostable push service).
- Add `NTFY_URL` env var (e.g. `https://ntfy.sh/railpredict-alerts`).
- In the state-machine broadcast consumer in `main.rs`, when a train transitions to
  `TrainState::Critical`, send a POST to the ntfy topic:
  ```
  POST {NTFY_URL}
  Title: Train {headcode} now Critical
  Body: {origin_crs} → {destination_crs} dep {scheduled_departure} — disruption detected
  ```
- Gate behind `NOTIFICATIONS_ENABLED=true` env var (default: off).
- No client-side Web Push API needed — users subscribe to the ntfy topic directly.

---

### 8.2 — `delay_history` write-heavy bottleneck at UK-network scale
At Darwin's peak (400 msg/s × 60s × 3,000 services), `delay_history` grows by ~12M rows
per day. After one year without pruning: ~4 billion rows. The prune job (TierADataLayer
6.1) keeps the table bounded for regional scope, but for national-scale deployment the
unique index maintenance becomes expensive.

**Fix — DB migration for range partitioning (do before data volume grows):**
- Create `migrations/20240418_partition_delay_history.sql`:
  - Convert `delay_history` to a range-partitioned table on `recorded_at` (quarterly
    partitions).
  - Add partitions for the current and next 2 quarters.
  - Note: Postgres partitioning requires either a fresh table or careful `ALTER TABLE`
    steps with data migration. Document the manual steps for existing installs.
- Add a comment to the migration noting that TimescaleDB is the next step if full
  national-network scale is ever targeted.
- This is a schema-level change — no Rust code changes required beyond the migration file.

---

## Priority Order

1. 5.3 (fare display) — smallest change, no new dependencies, immediately useful
2. 8.2 (delay_history partitioning) — do before data accumulates, not after
3. 5.1 (journey search) — high UX value; needs TierADataLayer 2.5 first
4. 5.4 (weather volatility) — new integration, moderate complexity
5. 5.5 (push notifications) — nice-to-have; simple with ntfy

---

## Files Expected to Change

- `RailPredict/src/frontend/detail.rs` (5.3 fare display)
- `RailPredict/src/api/mod.rs` (5.1 journey route)
- `RailPredict/src/api/handlers.rs` (5.1 journey handler)
- `RailPredict/src/frontend/search.rs` (5.1 two-input form)
- `RailPredict/src/weather/mod.rs` (new module, 5.4)
- `RailPredict/src/main.rs` (5.4 weather task, 5.5 push notifications)
- `RailPredict/src/config.rs` (5.4 NTFY_URL, 5.5 NOTIFICATIONS_ENABLED)
- `migrations/20240418_partition_delay_history.sql` (new file, 8.2)
