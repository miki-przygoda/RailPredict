# Technical Debt — Correctness, Performance & Cleanup

_Small, targeted fixes across the codebase. None require architectural changes. Most are
independent and can be batched in a single agent pass._

_Prereqs:_
- _Item 4.5 (dead_code suppression removal) depends on TierCWiring epic being complete._
  _All other items are independent._

---

## Items from Improvements.md

### 9.1 — `Stamped<bool>` for `is_cancelled` should be `Stamped<Option<bool>>`
`is_cancelled` defaults to `Stamped::new(false)`, meaning a train that has never received
a Darwin TS message is represented as "definitely not cancelled" rather than "unknown".
A handler serving this train before any Darwin data arrives will silently assert it is
running.

**Fix:**
- Change `is_cancelled: Stamped<bool>` to `is_cancelled: Stamped<Option<bool>>` in
  `src/types/train_status.rs`.
- Update `TrainStatus::new` to initialise it as `Stamped::new(None)`.
- Update all callers that read `is_cancelled.value` to treat `None` as "unknown" /
  not-yet-confirmed-running.
- Update the departure board rendering to show no cancellation badge (rather than
  a "running" badge) when the value is `None`.
- Adjust the test in `train_status.rs` that checks `new_status_is_not_cancelled` to
  match the new semantics.

---

### 9.2 — `best_delay_mins` and `best_platform` are not documented
These helper methods make a policy choice that every UI caller relies on:
- `best_delay_mins`: prefers `reported_delay_mins`, falls back to `predicted_delay_mins`.
- `best_platform`: prefers `actual_platform`, falls back to `scheduled_platform`.

**Fix:**
- Add a one-line doc comment above each method in `src/types/train_status.rs` making the
  priority explicit (e.g. `// Returns reported delay if known, otherwise the prediction.`).
- Add a dedicated test for each: one where both fields are set (verifying the priority),
  one where only the fallback is set (verifying the fallback is returned).

---

### 9.3 — STOMP frame body is read byte-by-byte
`read_frame` in `src/ingestion/stomp_client.rs` reads the NULL-terminated body one byte
at a time. At Darwin's peak (~400 msg/s, 2–10 KB messages) this is up to 4 million
single-byte async reads per second.

**Fix:**
- Replace the byte-by-byte read loop with `AsyncBufReadExt::read_until(0, &mut body)`.
- This is a single syscall per message body instead of `N` syscalls.
- Ensure `BufReader` wraps the stream (it likely already does — check before modifying).
- Verify the existing STOMP mock tests still pass after the change.

---

### 9.4 — `departures_handler` sorts by stringified RFC3339 timestamp
`entries.sort_by_key(|e| e.scheduled_departure.clone())` sorts by the string
representation. This happens to be correct since all times are UTC (ending in `Z`), but
it is a hidden invariant.

**Fix:**
- In `src/api/handlers.rs`, change the sort key to the parsed `chrono::DateTime<Utc>`
  value rather than the string representation.
- If `DepartureBoardEntry::scheduled_departure` is already a `String` in the DTO, parse
  it back to `DateTime<Utc>` for sorting, or add a separate sort-key field. Alternatively,
  sort before converting to the DTO.
- Add a test that verifies departure entries are returned in chronological order.

---

### 9.5 — `IngestSource::Cif` panics with `unimplemented!()`
Running `railpredict ingest-static --source cif` causes a panic rather than a clean exit.

**Fix:**
- In `src/cli.rs` or `src/ingestion/gtfs.rs` (wherever the CIF branch lives), replace
  `unimplemented!()` with `anyhow::bail!("CIF ingest is not yet implemented")`.
- Process exits cleanly with a non-zero exit code and a human-readable error message.

---

### 7.1 — Departure board handler holds many read locks sequentially
`departures_handler` and `departures_fragment` iterate `snapshot_all()` and call
`arc.read().await` inside a loop. With 500 active trains, this is 500 sequential async
lock acquisitions per page load.

**Fix:**
- Add a `departure_snapshot(crs: &str) -> Vec<DepartureBoardEntry>` method to
  `TrainRegistry` (in `src/cache/train_registry.rs`) that:
  1. Iterates all entries once.
  2. Acquires each `RwLock` read guard.
  3. Filters by `origin_crs == crs` and copies the needed fields into a plain struct.
  4. Returns `Vec<DepartureBoardEntry>` directly.
- Handlers call this single method rather than acquiring locks individually.
- This also removes the need for handlers to access `AppState` internals (registry field)
  directly — cleaner separation.

---

### 7.3 — DB pool is hardcoded at 10
`PgPoolOptions::new().max_connections(10)` is a guess with no tuning path.

**Fix:**
- Read `DB_MAX_CONNECTIONS` env var in `src/config.rs` (type: `u32`, default: `10`).
- Pass it through to `PgPoolOptions::new().max_connections(config.db_max_connections)` in
  `main.rs`.
- Add the var to `.env.example` with a comment: "Default 10. Set to
  Postgres max_connections / number of app instances, leaving headroom for admin tools."

---

### 4.5 — Remove `#[allow(dead_code)]` suppressors (prereq: TierCWiring complete)
Once the networking layer is wired (TierCWiring epic), these suppressors will no longer
be needed. Leaving them in hides legitimate future dead-code warnings.

**Fix (only after TierCWiring is complete):**
- Remove all `#[allow(dead_code)]` attributes from `networking/`, `ingestion/stomp_client.rs`,
  and `api/mod.rs`.
- Compile with `cargo clippy -- -D warnings`.
- Delete any code that remains genuinely unreachable; do not re-suppress.

---

## Priority Order

1. 9.5 (CIF panic) — one-line fix, highest safety ROI
2. 9.3 (STOMP byte-by-byte) — performance fix, easy swap
3. 9.4 (sort timestamp) — correctness, easy fix
4. 9.1 (`is_cancelled` Option) — semantic correctness across all callers
5. 7.3 (DB pool env var) — ops tuning, small config change
6. 9.2 (document helpers) — documentation + tests
7. 7.1 (departure board lock snapshot) — performance, slightly larger refactor
8. 4.5 (dead_code cleanup) — only after TierCWiring is complete

---

## Files Expected to Change

- `RailPredict/src/types/train_status.rs` (9.1, 9.2)
- `RailPredict/src/ingestion/stomp_client.rs` (9.3)
- `RailPredict/src/api/handlers.rs` (9.4)
- `RailPredict/src/cli.rs` or `src/ingestion/gtfs.rs` (9.5)
- `RailPredict/src/cache/train_registry.rs` (7.1)
- `RailPredict/src/config.rs` (7.3)
- `RailPredict/src/main.rs` (7.3 wiring)
- `.env.example` (7.3 documentation)
- All files with `#[allow(dead_code)]` in `networking/` and `ingestion/` (4.5, after TierCWiring)
