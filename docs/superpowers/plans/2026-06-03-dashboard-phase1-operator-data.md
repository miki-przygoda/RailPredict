# Dashboard Phase 1 — Operator (TOC) Data Plumbing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Capture real operator/TOC identity from the GTFS feed and store it on the small `services` table (plus an `operators` reference table), so every historic delay/prediction row can be grouped by operator via a query-time JOIN — with **zero writes to the 6.7M-row `delay_history` table**.

**Architecture:** Operator identity rides in the weekly GTFS feed: `agency.txt` (agency_id → agency_name) and `routes.txt` (route_id → agency_id), with `trips.txt.route_id` linking a UID's trip to its agency. We parse those, derive `uid → toc` (the GTFS agency_id), and write `toc` onto `services` during the existing ingest. A new `operators` table maps `toc → friendly name + brand colour` (names from `agency.txt`, colours from a curated Rust map). Analytics group by JOINing `delay_history`/`prediction_outcomes` to `services` on `uid` — no per-row denormalisation, so the 6.7M history rows are never rewritten.

**Tech Stack:** Rust, sqlx (Postgres), the `csv` crate, axum. GTFS ingest in `src/ingestion/gtfs.rs`.

**Spec:** `docs/superpowers/specs/2026-06-03-dashboard-overhaul-design.md` (§6)
**Exploration:** `data/dashboard-exploration/04-operator-analytics.md`

**Scope note / spec deviation:** The spec lists a `prediction_snapshots` read path under Phase 1. It is **deferred to Phase 4** (its only consumer, the predictions explorer). Phase 1 is purely operator-data plumbing.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `migrations/20240417120014_add_operators.sql` | `services.toc` column + `operators` table | Create |
| `RailPredict/src/ingestion/gtfs.rs` | Parse agency/routes, derive uid→toc, write toc + seed operators | Modify |
| `RailPredict/src/ingestion/operators.rs` | Curated TOC brand-colour map (pure) | Create |
| `RailPredict/src/ingestion/mod.rs` | Register `operators` module | Modify |
| `RailPredict/src/db/operators.rs` | `Operator` struct + `list_operators` read helper | Create |
| `RailPredict/src/db/mod.rs` | Register `operators` db module | Modify |
| `RailPredict/tests/db_integration.rs` | sqlx::test covering toc storage + JOIN + list_operators | Modify |
| `CLAUDE.md`, `README.md`, `TODO.md`, `CHANGELOG.md`, `RailPredict/Cargo.toml` | Version bump to 1.12.6 | Modify |

**Phase-1 scope (YAGNI):** capture + store + reference data + a read helper for the reference table. The actual per-operator analytics GROUP BY queries and the `/operators` pages are Phase 3. Cancellation persistence and the Darwin `<schedule>` live path are out of scope (spec §6 marks them optional/stretch).

---

## Task 1: Migration — `services.toc` column + `operators` table

**Files:**
- Create: `migrations/20240417120014_add_operators.sql`

- [ ] **Step 1: Write the migration**

Create `migrations/20240417120014_add_operators.sql`:
```sql
-- Operator (TOC) identity for per-operator analytics.
--
-- services.toc holds the GTFS agency_id derived from agency.txt/routes.txt
-- (the most reliable operator source in the NR feed). Because services.uid is
-- the PK and both delay_history and prediction_outcomes are keyed on uid,
-- a single toc column on services unlocks operator grouping on ALL historic
-- data via a query-time JOIN — no per-row backfill of the large tables.
--
-- The operators table maps that agency_id to a friendly name + brand colour
-- for the dashboard. Names come from agency.txt; colours from a curated map.

ALTER TABLE services ADD COLUMN IF NOT EXISTS toc TEXT;
CREATE INDEX IF NOT EXISTS services_toc_idx ON services(toc);

CREATE TABLE IF NOT EXISTS operators (
    toc         TEXT        PRIMARY KEY,
    name        TEXT        NOT NULL,
    brand_color CHAR(7)     NOT NULL DEFAULT '#9aa7b4',
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

- [ ] **Step 2: Verify migration compiles into the binary**

The app runs `sqlx::migrate!` at startup; tests use `#[sqlx::test]` which applies migrations to an ephemeral DB. Confirm the crate still builds (the `migrations/` dir is compiled in):
```bash
cd RailPredict && cargo build 2>&1 | tail -3
```
Expected: clean build. (No DB needed to compile.)

- [ ] **Step 3: Commit**
```bash
git add migrations/20240417120014_add_operators.sql
git commit -m "feat(db): operators table + services.toc column"
```

---

## Task 2: GTFS pure parsers — route_id on trips, agency, routes, derivation (TDD)

**Files:**
- Modify: `RailPredict/src/ingestion/gtfs.rs` (row structs + pure parse/derive fns + tests)

- [ ] **Step 1: Add `route_id` to `GtfsTrip` (serde-default so existing test CSVs without the column still parse)**

In `gtfs.rs`, change the `GtfsTrip` struct:
```rust
/// A single row from GTFS `trips.txt`.
#[derive(Debug, Deserialize)]
struct GtfsTrip {
    #[serde(default)]
    route_id: String,
    trip_id: String,
    service_id: String,
    #[allow(dead_code)]
    trip_headsign: Option<String>,
}
```

- [ ] **Step 2: Add agency/route row structs**

Add near the other GTFS row structs:
```rust
/// A single row from GTFS `agency.txt`.
#[derive(Debug, Deserialize)]
struct GtfsAgency {
    #[serde(default)]
    agency_id: String,
    agency_name: String,
}

/// A single row from GTFS `routes.txt`.
#[derive(Debug, Deserialize)]
struct GtfsRoute {
    route_id: String,
    #[serde(default)]
    agency_id: String,
}
```

- [ ] **Step 3: Write failing tests for the new pure functions**

Add these tests inside the existing `#[cfg(test)] mod tests` block in `gtfs.rs`:
```rust
    #[test]
    fn parse_agency_maps_id_to_name() {
        let csv = "agency_id,agency_name,agency_url,agency_timezone\n\
                   GW,Great Western Railway,http://x,Europe/London\n\
                   VT,Avanti West Coast,http://y,Europe/London\n";
        let m = parse_agency(csv.as_bytes()).unwrap();
        assert_eq!(m.get("GW").map(String::as_str), Some("Great Western Railway"));
        assert_eq!(m.get("VT").map(String::as_str), Some("Avanti West Coast"));
    }

    #[test]
    fn parse_routes_maps_route_to_agency() {
        let csv = "route_id,agency_id,route_short_name,route_type\n\
                   R1,GW,GWR,2\n\
                   R2,VT,AWC,2\n";
        let m = parse_routes(csv.as_bytes()).unwrap();
        assert_eq!(m.get("R1").map(String::as_str), Some("GW"));
        assert_eq!(m.get("R2").map(String::as_str), Some("VT"));
    }

    #[test]
    fn derive_uid_toc_resolves_via_route_first_seen_wins() {
        let trips = vec![
            GtfsTrip { route_id: "R1".into(), trip_id: "C12345_20240417".into(), service_id: "WD".into(), trip_headsign: None },
            GtfsTrip { route_id: "R2".into(), trip_id: "C12345_20240418".into(), service_id: "WD".into(), trip_headsign: None }, // same uid, later — ignored
            GtfsTrip { route_id: "RX".into(), trip_id: "D99999_20240417".into(), service_id: "WD".into(), trip_headsign: None }, // unknown route — skipped
        ];
        let mut routes = HashMap::new();
        routes.insert("R1".to_string(), "GW".to_string());
        routes.insert("R2".to_string(), "VT".to_string());
        let m = derive_uid_toc(&trips, &routes);
        assert_eq!(m.get("C12345").map(String::as_str), Some("GW")); // first-seen wins
        assert_eq!(m.get("D99999"), None); // unknown route → no toc
    }
```

- [ ] **Step 4: Run tests, confirm FAIL (functions undefined)**
```bash
cd RailPredict && cargo test --lib ingestion::gtfs 2>&1 | tail -15
```
Expected: compile error — `parse_agency`/`parse_routes`/`derive_uid_toc` not found.

- [ ] **Step 5: Implement the three pure functions**

Add near the other pure parse functions in `gtfs.rs`:
```rust
/// Parse `agency.txt` into a map of `agency_id → agency_name`.
/// Rows with an empty agency_id are skipped.
fn parse_agency(csv_bytes: &[u8]) -> anyhow::Result<HashMap<String, String>> {
    let mut reader = csv::Reader::from_reader(csv_bytes);
    let mut map = HashMap::new();
    for result in reader.deserialize::<GtfsAgency>() {
        match result {
            Ok(row) if !row.agency_id.is_empty() => {
                map.insert(row.agency_id, row.agency_name);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "Skipping malformed GTFS agency.txt row"),
        }
    }
    Ok(map)
}

/// Parse `routes.txt` into a map of `route_id → agency_id`.
/// Rows with an empty agency_id are skipped.
fn parse_routes(csv_bytes: &[u8]) -> anyhow::Result<HashMap<String, String>> {
    let mut reader = csv::Reader::from_reader(csv_bytes);
    let mut map = HashMap::new();
    for result in reader.deserialize::<GtfsRoute>() {
        match result {
            Ok(row) if !row.agency_id.is_empty() => {
                map.insert(row.route_id, row.agency_id);
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "Skipping malformed GTFS routes.txt row"),
        }
    }
    Ok(map)
}

/// Derive `uid → toc (agency_id)` from trips and a `route_id → agency_id` map.
/// First-seen UID wins (mirrors the service-build rule). UIDs whose route does
/// not resolve to a non-empty agency are omitted (they get NULL toc).
fn derive_uid_toc(
    trips: &[GtfsTrip],
    route_to_agency: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    for trip in trips {
        let Some(uid) = extract_uid(&trip.trip_id) else {
            continue;
        };
        if map.contains_key(uid) {
            continue;
        }
        if let Some(agency) = route_to_agency.get(&trip.route_id) {
            if !agency.is_empty() {
                map.insert(uid.to_owned(), agency.clone());
            }
        }
    }
    map
}
```

- [ ] **Step 6: Run tests, confirm PASS**
```bash
cd RailPredict && cargo test --lib ingestion::gtfs 2>&1 | tail -12
```
Expected: all gtfs tests pass (existing + 3 new).

- [ ] **Step 7: Commit**
```bash
git add RailPredict/src/ingestion/gtfs.rs
git commit -m "feat(ingest): parse GTFS agency/routes + derive uid→toc (pure fns)"
```

---

## Task 3: Brand-colour map + wire operator capture into the ingest

**Files:**
- Create: `RailPredict/src/ingestion/operators.rs`
- Modify: `RailPredict/src/ingestion/mod.rs`
- Modify: `RailPredict/src/ingestion/gtfs.rs` (upsert_services gains toc; new upsert_operators; ingest pipeline wiring)

- [ ] **Step 1: Create the brand-colour map module with tests**

Create `RailPredict/src/ingestion/operators.rs`:
```rust
//! Curated brand colours for GB train operating companies.
//!
//! Operator *names* come from the GTFS `agency.txt` feed; this module attaches a
//! brand colour by matching on a normalised name. Unknown operators fall back to
//! a neutral grey. Colours are hex strings suitable for CSS / SVG fills.

/// Neutral fallback for operators we don't have a brand colour for.
pub const DEFAULT_BRAND: &str = "#9aa7b4";

/// Return a brand colour for an operator, matched case-insensitively on a
/// substring of its name. Falls back to [`DEFAULT_BRAND`].
pub fn brand_color(operator_name: &str) -> &'static str {
    let n = operator_name.to_ascii_lowercase();
    // Ordered longest/most-specific first where names could overlap.
    const TABLE: &[(&str, &str)] = &[
        ("great western", "#0a493e"),
        ("avanti", "#11354e"),
        ("south western", "#24398c"),
        ("southeastern", "#00a3e0"),
        ("southern", "#8cc63f"),
        ("thameslink", "#ff5aa7"),
        ("great northern", "#1d1d4e"),
        ("gatwick express", "#ec1c24"),
        ("c2c", "#b7007c"),
        ("chiltern", "#00bfff"),
        ("cross country", "#660f21"),
        ("crosscountry", "#660f21"),
        ("east midlands", "#713563"),
        ("greater anglia", "#d70428"),
        ("hull trains", "#1d1d1b"),
        ("lumo", "#2b2e83"),
        ("london north eastern", "#ce0e2d"),
        ("lner", "#ce0e2d"),
        ("london overground", "#ee7d11"),
        ("merseyrail", "#fdb913"),
        ("northern", "#262262"),
        ("scotrail", "#1e3a8a"),
        ("transpennine", "#0a0a64"),
        ("transport for wales", "#ee2e24"),
        ("west midlands", "#e07e26"),
        ("elizabeth line", "#7156a5"),
        ("heathrow express", "#532e63"),
        ("grand central", "#1d1d1b"),
        ("caledonian sleeper", "#1b1b3a"),
        ("island line", "#1e90ff"),
        ("stansted express", "#6a1b9a"),
    ];
    for (needle, hex) in TABLE {
        if n.contains(needle) {
            return hex;
        }
    }
    DEFAULT_BRAND
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_operator_gets_brand() {
        assert_eq!(brand_color("Great Western Railway"), "#0a493e");
        assert_eq!(brand_color("AVANTI WEST COAST"), "#11354e");
        assert_eq!(brand_color("London North Eastern Railway"), "#ce0e2d");
    }

    #[test]
    fn unknown_operator_falls_back() {
        assert_eq!(brand_color("Imaginary Trains Ltd"), DEFAULT_BRAND);
        assert_eq!(brand_color(""), DEFAULT_BRAND);
    }
}
```

- [ ] **Step 2: Register the module**

In `RailPredict/src/ingestion/mod.rs`, add alongside the other `pub mod` lines (e.g. after `pub mod gtfs;`):
```rust
pub mod operators;
```

- [ ] **Step 3: Run the operators tests (RED→GREEN already green; verify)**
```bash
cd RailPredict && cargo test --lib ingestion::operators 2>&1 | tail -8
```
Expected: 2 passed.

- [ ] **Step 4: Change `upsert_services` to carry `toc`**

In `gtfs.rs`, change the `upsert_services` signature and body. The services tuple gains an `Option<String>` toc:
```rust
/// Upsert services rows. Each tuple is `(uid, origin_crs, destination_crs, runs_on_days, toc)`.
/// Only services whose origin_crs and destination_crs are in `known_stations` are inserted
/// to avoid FK violations.
async fn upsert_services(
    db: &Db,
    services: &[(String, String, String, i16, Option<String>)],
    known_stations: &HashSet<String>,
) -> anyhow::Result<usize> {
    let filtered: Vec<&(String, String, String, i16, Option<String>)> = services
        .iter()
        .filter(|(_, origin, dest, _, _)| {
            known_stations.contains(origin) && known_stations.contains(dest)
        })
        .collect();

    let mut total = 0usize;
    for chunk in filtered.chunks(SERVICES_CHUNK) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO services (uid, origin_crs, destination_crs, runs_on_days, toc, updated_at) ",
        );
        qb.push_values(chunk, |mut b, (uid, origin, dest, days, toc)| {
            b.push_bind(uid)
                .push_bind(origin)
                .push_bind(dest)
                .push_bind(days)
                .push_bind(toc.clone())
                .push_bind(chrono::Utc::now());
        });
        qb.push(
            " ON CONFLICT (uid) DO UPDATE SET
                origin_crs      = EXCLUDED.origin_crs,
                destination_crs = EXCLUDED.destination_crs,
                runs_on_days    = EXCLUDED.runs_on_days,
                toc             = COALESCE(EXCLUDED.toc, services.toc),
                updated_at      = EXCLUDED.updated_at",
        );
        qb.build().execute(db).await?;
        total += chunk.len();
    }
    Ok(total)
}
```
Note `COALESCE(EXCLUDED.toc, services.toc)` so a later ingest that can't resolve a toc doesn't wipe a previously-known one.

- [ ] **Step 5: Add `upsert_operators`**

Add a new upsert function in `gtfs.rs` near `upsert_services`:
```rust
/// Upsert operator reference rows from `agency_id → agency_name`, attaching a
/// brand colour from the curated map.
async fn upsert_operators(db: &Db, agencies: &HashMap<String, String>) -> anyhow::Result<usize> {
    if agencies.is_empty() {
        return Ok(0);
    }
    let rows: Vec<(&String, &String)> = agencies.iter().collect();
    let mut total = 0usize;
    for chunk in rows.chunks(SERVICES_CHUNK) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO operators (toc, name, brand_color, updated_at) ",
        );
        qb.push_values(chunk, |mut b, (toc, name)| {
            b.push_bind(*toc)
                .push_bind(*name)
                .push_bind(crate::ingestion::operators::brand_color(name))
                .push_bind(chrono::Utc::now());
        });
        qb.push(
            " ON CONFLICT (toc) DO UPDATE SET
                name        = EXCLUDED.name,
                brand_color = EXCLUDED.brand_color,
                updated_at  = EXCLUDED.updated_at",
        );
        qb.build().execute(db).await?;
        total += chunk.len();
    }
    Ok(total)
}
```

- [ ] **Step 6: Wire agency/routes extraction + derivation into `run_ingest_from_bytes`**

In `run_ingest_from_bytes`, after the calendar/trips are parsed and BEFORE the "Phase 6: Build services list" loop, add agency + routes parsing and build the uid→toc map:
```rust
    // --- Operator identity: agency.txt + routes.txt → uid → toc ---
    let agencies = match extract_file(zip_bytes, "agency.txt") {
        Ok(bytes) => parse_agency(&bytes).unwrap_or_default(),
        Err(_) => {
            tracing::warn!("agency.txt not found in GTFS archive; operators will be unlabelled");
            HashMap::new()
        }
    };
    let route_to_agency = match extract_file(zip_bytes, "routes.txt") {
        Ok(bytes) => parse_routes(&bytes).unwrap_or_default(),
        Err(_) => {
            tracing::warn!("routes.txt not found in GTFS archive; operators will be unlabelled");
            HashMap::new()
        }
    };
    let uid_toc = derive_uid_toc(&trips, &route_to_agency);
    tracing::info!(agencies = agencies.len(), uid_toc = uid_toc.len(), "Resolved operator identity");
    if !agencies.is_empty() {
        let n = upsert_operators(db, &agencies).await?;
        emit(progress, |s| s.push_log(format!("Operators: {n}")));
    }
```

Then in the "Phase 6: Build services list" loop, attach the toc when pushing a service. Change the line `services.push((uid, origin_crs, dest_crs, days));` to:
```rust
        let toc = uid_toc.get(&uid).cloned();
        uid_to_days.insert(uid.clone(), days);
        services.push((uid, origin_crs, dest_crs, days, toc));
```
(Keep the existing `uid_to_days.insert` — just ensure the 5-tuple is pushed. If `uid_to_days.insert` already precedes the push, only change the push line and add the `let toc = ...` above it. The `uid` is moved into the tuple, so compute `toc` before moving — `uid_toc.get(&uid)` borrows, `.cloned()` copies the value out, fine before the move.)

- [ ] **Step 7: Build + full test suite**
```bash
cd RailPredict && cargo build 2>&1 | tail -5
cargo test 2>&1 | grep -E "test result:" | head
cargo clippy --lib 2>&1 | grep -E "gtfs.rs|operators.rs" || echo "no new warnings in changed files"
```
Expected: clean build, all green, no new warnings. (DB-integration tests for the new behaviour come in Task 4.)

- [ ] **Step 8: Commit**
```bash
git add RailPredict/src/ingestion/gtfs.rs RailPredict/src/ingestion/operators.rs RailPredict/src/ingestion/mod.rs
git commit -m "feat(ingest): capture operator toc onto services + seed operators table"
```

---

## Task 4: DB read helper + sqlx::test integration coverage

**Files:**
- Create: `RailPredict/src/db/operators.rs`
- Modify: `RailPredict/src/db/mod.rs`
- Modify: `RailPredict/tests/db_integration.rs`

- [ ] **Step 1: Create the read helper**

Create `RailPredict/src/db/operators.rs`:
```rust
//! Read access to the operators reference table.

use crate::db::Db;

/// An operator (TOC) reference row: code, friendly name, brand colour.
#[derive(Debug, Clone, sqlx::FromRow, PartialEq, Eq)]
pub struct Operator {
    pub toc: String,
    pub name: String,
    pub brand_color: String,
}

/// List all operators, ordered by friendly name.
pub async fn list_operators(db: &Db) -> sqlx::Result<Vec<Operator>> {
    sqlx::query_as::<_, Operator>(
        "SELECT toc, name, brand_color FROM operators ORDER BY name",
    )
    .fetch_all(db)
    .await
}
```

- [ ] **Step 2: Register the module**

In `RailPredict/src/db/mod.rs`, add alongside the other `pub mod` lines (e.g. after `pub mod predictions;`):
```rust
pub mod operators;
```

- [ ] **Step 3: Build (helper compiles)**
```bash
cd RailPredict && cargo build 2>&1 | tail -3
```
Expected: clean.

- [ ] **Step 4: Read the existing db_integration.rs test style**

Run:
```bash
cd RailPredict && sed -n '1,60p' tests/db_integration.rs
```
Note the `#[sqlx::test]` signature style (it injects a `PgPool`), and how existing tests insert stations/services (they must satisfy the `services → stations` FK). Match that style exactly for the new test.

- [ ] **Step 5: Add an integration test covering toc storage + JOIN + list_operators**

Append to `RailPredict/tests/db_integration.rs` a test in the same style as the others. Use the same pool parameter type the other tests use (`sqlx::PgPool`). The test:
```rust
#[sqlx::test]
async fn operator_toc_join_and_reference(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // Stations first (services FK → stations.crs).
    sqlx::query("INSERT INTO stations (crs, name) VALUES ('AAA','Alpha'),('BBB','Beta')")
        .execute(&pool).await?;
    // A service carrying a toc.
    sqlx::query("INSERT INTO services (uid, origin_crs, destination_crs, runs_on_days, toc) \
                 VALUES ('C12345','AAA','BBB',127,'GW')")
        .execute(&pool).await?;
    // Operator reference row.
    sqlx::query("INSERT INTO operators (toc, name, brand_color) VALUES ('GW','Great Western Railway','#0a493e')")
        .execute(&pool).await?;
    // A delay_history row keyed only by uid (no toc column).
    sqlx::query("INSERT INTO delay_history (uid, weekday, origin_crs, delay_mins) \
                 VALUES ('C12345',0,'AAA',5)")
        .execute(&pool).await?;

    // The JOIN labels the history row with its operator — no toc on delay_history.
    let toc: String = sqlx::query_scalar(
        "SELECT s.toc FROM delay_history d JOIN services s ON s.uid = d.uid LIMIT 1",
    ).fetch_one(&pool).await?;
    assert_eq!(toc, "GW");

    // The read helper returns the reference row.
    let ops = railpredict::db::operators::list_operators(&pool).await?;
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].name, "Great Western Railway");
    assert_eq!(ops[0].brand_color, "#0a493e");
    Ok(())
}
```
> If the crate name in `tests/` is not `railpredict`, use whatever the other tests use (check the existing `use` lines at the top of `db_integration.rs`). Match the existing import path for `list_operators`.

- [ ] **Step 6: Run the DB integration tests**

These require a Postgres test database (the same one the existing `db_integration.rs` tests use — `DATABASE_URL` / sqlx test config). Run:
```bash
cd RailPredict && cargo test --test db_integration operator_toc_join_and_reference 2>&1 | tail -15
```
Expected: PASS. If the suite cannot connect to a test DB in this environment, report DONE_WITH_CONCERNS noting the test is written and compiles (`cargo test --test db_integration --no-run` succeeds) but could not be executed here — do NOT mark BLOCKED for a missing DB.

- [ ] **Step 7: Commit**
```bash
git add RailPredict/src/db/operators.rs RailPredict/src/db/mod.rs RailPredict/tests/db_integration.rs
git commit -m "feat(db): list_operators read helper + operator JOIN integration test"
```

---

## Task 5: Version bump + backfill documentation

**Files:**
- Modify: `CLAUDE.md`, `README.md`, `TODO.md`, `CHANGELOG.md`, `RailPredict/Cargo.toml`

- [ ] **Step 1: Bump version to 1.12.6 / 03/06/2026 across all five files**

Find the current header line in each (`grep -rn '1.12.5' CLAUDE.md README.md TODO.md RailPredict/Cargo.toml CHANGELOG.md`), then:
- `CLAUDE.md`, `TODO.md`, `CHANGELOG.md`: `**version = "1.12.5" -- 03/06/2026**` → `**version = "1.12.6" -- 03/06/2026**`
- `README.md`: bump its version header (match its existing format, e.g. `**v1.12.5 — June 2026**` → `**v1.12.6 — June 2026**`)
- `RailPredict/Cargo.toml`: `version = "1.12.5"` → `version = "1.12.6"` (package version only)

- [ ] **Step 2: Add a CHANGELOG entry (top of entries)**
```markdown
## [1.12.6] — 2026-06-03
### Added
- Dashboard overhaul **Phase 1 (Operator data plumbing)**: GTFS `agency.txt`/`routes.txt` parsing derives a per-UID operator (`toc`) written onto the small `services` table, plus an `operators` reference table (friendly name from the feed + curated brand colour). Operator grouping on all 6.7M+ historic `delay_history` / `prediction_outcomes` rows is unlocked via a query-time JOIN on `uid` — **no rewrite of the large tables**. Re-run the GTFS ingest to populate `toc` ("backfill").
```

- [ ] **Step 3: Document the backfill step**

In `TODO.md` (or wherever ingest is documented), add a one-line note under the current sprint:
```markdown
- After deploying Phase 1, run the GTFS ingest once to populate `services.toc` and seed `operators` (CLI `ingest-static`, or the Dev Console → Ingest panel). This is the operator "backfill" — it only writes the small `services`/`operators` tables; `delay_history` is untouched.
```

- [ ] **Step 4: Build + full suite green**
```bash
cd RailPredict && cargo build 2>&1 | tail -2 && cargo test 2>&1 | grep -E "test result:" | head
```

- [ ] **Step 5: Commit**
```bash
git add CLAUDE.md README.md TODO.md CHANGELOG.md RailPredict/Cargo.toml RailPredict/Cargo.lock
git commit -m "chore: bump v1.12.6 — operator data plumbing (Phase 1)"
```
Confirm via `git status` that `.gitignore` / `skills-lock.json` are NOT staged.

---

## Self-Review

- **Spec coverage (§6):** capture operator (Task 2/3), store it on `services.toc` + `operators` table (Task 1/3), reference map = brand colours + names (Task 3), backfill = re-run ingest, no big-table write (Task 3/5 + JOIN). The `prediction_snapshots` read path is explicitly deferred to Phase 4 (documented above). Cancellation persistence + Darwin `<schedule>` are out of scope per spec §6 (optional/stretch).
- **Placeholder scan:** none — all code is complete. The only conditional is the DB-test execution caveat (Task 4 Step 6), which is an environment note, not a placeholder.
- **Type consistency:** the services tuple is `(String, String, String, i16, Option<String>)` consistently across `upsert_services` and the build loop; `derive_uid_toc` returns `HashMap<String,String>` consumed by the build loop via `.get(&uid).cloned()`; `Operator { toc, name, brand_color }` columns match the migration and the `list_operators` SELECT; `brand_color(&str) -> &'static str` is consumed by `upsert_operators`.
- **6.7M-row safety:** no migration or query writes per-row to `delay_history`/`prediction_outcomes`; operator labelling is a query-time JOIN on the `services` PK.
