# Tier A Data Layer — Completing the Static Data Pipeline

_The DB schema exists and the departure board queries exist, but the timetable data
is never fully ingested and the handlers don't use the DB. This epic completes the
full Tier A path: GTFS full ingest → DB populated → handlers merge DB + live registry →
proactive train wake-up at startup._

_Prereqs:_
- _2.5 (GTFS full ingest) should be done before 6.2 (proactive wake-up needs `timetable_calls` populated)._
- _`AppState::db` is already wired (completed as Improvements 2.3 prereq in the Observability epic)._
- _No dependency on TierCWiring — Tier A works without live GBR calls._

---

## Items from Improvements.md

### 2.3 (remaining) — Merge DB timetable + live registry in departure board handler
`AppState::db` is now wired. The `departures_from()` query and `timetable_calls` table
exist. But `departures_handler` and `departures_fragment` still only read from
`TrainRegistry` — if Darwin is not connected, the board returns empty.

**Fix:**
- In `departures_handler` (and the htmx fragment equivalent), change the read strategy:
  1. Call `db::static_data::departures_from(&state.db, crs, today)` to get the DB
     timetable rows for today.
  2. For each timetable row, look up the RID in `TrainRegistry`. If found, overlay the
     live fields (`actual_estimated_departure`, `reported_delay_mins`, `actual_platform`,
     `is_cancelled`, `destination_crs`) onto the `DepartureBoardEntry`.
  3. If not in registry, serve the timetable row as-is (Tier A, static data).
- This gives a populated departure board in all environments, with live enrichment when
  Darwin data is available.
- Station name lookup: use `db::static_data::get_station(&state.db, &destination_crs)`
  to resolve the raw CRS to a human-readable name. This unblocks the stub left by the
  FrontEndHardening epic.

---

### 2.4 — Station name autocomplete
The search `<input>` currently requires a CRS code. Normal users type station names.
The `stations` table has a GIN full-text index on `name` that is never queried.

**Fix — two parts:**

**Backend:**
- Add `GET /stations/search?q=<text>` JSON endpoint in `src/api/mod.rs` and handler in
  `src/api/handlers.rs`.
- Query: `SELECT crs, name FROM stations WHERE to_tsvector('english', name) @@ plainto_tsquery($1) LIMIT 10`.
- Returns `Vec<{ crs: String, name: String }>` as JSON.
- Apply CRS validation (3-letter check from item 3.1) to the query result, not the input.

**Frontend:**
- In `src/frontend/search.rs`, replace the plain `<input type="text">` with an htmx
  autocomplete pattern:
  - `hx-get="/stations/search"`, `hx-trigger="input changed delay:300ms"`,
    `hx-target="#station-results"`, `hx-vals='{"q": "..."}` (use `name` attribute for value binding).
  - Render a `<datalist>` or dropdown `<ul id="station-results">` fragment.
  - Fire only after 2+ characters (add `hx-trigger` condition or a short JS guard).
  - On selection, populate the hidden CRS field and submit.

---

### 2.5 — GTFS ingest only populates `stations` — `timetable_calls` and `services` are empty
`src/ingestion/gtfs.rs` parses `stops.txt` only. `trips.txt` + `stop_times.txt` were
marked "pending" in Phase 5 but are unimplemented. The `timetable_calls` table will
always be empty until this is added.

**Fix — extend `run_ingest` in `src/ingestion/gtfs.rs`:**

1. After upserting stations, extract `trips.txt` from the same ZIP.
   - Parse `trip_id`, `service_id`, `trip_headsign` (uid equivalent).
   - Map `service_id` bitmask: `monday`–`sunday` boolean columns in `calendar.txt` →
     `SMALLINT` bitmask (bit 0 = Monday ... bit 6 = Sunday).
   - Upsert into `services (uid, days_bitmask, headsign)`.

2. Extract `stop_times.txt`.
   - Parse `trip_id`, `stop_id`, `arrival_time`, `departure_time`, `stop_sequence`.
   - Map `stop_id` → CRS code via the station lookup populated in step 1.
   - Upsert into `timetable_calls (uid, operating_date, location_crs, call_order, scheduled_departure)`.
   - `operating_date` should be derived from the active service dates; for a rolling
     weekly import, use `CURRENT_DATE` through `CURRENT_DATE + 7`.

3. Handle Network Rail GTFS naming quirks:
   - `stop_id` in NR GTFS is prefixed (e.g. `9100LEEDS` rather than bare `LDS`). Strip
     the prefix and resolve to CRS.
   - `arrival_time`/`departure_time` can exceed 24:00:00 for overnight services — clamp
     to next-day `DateTime<Utc>` rather than panicking.

4. Add tests for the new parse functions (pure functions, no I/O — same pattern as
   existing `parse_stops` tests).

---

### 6.1 — No cleanup job for `timetable_calls` rows
`timetable_calls` grows by ~500k rows per weekly GTFS import with no expiry. After a
month: 2M+ rows for dates that have passed.

**Fix:**
- Add a 24-hour scheduled tokio task in `main.rs` (similar to the existing 60s flush
  task) that runs:
  ```sql
  DELETE FROM timetable_calls WHERE operating_date < CURRENT_DATE - INTERVAL '7 days'
  ```
  and:
  ```sql
  DELETE FROM delay_history WHERE recorded_at < NOW() - INTERVAL '91 days'
  ```
  (91 days = MAX_SAMPLES × 7 — keeps enough history for the prediction engine while
  bounding table growth).
- Log the number of rows deleted at DEBUG level.
- Alternatively, expose as `railpredict prune` CLI subcommand and wire both the task
  and the subcommand to the same underlying `db::maintenance::prune(db)` function.

---

### 6.2 — PollManager cannot proactively wake Dormant trains from Tier A
A train departing in 90 minutes won't enter the registry until Darwin mentions it.
Until then, it has no live state attached. This means `Monitored` trains won't start
polling when they should.

**Fix (depends on 2.5 — `timetable_calls` must be populated):**
- In `main.rs`, after `load_history` and before spawning the ingestion pipeline:
  1. Query `timetable_calls WHERE operating_date = CURRENT_DATE AND scheduled_departure > NOW()`.
  2. For each row, construct a `TrainStatus::new(train_id, scheduled_departure, scheduled_departure)`.
  3. Call `registry.warm(statuses)` to batch-load them.
  4. For each, compute `TrainState::from_departure(scheduled_departure, Utc::now())` and
     register with `PollManager` at the appropriate interval.
- This closes the gap between Tier A timetable data and the live state machine.
- Trains already in the registry (from a previous hot-start) should not be overwritten —
  use `registry.upsert` only if not already present, or check `registry.get` first.

---

## Priority Order

1. 2.5 (GTFS full ingest) — `timetable_calls` must be populated before 2.3, 6.1, 6.2 are useful
2. 2.3 remaining (merge DB + registry in handlers) — immediately improves dev experience
3. 6.1 (cleanup job) — simple task, prevents unbounded growth
4. 2.4 (station autocomplete) — UX blocker for real users
5. 6.2 (proactive wake-up) — closes the Tier A → state machine gap

---

## Files Expected to Change

- `RailPredict/src/ingestion/gtfs.rs` (parse trips + stop_times)
- `RailPredict/src/db/static_data.rs` (verify departures_from, get_station are correct)
- `RailPredict/src/api/handlers.rs` (merge DB + registry, station search endpoint, CRS name lookup)
- `RailPredict/src/api/mod.rs` (add /stations/search route)
- `RailPredict/src/frontend/search.rs` (autocomplete input)
- `RailPredict/src/main.rs` (cleanup task, proactive warm-up)
- Possibly a new `RailPredict/src/db/maintenance.rs` for the prune functions
