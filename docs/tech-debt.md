# Tech Debt & Deferred Refactors

A structured ledger of known-but-deferred work: approved-but-unstarted refactors,
verified dead code, and the Tier-C production-handoff checklist. Every item below was
confirmed against the source at the `ui/page-improvements` tip and cross-referenced to
the read-only refactor audit in `data/refactor-audit/` (six files). When in doubt, the
audit files carry the full file:line detail and reasoning — this ledger is the index.

Severity/category tags (`[HIGH] DEAD`, `[MED] RISK`, …) match the audit's own taxonomy.

---

## A. Decided refactors — approved, not yet done

These two were reviewed and a direction was chosen. They are pending implementation.

### A1. State-machine cut (rule engine + registration are production-dead)

- **Refs:**
  - `state_machine/train_state.rs:92` (`from_departure`), `:117` (`emergency_promote`) — zero production callers; only their own `#[cfg(test)]` tests.
  - `state_machine/poll_manager.rs:76` (`RegistrationMsg`), `:88` (`PollManagerHandles` / `register_tx`), `:157` (`push_registration`) — registration plumbing nothing feeds.
  - `state_machine/poll_manager.rs:67` (`StateChangeEvent.reason`) — always written (`PromotionReason::TimeBased`/`IncidentDetected`), never read by any consumer.
  - `main.rs:306` — `let (poll_manager, _pm_handles) = PollManager::new(...)`: the handles are bound to `_pm_handles` and never sent a single registration.
  - `ingestion/mod.rs:381-382,440-441` — ingestion hardcodes `TrainState::Active` / `Critical` / `Terminal` inline, bypassing the rule set.
- **Current state:** The authoritative transition rule set (`from_departure`/`emergency_promote`) and the whole `PollManager` registration channel are dead in production. No train is ever registered, so the spawned `PollManager::run()` loop sits forever on an empty heap and the global scheduler does nothing live. Real state transitions are emitted inline from the ingestion pipeline.
- **Decision / status:** **Cut** the dead rule engine + registration plumbing (`from_departure`, `emergency_promote`, `PollManagerHandles`/`RegistrationMsg`/`push_registration`, and `StateChangeEvent.reason`); keep the live inline logic in `ingestion/mod.rs` that actually drives transitions. **Status: pending.**
- **Audit:** `data/refactor-audit/core-types-statemachine.md` §2 (HIGH/MED rows) + §3 first bullet.

### A2. Association feature fix & wire (NP predecessor-delay signal is unreachable)

- **Refs:**
  - `ingestion/filter.rs:77` — taxonomy needle is lowercase `(b"association", MessageDecision::Drop)`; `contains_element` matches `<association`/`:association` only, never the real `<Association>`.
  - `ingestion/parser.rs:206,229,237` — the parser's `Association` arm and `ParsedUpdate::Association { prev_rid, next_rid }`, plus its own capital-`A` test fixture at `:477`.
  - `cache/train_registry.rs:227` — `record_association(prev_rid, next_rid)`, populating the turnround map.
  - `ingestion/mod.rs:275` — reads `predecessor_delay` from the registry into the volatility context.
  - `types/volatility.rs:61` (`predecessor_train_delay_mins`), `prediction/types.rs:64` (`predecessor_train_delay`), `prediction/engine.rs:203`, `prediction/onnx_engine.rs:62,161` — the downstream ML feature.
- **Current state:** Real Darwin frames are `<Association>` (capital A), but the filter-first taxonomy drops them on a lowercase needle before the parser that handles them ever runs. The entire predecessor-delay / turnround predictive signal — parser arm → `record_association` → volatility `predecessor_train_delay` → ONNX real-time feature — is therefore unreachable in the live pipeline. Only the synthetic lowercase test frame exercises the DROP branch.
- **Decision / status:** **Fix** the case-match (taxonomy → `Keep`/`Conditional` for capital `b"Association"` with route filtering), wire the path end-to-end, and add a capital-`A` Darwin frame fixture so a regression would actually be caught. **Status: pending.**
- **Audit:** `data/refactor-audit/networking-ingestion.md` §2 (first HIGH row) + §3 second bullet.

---

## B. Remaining dead code / no-value items

Verified by the audit (zero production callers, crate-wide grep) and not yet removed.
Each is marked **remove** or **keep** (intentional / Tier-C-staged).

| Item | Refs | State | Disposition |
|------|------|-------|-------------|
| `trains_at_tiploc` | `cache/train_registry.rs:290` | Test-only; only `cascade_trains_for_tiploc` is wired (into the currently-dead `check_tiploc_cascade`). ~22 lines of hot-path code, never invoked live. | **remove** (unless Tier-C cascade wiring is imminent). |
| `is_empty` | `cache/train_registry.rs:186` | Carries `#[allow(dead_code)]`; used only by tests (`:536`, `:542`). | **remove** or convert `#[allow(dead_code)]` → `#[cfg(test)]`. |
| `check_tiploc_cascade` + `cascade_trains_for_tiploc` | `ingestion/filter.rs:210`, `cache/train_registry.rs:323` | Tier-C-staged; no production caller. CLAUDE.md says wired "when Tier C is active". | **keep (Tier-C-staged)** — see §C2; gate behind a tracking issue rather than a free-floating TODO. |
| `StateChangeEvent.reason` | `state_machine/poll_manager.rs:67` | Write-only; no consumer reads it. | **remove** (part of A1). |
| `TrainStatus.cancellation_reason` | `types/train_status.rs:115,171` | Initialised `Stamped::new(None)`; never written from any source nor read. Dead data on the hot-path struct (and SSE payloads). | **remove** until a Darwin cancellation-reason source populates it. |
| `TrainId::headcode` constructor | `types/train_id.rs:60` | Constructor + its `InvalidHeadcodeLength/Format` errors have no production caller (only tests); `rid`/`uid` are used. Re-exported via `lib.rs`. | **keep (intentional API)** — Headcode variant is plausibly deliberate public surface; drop only if Darwin headcode ingestion is ruled out. |
| `Stamped::is_stale` | `types/train_status.rs:47` | Carries `#[allow(dead_code)]`; only tests call it. Per-field staleness was the stated reason `Stamped` exists, yet nothing in production checks it. | **keep (honest allow / unfinished feature)** — or wire into a consumer (e.g. `api/types.rs` "seconds since updated" computes age manually). |
| JSON `/stations/search` route + `StationResult.trains_today` | `api/mod.rs:205`, `api/handlers.rs:415`, `:408,:426` | The JSON route has no template consumer (only the htmx `/ui/stations/search` fragment is referenced); it duplicates the fragment's index-query + mapping. `trains_today` is hardcoded `0` at every site and the maud `@if result.trains_today > 0` branch is unreachable. | **remove** the JSON route + handler + dead field (or document as public API and dedup the shared `index.search → StationResult` mapping). |
| Journey self-join SQL living in `frontend/` | `frontend/search.rs:487`, `frontend/demo.rs:717` | The `timetable_calls` self-join "direct journey" query is duplicated across two render modules (search LIMIT-less, demo LIMIT 8). Raw SQL belongs in `db/` per the directory map, not frontend handlers. | **move** to a `db/static_data.rs` function (e.g. `direct_journeys(from, to, date, limit)`) and call from both. |
| `wait_for_shutdown` startup-tail duplication | `main.rs:817` | Near-complete copy of the normal startup tail (rebuilds `StationIndex`, assembles a second `AppState`, re-runs serve/shutdown) behind a 9-arg `#[allow(clippy::too_many_arguments)]`. Easy for the two `AppState` assemblies to drift. | **remove dup** — extract `assemble_app_state(...)` + `serve_api(app_state, listener, token)` shared by both paths. |

Additional low-risk masked-dead items the audit flags (collapsible into the same sweep,
detail in the audit files): `ParseError::Empty` (`parser.rs:108`, `#[allow(dead_code)]`,
never constructed), `GtfsTrip::trip_headsign` (`gtfs.rs:85`, parsed-and-discarded),
`MockGbrClient::set_error` (`gbr_client.rs:285`, unused test helper), `ENDPOINT_DEPARTURES`
(`gbr_client.rs:37`, no call site), the `extract_stops_txt` single-use wrapper
(`gtfs.rs:339`), `PredictionEngine::with_store` / `arc_store` (`engine.rs:130,141`, zero
callers), `db::operators::list_operators` (`operators.rs:14`, test-only, not on the
awaiting-UI list), and the stale noise `#[allow(unused_imports)]` on now-used re-exports
(`types/mod.rs:14`, `state_machine/mod.rs:10,12`, `cache/mod.rs:10`). All **remove**.

> Note: the audit's `[HIGH] RISK` on circuit-breaker error routing
> (`main.rs` string-matching `msg.contains("503")`) is **already fixed** — see commit
> `1809421 fix(networking): route circuit breaker on typed error kind, not message text`.

---

## C. Tier-C production-handoff readiness

Deferred until the project moves to production with company credentials and their historical
data. These are **not** bugs in the current (Tier A/B) system — they are the checklist for
flipping Tier C on.

### C1. Reconcile the GBR/RTT client contract before enabling Tier C

- **Refs:** `networking/gbr_client.rs:34-37` (consts / base URL), `:65` (`RttServiceResponse`), `:206` (run_date parse).
- **Issue:** `base_url` is `https://api.rtt.io/api`, but the client's doc table claims `x-apikey` header auth and `/v1/train/{rid}/status` + `/v1/station/{crs}/departures` paths. The real Realtime Trains (RTT) API uses **HTTP Basic** auth (username + token, not `x-apikey`), keys services by **UID + date** (`/json/service/{uid}/{yyyy}/{mm}/{dd}`, not RID), and has no `/v1/.../status` path. The response struct is even documented as keyed on `{uid}` while the const it uses is `{rid}`.
- **Action:** Before Tier C goes live, reconcile endpoint shape, auth scheme, and UID-vs-RID keying against the real RTT (or whatever company/GBR Retail) API — otherwise the client will 401/404. Also harden `run_date` parsing (`:206` currently `.unwrap_or_else(|_| Utc::now().date_naive())`, a silent data-corruption fallback on the per-poll hot path).
- **Audit:** `data/refactor-audit/networking-ingestion.md` §2 (DRIFT/RISK rows on `gbr_client.rs`).

### C2. Wire `check_tiploc_cascade` into the delay path when Tier C is active

- **Refs:** `ingestion/filter.rs:210` (`check_tiploc_cascade`), `cache/train_registry.rs:323` (`cascade_trains_for_tiploc`), `:290` (`trains_at_tiploc`).
- **Issue:** The knock-on-delay cascade detector and its registry helper are built and tested but have no production call site — staged for Tier C. Currently inert (see §B).
- **Action:** Wire into `ingestion/mod.rs` when Tier C is active; until then keep behind a tracking issue, not a free-floating TODO.

### C3. Operator league / drill-down render empty until GTFS populates `services.toc`

- **Refs:** `db/operators.rs:37` (`operator_league`, "Returns empty until `services.toc` is populated"), `:54,:57` (`LEFT JOIN operators o ON o.toc = s.toc`, `WHERE s.toc IS NOT NULL`); dashboard links at `frontend/dashboard.rs` (`/operators` panel + nav card link a route not yet registered).
- **Issue:** The operator analytics queries filter on `services.toc`; until the GTFS static ingest runs (CLI `ingest-static`, or Dev Console → Ingest panel), `toc` is null and these render empty. The dashboard also links to a not-yet-registered `/operators` page (Phase 3-5).
- **Action:** Run the GTFS ingest once on deploy to backfill `services.toc` + seed `operators` (small tables; `delay_history` untouched). Gate the `/operators` links behind the page landing, or ship a placeholder route.
- **Audit:** `data/refactor-audit/db-layer.md` (awaiting-UI section) + `data/refactor-audit/frontend.md` (`/operators` 404 finding).

---

## Where to read more

Full file:line detail and per-finding reasoning live in the read-only refactor audit:

- `data/refactor-audit/core-types-statemachine.md`
- `data/refactor-audit/networking-ingestion.md`
- `data/refactor-audit/cache-prediction-weather.md`
- `data/refactor-audit/db-layer.md`
- `data/refactor-audit/api-wiring.md`
- `data/refactor-audit/frontend.md`
