# TODO

The current version and last worked on date should be noted at the top of this file below this line:

**version = "1.16.1" -- 05/06/2026**

---

## How This File Works

- This is the active sprint tracker. It reflects what needs to be done **right now**.
- Larger epics live in `TODOs/` as their own files. Link to them from here when active.
- **Versioning rules (strict):**
  - Each completed TODO point → patch bump: `x.x.1 → x.x.2`
  - Each completed section/epic → minor bump: `x.1.x → x.2.0`
  - On any version bump, update the version string in: `CLAUDE.md`, `README.md`, `TODO.md`, `CHANGELOG.md`, `RailPredict/Cargo.toml` — all five, simultaneously.
  - Note the completion in `CHANGELOG.md` before moving on.
- `TODOs/` files are created when an epic starts and **destroyed** when it is complete. On destruction: summarise into `CHANGELOG.md`, bump the minor version, update all five files.
- The version header format and this section header are **permanent** — do not modify them.

---

## User Action Required

**Generate the `.sqlx/` offline snapshot** before any Docker build or CI run:

```bash
docker compose up -d db
cd RailPredict
DATABASE_URL=postgresql://railpredict:railpredict@localhost:5432/railpredict \
  cargo sqlx prepare
git add .sqlx/
git commit -m "chore: add sqlx offline snapshot"
```

The CI sqlx-check step will fail until this is done. See `TODOs/CI_DevEx.md` for full context.

---

## Remaining Epics

Four new items added to `TODOs/Improvements.md` post-v1.7.0. No epic files created yet —
create a focused epic file when starting work on any of these.

| Item | Description                                          | Effort | Prerequisite          |
|------|------------------------------------------------------|--------|-----------------------|
| 8.3  | Cross-source write ordering (`Stamped` version counter) | Medium | Tier C live (✓ v1.6.0) |
| 6.4  | `timetable_calls` self-join opt + materialised view   | Medium | 6.1 prune job (✓ v1.5.0) |
| 5.6  | Planned platform display from GTFS stop_times         | Small  | 2.5 GTFS ingest (✓ v1.5.0) |
| 2.8  | GBR Purchase API / Tier C checkout flow               | Large  | 2.1/2.2 wired (✓ v1.6.0) |

See `TODOs/Improvements.md` (sections 8.3, 6.4, 5.6, 2.8) for full detail on each.

### Completed

| Epic                      | Version | Notes                                                                                                        |
|---------------------------|---------|--------------------------------------------------------------------------------------------------------------|
| Production Hardening      | v1.2.0  | TLS, reconnect, rate limit, CORS, CRS validation, health probe, SECURITY.md                                  |
| CI & Developer Experience | v1.3.0  | GitHub Actions, cargo-deny, DB integration tests, README fix                                                 |
| Technical Debt            | v1.4.0  | 9.1–9.5, 7.1, 7.3 done; 4.5 deferred pending TierCWiring                                                     |
| Tier A Data Layer         | v1.5.0  | 2.3r, 2.4, 2.5, 6.1, 6.2 — full static pipeline; DB merge, autocomplete, GTFS, warm-up                       |
| Tier C Wiring             | v1.6.0  | 2.1, 2.2, 2.6, 8.1, 4.5 — live GBR calls end-to-end; poll consumer, JSON parse, state transitions            |
| Product Features          | v1.7.0  | 5.1, 5.3, 5.4, 5.5, 8.2 — journey search, fare chip, weather volatility, push notifications, DB partitioning |

---

## Tech Debt & Deferred Refactors

Full ledger: **`docs/tech-debt.md`** (indexed against the read-only audit in `data/refactor-audit/`).

**A. Approved refactors — ✓ completed (v1.12.10):**

- ✓ **State-machine cut** (`be012f7`) — removed `from_departure`/`emergency_promote`/`poll_interval`/`PromotionReason` and deleted `poll_manager.rs` (PollManager + registration + `main.rs` spawn). Kept the live inline logic; `StateChangeEvent` moved to `train_state.rs` and still drives ingestion→SSE. Net −525/+42.
- ✓ **Association fix & wire** (`caab859`) — filter taxonomy now routes capital `<Association>` (`Conditional`) to the parser; NP turnround predecessor-delay flows end-to-end, covered by a new integration test.

**B. Verified dead code / no-value items** (marked remove vs keep in the ledger): `trains_at_tiploc`, `is_empty`, `check_tiploc_cascade`/`cascade_trains_for_tiploc` (Tier-C-staged), `StateChangeEvent.reason`, `TrainStatus.cancellation_reason`, `TrainId::headcode` (keep — intentional API), `Stamped::is_stale`, JSON `/stations/search` + `StationResult.trains_today`, the journey self-join SQL in `frontend/` (move to `db/static_data.rs`), `wait_for_shutdown` startup-tail duplication, plus a masked-dead `#[allow]` sweep.

**C. Tier-C production-handoff readiness** (deferred until prod with company creds + their historical data): reconcile `gbr_client.rs` RTT endpoint/auth/UID-vs-RID contract; wire `check_tiploc_cascade` into the delay path; operator league/drill-down stay empty until the GTFS ingest populates `services.toc`.

---

## Strategic Notes (Carry-Forward)

These are cross-cutting design decisions to keep in mind across all epics:

- **The Identifier Problem:** Use `TrainID` everywhere. Never pass raw strings for train identifiers across module boundaries.
- **The Waiter Pattern:** The coalescer in Epic 3 is the most impactful single piece of work for latency. Prioritise it.
- **Backpressure:** GBR will revoke API keys for aggressive polling. The rate limiter and circuit breaker are not optional.
- **The Ingestion Filter:** Apply the region/route filter as step one of ingestion. CPU cost compounds fast on an unfiltered firehose.
- After deploying Phase 1, run the GTFS ingest once to populate `services.toc` and seed `operators` (CLI `ingest-static`, or the Dev Console → Ingest panel). This is the operator "backfill" — it only writes the small `services`/`operators` tables; `delay_history` is untouched.
