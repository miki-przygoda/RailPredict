# Tech Debt & Deferred Refactors

Forward-looking ledger of deferred work. The two approved refactor surgeries (§A) and the
full dead-code / no-value sweep (§B) from the read-only audit are **complete** — see the
summary below and `CHANGELOG.md` (v1.12.9–v1.12.11). What remains live is the **Tier-C
production-handoff checklist (§C)**. Full per-finding detail for everything lives in the
audit in `data/refactor-audit/` (six files).

---

## Completed (this session)

**Robustness pass (v1.12.9):** typed circuit-breaker routing (no more `msg.contains("503")`);
`journey_handler` no longer leaks raw DB errors; request-path query failures now logged;
`/report` moved onto the rate-limited router; `run_date` rejects a bad date instead of
fabricating "today"; dead `Config` fields removed; startup ingest channel unified.

**§A — approved surgeries (v1.12.10):**
- **State-machine cut** (`be012f7`) — removed the dead rule engine (`from_departure`/`emergency_promote`/`poll_interval`/`PromotionReason`) and the inert `PollManager` (deleted `poll_manager.rs` + its `main.rs` spawn). Kept the live inline transition logic; `StateChangeEvent` moved to `train_state.rs` and still drives the ingestion→broadcast→SSE path.
- **NP association fix & wire** (`caab859`) — the filter now routes the capital `<Association>` local-name (`Conditional`) to the parser, so the turnround predecessor-delay signal flows end-to-end (covered by an integration test).

**§B — dead-code / no-value sweep (v1.12.11):** removed `trains_at_tiploc`, `TrainStatus.cancellation_reason`, `ParseError::Empty`, `GtfsTrip.trip_headsign`, the `extract_stops_txt` wrapper, the orphan JSON `/stations/search` route + `trains_today`, and the stale `#[allow]`s; moved the journey self-join SQL into `db::static_data::direct_journeys`; deduped the two startup paths (`build_station_index` + `assemble_app_state`). **Kept on purpose:** `db::operators::list_operators` (Phase 3 `/operators` UI), `TrainId::headcode` (public API), `Stamped::is_stale`, and the Tier-C-staged `check_tiploc_cascade`.

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

### C3. Operator league / drill-down render empty until GTFS populates `services.toc`
- **Refs:** `db/operators.rs` (`operator_league`/drill-down filter on `services.toc`); dashboard `/operators` links in `frontend/dashboard.rs`.
- **Issue:** the operator analytics queries filter on `services.toc`; until the GTFS static ingest runs (CLI `ingest-static`, or Dev Console → Ingest panel), `toc` is null and these render empty. The dashboard also links to a not-yet-registered `/operators` page (Phase 3 UI).
- **Action:** run the GTFS ingest once on deploy to backfill `services.toc` + seed `operators` (small tables; `delay_history` untouched). The `/operators` links land with the Phase 3 page (see the visual-changes-plan).
- **Audit:** `data/refactor-audit/db-layer.md` (awaiting-UI section) + `data/refactor-audit/frontend.md`.

---

## Where to read more

Full file:line detail and per-finding reasoning live in the read-only refactor audit:
`data/refactor-audit/{core-types-statemachine, networking-ingestion, cache-prediction-weather, db-layer, api-wiring, frontend}.md`.
