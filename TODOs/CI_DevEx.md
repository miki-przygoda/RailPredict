# CI & Developer Experience

_Automated quality gates, dependency auditing, and housekeeping that should be in place
before any contributor other than the primary author works on the codebase._

_Prereqs:_
- _Item 1.3 (.sqlx/ snapshot) requires a running Postgres database to generate the snapshot._
  _This is a **user action** that must be run locally before the rest of this epic can be_
  _completed. See instructions below._
- _Item 4.5 (dead_code removal) depends on TierCWiring being complete. It is tracked in_
  _`TODOs/TierCWiring.md`, not here._

---

## User Action Required Before This Epic: Item 1.3 — `.sqlx/` Offline Snapshot

`Dockerfile` sets `SQLX_OFFLINE=true` in the builder stage, but no `.sqlx/` directory
exists in the repo. `cargo build --release` **will fail** inside Docker because sqlx
cannot verify queries at compile time without either a live DB or the snapshot.

**This step cannot be automated by an agent — it requires a live Postgres database.**

To generate the snapshot locally:
```bash
# From repo root, with Docker Compose running:
docker compose up -d db
cd RailPredict
DATABASE_URL=postgresql://railpredict:railpredict@localhost:5432/railpredict \
  cargo sqlx prepare
# This creates/updates the .sqlx/ directory
git add .sqlx/
git commit -m "chore: add sqlx offline snapshot"
```

Once `.sqlx/` is committed, add `cargo sqlx prepare --check` to the CI pipeline (item 4.1).

---

## Items from Improvements.md

### 4.1 — CI pipeline
No `.github/workflows/` directory exists. Nothing runs automatically on push or PR.

**Fix — create `.github/workflows/ci.yml`:**
```yaml
on: [push, pull_request]
jobs:
  ci:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16-alpine
        env:
          POSTGRES_USER: railpredict
          POSTGRES_PASSWORD: railpredict
          POSTGRES_DB: railpredict
        options: >-
          --health-cmd pg_isready
          --health-interval 10s
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: clippy }
      - uses: Swatinem/rust-cache@v2
      - name: sqlx offline check
        run: cargo sqlx prepare --check
        working-directory: RailPredict
        env:
          DATABASE_URL: postgresql://railpredict:railpredict@localhost:5432/railpredict
      - name: clippy
        run: cargo clippy -- -D warnings
        working-directory: RailPredict
      - name: test
        run: cargo test
        working-directory: RailPredict
        env:
          DATABASE_URL: postgresql://railpredict:railpredict@localhost:5432/railpredict
      - name: release build
        run: cargo build --release
        working-directory: RailPredict
```

Note: `cargo sqlx prepare --check` step requires the `.sqlx/` snapshot to be committed
first (item 1.3 above).

---

### 4.2 — No `cargo-deny` or dependency audit
No `deny.toml`. The dependency tree includes network-facing crates (`reqwest`, `rustls`,
`sqlx`). A supply chain advisory will not be caught.

**Fix:**
- Add `cargo deny check` step to `.github/workflows/ci.yml` after the checkout step.
- Create `deny.toml` at repo root with a permissive starting configuration:
  ```toml
  [advisories]
  vulnerability = "deny"
  unmaintained = "warn"
  yanked = "deny"

  [licenses]
  allow = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "ISC", "OpenSSL"]

  [bans]
  multiple-versions = "warn"
  ```
- Tighten the config over time as the dependency tree stabilises.

---

### 4.3 — No integration tests against a real database
All tests run against in-memory state or mocks. `src/db/` functions are tested only at
compile time. The `db` service in `docker-compose.yml` exists but no test harness uses it.

**Fix:**
- Add `tests/db_integration.rs` (or `tests/db/mod.rs` if split by function).
- Use `#[sqlx::test]` macro — it spins up a schema-migrated Postgres database per test
  function automatically. No shared state between tests.
- Cover:
  - `db::history::load_history` — insert known rows, verify `HistoricalStore` reconstructed correctly.
  - `db::history::flush_history` — write a store, flush, verify rows in DB.
  - `db::static_data::get_station` — insert a station, look it up.
  - `db::static_data::departures_from` — insert `timetable_calls` rows, verify returned correctly.
  - `db::static_data::cheapest_fare` — insert fares, verify cheapest returned.
- These tests require the CI Postgres service (item 4.1) to run.

---

### 4.4 — README tech stack table is out of date
README lists `moka` and `polars` as dependencies. Neither is in `Cargo.toml`.
The actual cache is `dashmap`.

**Fix:**
- Update the tech stack table in `README.md` to match `RailPredict/Cargo.toml` exactly.
- Correct entries: `dashmap` (not `moka`), no `polars`. Add `metrics` +
  `metrics-exporter-prometheus` (added in Observability epic).
- While there: verify all other rows are accurate.

---

## Priority Order

1. 4.4 (README fix) — trivial, no prereqs, do immediately
2. User action: generate `.sqlx/` snapshot (blocks Docker builds)
3. 4.1 (CI pipeline) — foundational; unblocks everything else
4. 4.2 (cargo-deny) — add to CI after 4.1 exists
5. 4.3 (DB integration tests) — requires CI Postgres service from 4.1

---

## Files Expected to Change

- `README.md` (tech stack table fix)
- `.github/workflows/ci.yml` (new file)
- `deny.toml` (new file)
- `tests/db_integration.rs` (new file)
- `.sqlx/` directory (user-generated, committed separately)

---

## Implementation Status

### 4.4 — README tech stack table fix
**COMPLETE** (17/04/2026)
- Replaced `moka` with `dashmap`.
- Removed `polars` row entirely.
- Added `metrics` + `metrics-exporter-prometheus` (Observability epic, v1.1.2).
- Added `tower_governor` row, marked "pending" (not yet in Cargo.toml).
- Expanded table to cover all actual Cargo.toml dependencies: `sqlx`, `axum`,
  `tower-http`, `maud`, `rust-embed`, `clap`, `quick-xml`, `serde`/`serde_json`,
  `tracing`/`tracing-subscriber`.

### 4.1 — CI pipeline
**COMPLETE** (17/04/2026)
- Created `.github/workflows/ci.yml`.
- Triggers on push to any branch and pull_request.
- Postgres 16-alpine service container with health check (`pg_isready`).
- Steps: checkout → cargo-deny (via `taiki-e/install-action`) → rust toolchain
  (stable + clippy) → rust-cache → sqlx offline check → clippy -D warnings →
  cargo test → cargo build --release.
- `DATABASE_URL` env var set on sqlx-check, test, and release-build steps.
- Inline comment on the sqlx step explains the `.sqlx/` prerequisite (item 1.3).

### 4.2 — cargo-deny
**COMPLETE** (17/04/2026)
- Created `deny.toml` at repo root with `version = 2` advisories/licenses sections,
  expanded license allow-list (`Unicode-3.0`, `Unicode-DFS-2016` added over minimum),
  `copyleft = "warn"`, `multiple-versions = "warn"`.
- Added `cargo deny check` step to `ci.yml` (runs after checkout, before toolchain).

### 4.3 — DB integration tests
**COMPLETE** (17/04/2026)
- Created `RailPredict/tests/db_integration.rs` with 11 test functions.
- Uses `#[sqlx::test(migrations = "../../migrations")]` macro (injected `PgPool`,
  no `DATABASE_URL` env var required at test macro invocation).
- Coverage:
  - `load_history` — fresh DB returns empty store.
  - `flush_history` + `load_history` — roundtrip verifies 2 patterns survive.
  - `flush_history` — idempotent (second flush does not duplicate rows).
  - `get_station` — returns `None` for unknown CRS.
  - `get_station` — returns full row for inserted station (KGX, with coords).
  - `departures_from` — empty for station with no calls.
  - `departures_from` — returns 2 calls in `scheduled_departure` order.
  - `departures_from` — excludes calls from a different date.
  - `cheapest_fare` — returns `None` when fares table is empty.
  - `cheapest_fare` — returns cheapest of two valid fares, ignores expired fare.
  - `cheapest_fare` — excludes fares with `valid_from` in the future.
- `sqlx` is already in `[dependencies]` (not `[dev-dependencies]`) so these tests
  compile without any Cargo.toml changes.
- FK constraints in the schema (timetable_calls → services → stations) require
  seeding stations and services before inserting timetable rows; tests do this.

### 1.3 — `.sqlx/` offline snapshot
**PENDING — user action required.** See "User Action Required" section above.
The CI sqlx-check step will fail until `.sqlx/` is committed.
