# Tech Debt & Deferred Refactors

Forward-looking ledger of deferred work. The two approved refactor surgeries (§A) and the
full dead-code / no-value sweep (§B) from the read-only audit are **complete** — see the
summary below and `CHANGELOG.md` (v1.12.9–v1.12.11). The operator-league emptiness item
(old §C3) was also **resolved** by the Phase-2 journeys re-point (v1.16.0). What remains
live is the **Tier-C client/cascade handoff (§C, items C1–C2)**. Full per-finding detail
for everything lives in the audit in `data/refactor-audit/` (six files).

---

## Completed

**Robustness pass (v1.12.9):** typed circuit-breaker routing (no more `msg.contains("503")`);
`journey_handler` no longer leaks raw DB errors; request-path query failures now logged;
`/report` moved onto the rate-limited router; `run_date` rejects a bad date instead of
fabricating "today"; dead `Config` fields removed; startup ingest channel unified.

**§A — approved surgeries (v1.12.10):**
- **State-machine cut** (`be012f7`) — removed the dead rule engine (`from_departure`/`emergency_promote`/`poll_interval`/`PromotionReason`) and the inert `PollManager` (deleted `poll_manager.rs` + its `main.rs` spawn). Kept the live inline transition logic; `StateChangeEvent` moved to `train_state.rs` and still drives the ingestion→broadcast→SSE path.
- **NP association fix & wire** (`caab859`) — the filter now routes the capital `<Association>` local-name (`Conditional`) to the parser, so the turnround predecessor-delay signal flows end-to-end (covered by an integration test).

**§B — dead-code / no-value sweep (v1.12.11):** removed `trains_at_tiploc`, `TrainStatus.cancellation_reason`, `ParseError::Empty`, `GtfsTrip.trip_headsign`, the `extract_stops_txt` wrapper, the orphan JSON `/stations/search` route + `trains_today`, and the stale `#[allow]`s; moved the journey self-join SQL into `db::static_data::direct_journeys`; deduped the two startup paths (`build_station_index` + `assemble_app_state`). **Kept on purpose:** `db::operators::list_operators` (Phase 3 `/operators` UI), `TrainId::headcode` (public API), `Stamped::is_stale`, and the Tier-C-staged `check_tiploc_cascade`.

**Old §C3 — operator league emptiness (resolved v1.16.0–v1.16.x):** the league/drill-down
no longer wait on a GTFS `services.toc` backfill. Phase 2 re-pointed `db/operators.rs` to
read `toc` directly from the `journeys` table (populated by live Darwin `schedule` frames),
and the `/operators` + `/operators/:toc` pages are registered (`api/mod.rs`). The league now
renders real operator coverage (24 TOCs) straight from the live feed — no static-ingest
prerequisite. Closed; only C1 (RTT client contract) and C2 (cascade wiring) remain in §C.

---

## C. Tier-C production-handoff readiness  *(the live section)*

Deferred until the project moves to production with the company's credentials and their
historical data. These are **not** bugs in the current (Tier A/B) system — they are the
checklist for flipping Tier C on.

### C1. Reconcile the GBR/RTT client contract before enabling Tier C
- **Refs:** `networking/gbr_client.rs` endpoint consts / base URL, `RttServiceResponse`.
- **Issue:** `base_url` is `https://api.rtt.io/api`, but the client uses an `x-apikey` header and a RID-keyed `/v1/train/{rid}/status` path. The real Realtime Trains (RTT) API uses **HTTP Basic** auth (username + token) and keys services by **UID + date** (`/json/service/{uid}/{yyyy}/{mm}/{dd}`), with no `/v1/.../status` path.
- **Action:** reconcile endpoint shape, auth scheme, and UID-vs-RID keying against the real RTT (or the company/GBR Retail) API before Tier C goes live — otherwise the client will 401/404. (The `run_date` silent-fallback was already hardened in v1.12.9.)
- **Audit:** `data/refactor-audit/networking-ingestion.md` §2 (DRIFT rows on `gbr_client.rs`).

### C2. Wire `check_tiploc_cascade` into the delay path when Tier C is active
- **Refs:** `ingestion/filter.rs` (`check_tiploc_cascade`), `cache/train_registry.rs` (`cascade_trains_for_tiploc`).
- **Issue:** the knock-on-delay cascade detector and its registry helper are built and tested but have no production call site — staged for Tier C, currently inert. (The unrelated dead `trains_at_tiploc` probe was removed in v1.12.11.)
- **Action:** wire into `ingestion/mod.rs` when Tier C is active.

*(Former C3 — operator-league emptiness — is resolved; see the Completed section above.)*

---

## Where to read more

Full file:line detail and per-finding reasoning live in the read-only refactor audit:
`data/refactor-audit/{core-types-statemachine, networking-ingestion, cache-prediction-weather, db-layer, api-wiring, frontend}.md`.
