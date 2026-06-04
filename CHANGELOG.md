# Changelog

The current version and last worked on date should be noted at the top of this file below this line:

**version = "1.12.9" -- 04/06/2026**

---

## [1.12.9] — 2026-06-04
### Fixed (production-readiness pass — robustness)
- **Circuit breaker** now routes on a typed `GbrErrorKind` instead of `msg.contains("503")` — timeouts, transport failures, and GBR 500/502/504 finally trip it (previously only a literal "503" did, so a brownout would hammer upstream indefinitely).
- **`journey_handler`** no longer leaks raw sqlx error text to clients (logs server-side, returns a generic message).
- **Request-path DB queries** in `build_departure_board` now `tracing::warn!` on failure instead of silently serving an empty board.
- **`/report`** (4 heavy aggregations over `delay_history`) moved off the un-rate-limited infra router onto the rate-limited router.
- **`gbr_client` `run_date`** now rejects a malformed date instead of fabricating "today" (which silently corrupted departure times on the live path).
### Changed (refactor / cleanup)
- Reconciled the ONNX feature-count contract to the code (14 day / 22 rt) across the module doc, CLAUDE.md, and `LiveFeatures`; named the inference magic numbers.
- DB layer: lone `query!` macro → runtime `query_as`; consistent MAE cast and rolling-window bound; `DailyPoint.day`; doc accuracy.
- Removed dead code: unused `Config` fields (`gbr_api_*`, `darwin_*`), `PredictionEngine::with_store`/`arc_store`, `MockGbrClient::set_error`, `ENDPOINT_DEPARTURES`, stale `#[allow]`s, and the fictional "HFT flat-Vec" registry doc.
- Deduped frontend formatters into `components.rs`; collapsed `station_congestion`/`operator_cascade` into one helper.
- Unified the startup auto-ingest progress channel so `/ui/demo/ingest/stream` reflects it.
- Recorded all deferred refactors (state-machine cut, association fix), remaining dead code, and Tier-C readiness in `docs/tech-debt.md`.

## [1.12.8] — 2026-06-04
### Added
- Dashboard overhaul **Phase 3–5 data layer** (backend-only, fully `sqlx::test`-covered, no UI yet): the read/query foundations the operator, prediction-explorer, and station-explorer pages will render on top of. Built so they could be implemented and verified entirely without a browser.
  - `db/predictions.rs`: `prediction_snapshots` read path — `snapshots_for_rid`, `convergence_for_rid` (per-train predicted-vs-actual convergence trail), and `leadtime_accuracy` (mean absolute error bucketed by minutes-before-departure).
  - `db/operators.rs`: per-operator drill-down — `operator_detail`, `operator_daily_series` (punctuality trend), `operator_delay_distribution`, `operator_routes` (O–D pairs ranked by avg delay). All via the query-time `delay_history ⋈ services` JOIN on `uid` — no writes to the large tables.
  - `db/analytics.rs` (new): prediction-accuracy analytics — `calibration_curve`, `confidence_error`, `error_distribution` (signed-error bias histogram), `accuracy_over_time`. No day-ahead/real-time split (the outcomes ledger has no model discriminator).
  - `db/stations.rs` (new): per-station explorer — `station_summary`, `station_heatmap` (delay by weekday×hour), `station_busiest_services`.
  - 23 new `sqlx::test` DB tests. Clippy cleaned tree-wide (dead import, collapsible-if let-chains, complex-type alias).

## [1.12.7] — 2026-06-03
### Changed
- Dashboard overhaul **Phase 2 (Overview cockpit)**: `/` rebuilt into a live Signal-Terminal cockpit — KPI strip (on-time %, avg delay, prediction MAE, trains tracked) with sparklines + trend arrows, re-scopable by the global time-range picker (24h/7d/30d/all via htmx); a live network-state panel (tracked/on-time/delayed/cancelled + worst current delays) from the registry; a mini operator league (top 8 by on-time %, empty until the GTFS operator ingest runs); and a data-coverage footer (stations, real & synthetic record counts). New read helpers `db/overview.rs` and `operator_league`, plus `TrainRegistry::network_summary`. Emoji nav icons replaced with inline SVG.

## [1.12.6] — 2026-06-03
### Added
- Dashboard overhaul **Phase 1 (Operator data plumbing)**: GTFS `agency.txt`/`routes.txt` parsing derives a per-UID operator (`toc`) written onto the small `services` table, plus an `operators` reference table (friendly name from the feed + curated brand colour). Operator grouping on all 6.7M+ historic `delay_history` / `prediction_outcomes` rows is unlocked via a query-time JOIN on `uid` — **no rewrite of the large tables**. Re-run the GTFS ingest to populate `toc` ("backfill").

## [1.12.5] — 2026-06-03
### Added
- Dashboard overhaul **Phase 0 (Foundation)**: "Signal Terminal" design system — IBM Plex Mono/Sans typography, an amber terminal accent with green/amber/red reserved for punctuality semantics, vendored uPlot, `frontend/charts.rs` inline-SVG helpers (KPI cards, sparklines, trend arrows, bar cells), an upgraded nav with active states + a Dashboard link, a global time-range picker, and a KPI strip on the dashboard proving the system.

## v1.12.4 — 31/05/2026 — Prediction feature logging (schema + Rust wiring)

- **`migrations/20240417120013_prediction_features.sql`**: adds `features JSONB` column to
  `prediction_outcomes`; creates `prediction_snapshots` table (one row per significant prediction
  event per RID, indexed by rid and snapshotted_at).
- **`RailPredict/src/prediction/onnx_engine.rs`**: `predict_day_ahead` and `predict_realtime` now
  return `Option<(i32, JsonValue)>` — the predicted delay plus a named feature vector as JSON.
  `DAY_FEATURE_NAMES` and `RT_EXTRA_NAMES` constants added; `feats_to_json` helper zips names onto values.
- **`RailPredict/src/types/volatility.rs`**: `VolatilityContext.prediction_features: Option<JsonValue>`
  added; populated by `engine.rs` on every ML prediction; cleared on statistical fallback.
- **`RailPredict/src/prediction/engine.rs`**: handles `(i32, JsonValue)` from ONNX; stores features
  in `status.volatility.prediction_features`.
- **`RailPredict/src/db/predictions.rs`**: `insert_first_prediction` binds `features` column;
  new `insert_snapshot(db, rid, uid, predicted_delay_mins, features)` function.
- **`RailPredict/src/ingestion/mod.rs`**: `insert_snapshot` called alongside
  `insert_first_prediction` in the same `tokio::spawn` block for every initial ML prediction.

## v1.12.3 — 30/05/2026 — v7.5-HC models activated (3k trees, operator features)

- **`scripts/compare_models.py` v7.5**: two new interaction features — `weekday_operator_enc`
  (encodes weekday × UID-prefix pair; captures operator-specific day-of-week patterns) and
  `operator_relative_delay` (rolling_mean_7d minus operator's training-set baseline; ranks
  feature #7 in day-ahead importance). Day-ahead grows from 12 → 14 features; real-time from
  20 → 22 features. `feature_meta.json` now includes `weekday_operator` and `operator_mean_delay` maps.
- **HC model training**: high-convergence variant with 3k trees, lr=0.015, num_leaves=255,
  min_child_samples=100. best_iter=3000 on both models (hitting budget; models are at data's
  noise floor). Day-ahead MAE 13.94 → 13.47 min (−0.47); real-time MAE 4.06 → 3.97 min (−0.09).
- **`models/day_ahead.onnx` + `models/realtime.onnx`**: promoted to HC versions (58MB each,
  up from 14MB standard). Previous 14MB standard models remain as `*_hc.onnx` fallbacks.
- **`RailPredict/src/prediction/onnx_engine.rs`**: updated feature vector (14/22 features),
  added `weekday_operator_map` and `operator_mean_delay` maps with graceful fallback for
  older `feature_meta.json` files. `+6` bias correction retained on day-ahead (bias −5.69 min).
- **`scripts/generate_synthetic.py`**: fixed distribution parameters so on-time targets are
  mathematically correct — good days N(−5,3)→95.3% on-time; average days 80% bimodal pool
  → 70.8% on-time. Fixed `_load_pool()` to not SELECT non-existent columns from delay_history.
- **`RailPredict/src/frontend/dashboard.rs`**: split "Delay records" metric into "Real delay
  records" and "Synthetic records" (separate DB queries via `tokio::try_join!`).

## v1.12.0 — 29/05/2026 — ML model v3: sample weighting, stratified split, fixed features

- **`scripts/compare_models.py` v3**: random 15% stratified test split; equal-tier sample
  weighting (on-time/slight/moderate/severe each contribute 25% of gradient weight); fixed
  `preceding_delay_mins` (was hardcoded 0, now merge_asof — 65% populated, top-8 RT feature);
  fixed `mins_until_departure` (was hardcoded 0, now 3rd most important RT feature); updated
  hyperparameters (n_estimators=1500, num_leaves=127, min_child_samples=100).
- **Model results**: day-ahead MAE 17.59 → 13.57 min (−4.0); real-time MAE 5.92 → 4.02 min
  (−1.9); real-time ±10m accuracy 90.7%; near-zero bias (−0.14 min).
- **`RailPredict/src/main.rs`**: heartbeat recorder task — subscribes to state-change broadcast,
  calls `record_outcome` on every poll tick for Active/Critical trains so on-time trains generate
  training records even when Darwin sends no TS message.
- **`scripts/fetch_hsp_history.py`**: complete rewrite — route-based O-D pair approach (fixes
  `from_loc == to_loc` API restriction); correct `rid` field name in serviceDetails POST; correct
  `days` value derived from date weekday; extracts delay rows for all stops in each service;
  DB-backed progress tracking (no flat files); silent by default (`--verbose`/`--dry-run` flags).
- **`scripts/run_hsp_fetch.sh`**: stripped log-file plumbing; progress via DB query.

## v1.11.0 — 26/05/2026 — Dashboard redesign + in-memory station index + seeds

Merges the `ui/dashboard-redesign` branch. Three independent threads:

**Dashboard redesign — `src/frontend/dashboard.rs` + `static/style.css`**
- Plain status page replaced with a two-column hero, 7-card metrics grid
  (including ML accuracy cards — median AE, % within ±5 min, bias —
  with green-tinted borders when prediction data is present), and a
  3-card navigation section.
- Adds `PredStats` query via `tokio::join!` to avoid sequential DB
  round-trips on dashboard load.

**In-memory station index — `src/cache/station_index.rs`**
- New module: word-prefix tree built once at startup from the `stations`
  table. Replaces the SQL full-text query (`to_tsvector`) for
  `/ui/stations/search` autocomplete.
- Wired into `AppState` via `main.rs`; `handlers::station_search_handler`
  and `search::station_suggestions_fragment` now query the index instead
  of the DB. Cold lookups drop from ~12 ms (DB round-trip) to sub-ms.

**Registry fix — `src/cache/train_registry.rs`**
- `departure_snapshot` now takes both CRS and TIPLOC so Darwin entries
  stored under a TIPLOC (e.g. `WATRLMN`) match queries for the CRS
  (e.g. `WAT`). Fixes empty departure boards for stations where Darwin
  uses TIPLOC keys.
- `rid` field stringification no longer prepends `RID:` (was breaking
  `/trains/<id>/view` with 404s).

**Seed scripts + docs**
- `scripts/seed_stations.py` — OSM-based station seed (~120 TIPLOCs).
- `scripts/seed_history.py` — synthetic delay history (~1.7M rows over
  90 days; per-hour, per-station, weekend multipliers).
- `Makefile` — `make seed-stations`, `make seed-history` targets.
- `docs/model-performance.md` — ML model evaluation: 5.3 min MAE,
  77% within ±5 min on 1,485 real UK trains (25 May 2026 Darwin feed).

**Infra reshuffle**
- `deploy/prometheus.yml` (moved from root)
- `deploy/docker-compose.prod.yml` (moved from root)
- `docs/improvements.md` (moved from `TODOs/Improvements.md`; old dir removed)
- `docker-compose.yml` — volume mount paths updated for the deploy/ move.
- `logs/.gitkeep` — ensures directory survives clones.
- `.gitignore` — broaden `**/.DS_Store`; `/logs/*` + `!logs/.gitkeep`.

`prediction_outcomes` (v1.10.0) is untouched — the dashboard's ML metrics
read from `delay_history` (which feeds the model), while the detail page
and `/demo/predictions` continue to surface the per-train ledger.

---

## v1.10.0 — 26/05/2026 — Per-train prediction tracking + dev/ML console panel

Adds a per-RID predicted-vs-actual ledger and surfaces it across the developer
console and the public train detail page. Layered on top of the v1.9.0 ONNX
engine — whatever the engine produces (ONNX real-time, ONNX day-ahead, or the
statistical fallback) is captured at first sighting, then compared to the
actual outcome when each train deactivates.

The pattern-aggregated `delay_history` table that feeds the model is unchanged.
This is a separate, per-train-instance ledger answering "what did we predict
for *this specific train* and what actually happened?"

**New table — `prediction_outcomes`**
- `migrations/20240417120010_create_prediction_outcomes.sql` — keyed on `rid`,
  carries the first prediction we made (`predicted_delay_mins`,
  `prediction_confidence`, correlation signal), then `final_delay_mins` /
  `finalised_at` are filled in on deactivation. Indexes on `predicted_at DESC`
  (recent feed), partial on `finalised_at` (recent outcomes), and on `uid`.

**`src/db/predictions.rs`**
- `insert_first_prediction(db, &TrainStatus)` — `ON CONFLICT DO NOTHING` keeps
  the first prediction we made, so the eventual comparison is fair.
- `finalise_outcome(db, rid, final_delay_mins)` — guarded by `finalised_at IS NULL`.
- `recent_predictions(db, limit)` and `prediction_for_rid(db, rid)` for reads.
- `accuracy_summary(db, window_hours)` — rolling 24h MAE + mean predicted +
  mean actual + finalised count for the dev panel cards.
- `PredictionOutcome::abs_error_mins()` helper.

**Ingestion write path**
- `PipelineContext` gains `db: Option<Db>` and `persisted_predictions:
  Arc<DashSet<String>>`. `None` keeps tests / the `passthrough` constructor
  database-free.
- On every TS message: after `engine.predict_and_update`, if the registry
  now holds a `Some(predicted)` and the RID isn't in the dedup set, spawn a
  fire-and-forget `insert_first_prediction`. Closure is sync, DB call runs in
  a detached task — never blocks the registry lock.
- On Deactivated: snapshot `reported_delay_mins.value` before removing the
  registry entry, then spawn `finalise_outcome`. RID is also dropped from
  the dedup set so memory stays bounded.

**DTO surface — `src/api/types.rs`**
- `TrainSummary` and `LiveUpdateEvent` gain `prediction_confidence: Option<f32>`
  alongside the `predicted_delay_mins` field added in v1.9.0. Both use
  `#[serde(default)]` so older clients don't break.
- `train_handler`, JSON `live_handler`, and the HTML SSE `ui_live_handler`
  populate the new field from `TrainStatus.volatility.historical_reliability`.

**Train detail page — `src/frontend/detail.rs`**
- New `prediction_card` section between the header and live updates: shows
  the live engine prediction (with High/Medium/Low confidence label), the
  persisted "First prediction" snapshot from `prediction_outcomes`, and
  once finalised the actual outcome with absolute error.
- Correlation footer surfaces the preceding-RID signal when one is present.

**Dev console — `src/frontend/demo.rs`**
- New full-width "Predicted vs Actual" section above the existing grid.
- `/ui/demo/predictions` fragment: 5 summary cards (24h MAE, mean predicted,
  mean actual, finalised count, recent rows) + a 30-row live ledger table
  with In flight / Finalised status pills. Auto-refreshes every 10s.
  Complements the public `/predictions` analytics page added in v1.9.0:
  `/predictions` is for visitors; `/demo/predictions` is for operators.

**CSS**
- `.prediction-card`, `.prediction-grid`, `.prediction-cell`, `.prediction-value`,
  `.prediction-sub`, `.prediction-correlation`, `.prediction-inline-chip`, `.dim`.
- `.demo-pred-table-wrap`, `.demo-pred-table` for the dev panel ledger.

201 unit tests passing, clippy clean. No API breaks; new DTO fields are
optional with `#[serde(default)]`.

---

## v1.9.0 — 25/05/2026 — ML delay prediction (LightGBM → ONNX → Rust inference)

Two LightGBM models trained from `delay_history`, exported as ONNX, and loaded at
startup for in-process inference via `ort` (ONNX Runtime). Prediction engine tries
ONNX first, falls back to statistical trimmed-mean.

**Python training pipeline — `scripts/`**
- `scripts/train_models.py` — loads all `delay_history` from Postgres, engineers
  rolling 7-day features per service pattern (no data leakage), trains two
  `LGBMRegressor` models, exports `day_ahead.onnx` (10 features) and
  `realtime.onnx` (15 features) via `onnxmltools.convert_lightgbm`.
  Day-ahead MAE: 12.0 min vs 27.5 min trimmed-mean baseline (56% improvement).
  Real-time MAE: 3.8 min with live Darwin delay signal.
- `scripts/requirements.txt` — pinned deps for `lightgbm`, `scikit-learn`,
  `onnxmltools`, `skl2onnx`, `sqlalchemy`, `python-dotenv`.
- `Makefile` — `make train` creates/reuses `scripts/.venv`, installs deps, runs training.
- `.gitignore` — `models/*.onnx`, `models/feature_meta.json`, `scripts/.venv/`.

**Rust inference — `src/prediction/onnx_engine.rs`**
- `OnnxEngine` struct: two optional `Mutex<Session>` (day-ahead and real-time),
  `crs_map` and `uid_prefix_map` loaded from `models/feature_meta.json`.
- `load()` silently skips missing model files; logs INFO on load, WARN if absent.
- `predict_day_ahead()` — 10-feature vector, clamps output to `[-120, 600]`.
- `predict_realtime()` — extends day-ahead vector with 5 live Darwin/weather features.
- Sessions wrapped in `Mutex` because `Session::run` requires `&mut self`.

**New types — `src/prediction/types.rs`**
- `RollingStats` — `mean_delay`, `std_delay`, `on_time_pct`, `sample_count_log`.
- `LiveFeatures` — `current_delay_mins`, `preceding_delay_mins`, `wind_mph`,
  `volatility_score`, `mins_until_departure`.
- `HistoricalStore::rolling_stats_7d()` — computes 7-day rolling window from
  in-memory store; returns zero-filled `RollingStats` when no data (cold start safe).

**Prediction engine wiring — `src/prediction/engine.rs`**
- `PredictionEngine` gains `onnx: Arc<OnnxEngine>` field.
- `with_store_and_onnx()` constructor; `with_store()` defaults to empty `OnnxEngine`.
- `predict_and_update_with_correlation` tries real-time ONNX (when `reported_delay_mins`
  is known), then day-ahead ONNX, then statistical trimmed-mean fallback.

**Dependencies — `RailPredict/Cargo.toml`**
- `ort = "=2.0.0-rc.12"` with `features = ["ndarray"]` (statically linked ORT binary).
- `ndarray = "0.17"` (must match ort's transitive dependency version).

---

## v1.8.1 — 26/05/2026 — Export-site hardening

Tightens the v1.8.0 static export against script-tag injection, fixes a colour-class
bug for missing on-time data, and parallelises the four DB queries.

**`src/export/mod.rs`**
- `gather()` now runs `query_summary`, `query_daily`, `query_services`, `query_hourly`
  concurrently via `tokio::try_join!` instead of awaiting each one sequentially.
  Wall-time drops roughly 3–4× on the same connection pool.
- The serialised JSON has `</` escaped to `<\/` before substitution into the host
  `<script>` tag. `\/` is a valid JSON escape for `/`, so parsed values are unchanged,
  but a stray `</script>` in a string field can no longer terminate the tag.
- Collapsed a nested `if let … { if … }` into a single let-chain (clippy fix).

**`src/export/template.html`**
- Added an `esc()` helper that HTML-escapes `& < > " '`. The service-breakdown table
  now passes `s.uid` and `s.origin_crs` through it before interpolating into the
  row template — previously they were dropped into `innerHTML` raw.
- New `cardPctCls()` helper handles a null `on_time_pct` correctly. Previously
  `null >= 70` evaluated to `false`, so the summary card for a service with no
  on-time data rendered as red ("c-red") instead of unstyled.

No schema, no migration, no API change. 201 tests, all passing.

---

## v1.8.0 — 25/05/2026 — Static site export with predicted-vs-actual accuracy

New `export-site` CLI subcommand generates a self-contained HTML page showing the
last N days of delay history alongside prediction accuracy. Run `make export` to
write `docs/index.html`; deploy that file to any static host.

**Migration — `predicted_delay_mins` column**
- `migrations/20240417120009_add_predicted_delay_to_history.sql` — adds nullable
  `INTEGER` column `predicted_delay_mins` to `delay_history`. Existing rows carry
  `NULL`; new rows written after this version store the engine's prior prediction.

**Prediction engine — capture prediction at record time**
- `src/prediction/types.rs` — `DelayRecord` gains `predicted_delay_mins: Option<i32>`.
- `src/prediction/engine.rs` — `record_outcome` captures `status.predicted_delay_mins.value`
  before inserting the new observation. Because `record_outcome` is called before
  `predict_and_update` in the ingestion loop, this is the prediction the engine held
  just before seeing the actual delay — a true forecast, not a post-hoc one.

**DB layer — flush and load include new field**
- `src/db/history.rs` — `flush_history` includes `predicted_delay_mins` in the INSERT
  column list; `load_history` selects and reconstructs it.

**Export module — `src/export/`**
- `src/export/mod.rs` — four async query functions (summary, daily, service breakdown,
  hourly); serialises to `ExportData` JSON; substitutes into the HTML template; writes
  the output file, creating parent directories as needed.
- `src/export/template.html` — self-contained dark-themed page: six summary cards,
  dual-axis daily accuracy chart (observations + mean abs error + mean actual),
  hour-of-day bar chart coloured by severity, service breakdown table (top 100 by
  observation count with on-time % badge and prediction coverage %).

**CLI + Makefile**
- `src/cli.rs` — `Commands::ExportSite { output, days }` subcommand; defaults to
  `docs/index.html` and 7-day window.
- `Makefile` — `make export` target; override window with `make export DAYS=14`.

175 tests, all passing.

---

## v1.7.0 — 20/04/2026 — Product Features epic complete

All five items from `TODOs/AgentA.md`, `TODOs/AgentB.md`, and `TODOs/AgentC.md` complete. The system now has journey search, fare display, weather-driven volatility, push notifications, and a partitioned delay history table.

**5.1 — Journey search (A→B)**
- `src/api/handlers.rs` — `JourneyQuery` struct, `journey_handler` with self-join on `timetable_calls` (keyed on `uid + operating_date`; no `trip_id` column exists). Returns ordered services calling both origin and destination.
- `src/api/mod.rs` — `/journeys` and `/ui/journeys` routes added to `build_api_router`.
- `src/frontend/search.rs` — tab-toggle UI (Departures / Journey); `journeys_fragment`; shared autocomplete dropdowns parameterised by `crs_input_id`/`q_input_id`.

**5.3 — Fare display on train detail page**
- `src/frontend/detail.rs` — snapshot tuple extended to capture `destination_crs`; `cheapest_fare` called with `NaiveDate`; `pence_to_pounds` formatter; `.fare-chip` span rendered when fare is available.

**5.4 — Weather-driven volatility promotions**
- `src/weather/mod.rs` (new) — `VolatilityStore`, `WeatherAnchor`, `fetch_wind_mph` via Open-Meteo, `run_weather_task` (10-min interval). `WEATHER_ANCHORS` env var.
- `src/config.rs` — `weather_anchors: Vec<(String, f64, f64)>`, `parse_weather_anchors` public parser with 3 unit tests.
- `src/main.rs` — weather task spawned conditionally; wind >50mph sets `volatility.wind_speed_mph` + `incident_flagged` after each GBR poll.

**5.5 — Push notifications on Critical promotions**
- `src/main.rs` — ntfy push task spawned when `NOTIFICATIONS_ENABLED=true` and `NTFY_URL` set. Fires on `new_state==Critical && old_state!=Critical`. Dedup guard prevents repeat fires.
- `src/config.rs` — `ntfy_url: Option<String>`, `notifications_enabled: bool`.

**8.2 — `delay_history` range partitioning**
- `migrations/20240419_partition_delay_history.sql` (new) — renames existing table to `_legacy`, creates quarterly RANGE partitions covering 2024 Q1 – 2026 Q2, copies data. DROP of legacy table intentionally commented out for manual verification.

175 tests, all passing.

---

## v1.6.0 — 18/04/2026 — Tier C Wiring epic complete

All five items from `TODOs/TierCWiring.md` complete. The networking layer is now fully wired into the runtime — the first time live GBR data flows from the polling path into the registry.

**2.1 — Poll consumer task**
- `src/main.rs` — spawns a single `tokio` poll consumer task that subscribes to the state broadcast channel, filters Active/Critical trains, calls GBR via `Coalescer` + `RateLimiter` + `CircuitBreaker`, and writes results back to the registry. Conditional on `GBR_API_KEY` being present in the environment.

**2.2 — GBR RTT JSON parsing**
- `src/networking/gbr_client.rs` — `RttServiceResponse` and `RttLocation` structs with `#[serde(rename_all = "camelCase")]`; `parse_rtt_time` converts `"HHMM"` string to `DateTime<Utc>`. Replaces stub `Err(NotFound)` with real deserialization; maps RTT fields to `TrainStatus`.

**2.6 — State machine transitions wired**
- Poll consumer re-evaluates `TrainState::from_departure` after each GBR write and emits a real `StateChangeEvent` if the state changed, re-queuing the train at the new interval.

**8.1 — Last-writer-wins race fix**
- Poll consumer compares the RTT response timestamp against `Stamped::last_updated` before applying any field update; GBR poll writes are rejected if the stored field is already newer (Darwin push is authoritative when more recent).

**4.5 — Remove all `#[allow(dead_code)]` suppressors**
- Removed from `networking/circuit_breaker.rs`, `networking/rate_limiter.rs`, `networking/gbr_client.rs`, `networking/mod.rs`, `ingestion/stomp_client.rs`, `state_machine/poll_manager.rs`. `MockStompClient::new()` moved to `#[cfg(test)]`.

172 tests, all passing.

---

## v1.5.0 — 18/04/2026 — Tier A Data Layer epic complete

All five items from `TODOs/TierADataLayer.md` complete. The static data pipeline is now end-to-end: GTFS is fully ingested, the departure board draws from the DB, and trains are pre-warmed into the registry at startup.

**2.3r — Departure board DB/registry merge**
- `src/api/handlers.rs` — `build_departure_board()` fetches today's timetable from the DB, overlays live registry data matched by scheduled departure time (±2 min), and batch-resolves destination names. Falls back to registry-only if no timetable is populated yet.

**2.4 — Station autocomplete**
- `src/api/handlers.rs` — `station_search_handler` uses `plainto_tsquery` full-text search on `known_stations`; returns empty list if query is under 2 characters.
- `src/frontend/search.rs` — htmx autocomplete pattern: visible text input with debounced `hx-get`; hidden CRS input populated on selection; `station_suggestions_fragment` renders `<ul>` dropdown.
- `src/api/mod.rs` — routes added for `/stations/search` and `/ui/stations/search`.

**2.5 — GTFS full ingest (trips + stop_times)**
- `src/ingestion/gtfs.rs` — `parse_trips`, `parse_calendar`, `parse_stop_times` pure functions added. Services keyed by UID with day-of-week bitmask from `calendar.txt`. Timetable calls upserted in 200-row chunks with a 7-day rolling window and `ON CONFLICT DO NOTHING`. Handles H≥24 overnight times; `stop_id_to_crs` extracts the last 3 uppercase chars from GTFS stop IDs.

**6.1 — Maintenance cleanup job**
- `src/db/maintenance.rs` (new) — `prune_old_rows` deletes `timetable_calls` older than 7 days and `delay_history` older than 91 days. `load_todays_calls` returns today's upcoming services for warm-up.
- `src/main.rs` — spawns a 24h prune task.

**6.2 — Proactive train warm-up at startup**
- `src/main.rs` — after `load_history`, calls `db::maintenance::load_todays_calls()`, constructs `(TrainId, TrainStatus)` pairs, and calls `registry.warm()` before spawning any tasks. Today's trains are in the registry before the first user request.

172 tests, all passing.

---

## v1.4.3 — 18/04/2026 — Darwin Push Port full end-to-end wiring

All fixes required to receive and parse live UK train data from the Darwin Push Port STOMP broker.

**rustls crypto provider**
- `src/main.rs` — `rustls::crypto::ring::default_provider().install_default()` called at process start; required by rustls 0.23 when multiple provider features are in the dependency tree (reqwest + sqlx both pull in rustls).
- `RailPredict/Cargo.toml` — `rustls` dependency given `features = ["ring"]` for deterministic provider selection.

**STOMP protocol compliance**
- `src/ingestion/stomp_client.rs` — CONNECT frame extended with `accept-version:1.0,1.1,1.2` and `host:` headers required by STOMP 1.2 (ActiveMQ rejects STOMP 1.0 CONNECT frames from credentialed clients).
- `src/ingestion/stomp_client.rs` — `writer` moved into the spawned reader task so the TCP write-half stays open; dropping `writer` at `subscribe()` return sent a TCP FIN that Darwin's ActiveMQ interpreted as a disconnect.
- `src/ingestion/stomp_client.rs` — body reading switched from `read_until(0, ...)` to exact-length `read_exact` using the `content-length` header; Darwin gzip payloads contain internal `\0` bytes that truncated `read_until` mid-body.
- `src/ingestion/stomp_client.rs` — STOMP ERROR frame now logged with `broker_message` and `broker_body` fields for actionable debugging.
- `.env` — `DARWIN_TLS=false` set; Darwin Push Port serves plain STOMP on port 61613 (no TLS wrapper).

**Gzip decompression**
- `RailPredict/Cargo.toml` — `flate2 = "1"` added.
- `src/ingestion/mod.rs` — `decompress_if_gzip()` helper detects `\x1f\x8b` magic bytes and decompresses with `GzDecoder` before any filter or parser step; Darwin Push Port v16 messages are always gzip-compressed.

176 tests (160 unit + 11 DB + 5 integration), all passing.

---

## v1.4.2 — 18/04/2026 — System dashboard and rate limiter fix

**System dashboard at /**
- `src/frontend/dashboard.rs` (new) — maud-rendered dashboard page showing DB status, Darwin feed status, active train count, station count, delay record count, navigation cards to Departures/Health/Metrics, and API reference table.
- `src/frontend/mod.rs` — `pub mod dashboard` added.
- `src/frontend/layout.rs` — "Departures" nav link added pointing to `/search`.
- `src/api/mod.rs` — `/` route moved to `infra_router` (no rate limiting); `/search` added to `build_api_router` as the new departures page route.
- `static/style.css` — dashboard CSS: `.dashboard`, `.status-grid`, `.status-card`, `.status-ok/.status-error`, `.nav-grid`, `.nav-card`, `.api-table`.

**Rate limiter ConnectInfo fix**
- `src/main.rs` — both `axum::serve(listener, app)` calls changed to `axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())`; `tower_governor`'s `PeerIpKeyExtractor` requires `ConnectInfo<SocketAddr>` to be present in the request extensions — without this every request fails with "Unable To Extract Key!".

---

## v1.4.1 — 18/04/2026 — Docker infrastructure fixes

**Dockerfile**
- Rust toolchain bumped `1.82-bookworm` → `1.88-bookworm` across all three build stages; `edition = "2024"` in `Cargo.toml` requires Rust ≥ 1.85, and cargo-chef 0.1.77 requires ≥ 1.88.
- `COPY RailPredict/static ./static` added to builder stage; `rust-embed` embeds static assets at compile time and panics if the folder is absent during `cargo build`.
- `COPY migrations /migrations` path corrected; `sqlx::migrate!("../migrations")` resolves relative to the crate root at `/app`, so migrations must be at `/migrations` (not `/app/migrations`).

**docker-compose.yml**
- `dns: [8.8.8.8, 8.8.4.4]` added to the `app` service; Docker's default resolver failed to resolve `darwin-dist-44ae45.nationalrail.co.uk` inside the container.
- DB host port mapping commented out to avoid conflict with a locally-running Postgres instance.

---

## v1.4.0 — 18/04/2026 — Technical Debt epic complete

Completes all actionable items of `TODOs/TechnicalDebt.md` (items 9.1–9.5, 7.1, 7.3). Item 4.5 (`#[allow(dead_code)]` removal) remains pending TierCWiring completion per its stated prereq.

**9.1 — `is_cancelled: Stamped<Option<bool>>`**
- `src/types/train_status.rs` — field changed from `Stamped<bool>` to `Stamped<Option<bool>>`; initialised as `None` (unknown) rather than `false` (falsely confirmed running).
- All callers updated: `api/handlers.rs`, `api/sse.rs`, `api/types.rs`, `frontend/detail.rs`, `frontend/search.rs`, `cache/train_registry.rs`, `ingestion/mod.rs`.

**9.2 — Doc comments on `best_delay_mins` / `best_platform`**
- `src/types/train_status.rs` — one-line doc comment above each method making the priority policy explicit.

**9.3 — STOMP body read replaced with `read_until`**
- `src/ingestion/stomp_client.rs` — byte-by-byte loop replaced with `AsyncBufReadExt::read_until(0, &mut body)`; single buffered syscall per message body at Darwin peak throughput.

**9.4 — Departure sort uses `DateTime<Utc>` not string**
- `src/api/handlers.rs` — `sort_by_key` now parses the RFC3339 string to `chrono::DateTime<Utc>` for ordering, eliminating the hidden UTC-only invariant.

**9.5 — CIF ingest exits cleanly instead of panicking**
- `src/main.rs` — `unimplemented!()` replaced with `anyhow::bail!`; clean non-zero exit with human-readable message.

**7.1 — `departure_snapshot` on `TrainRegistry`**
- `src/cache/train_registry.rs` — new `departure_snapshot(crs)` async method iterates the DashMap once, acquires read locks, filters, builds and returns `Vec<DepartureBoardEntry>` sorted by `DateTime<Utc>`.
- `src/api/handlers.rs` + `src/frontend/search.rs` — handlers simplified to a single registry call; 500 sequential lock acquisitions per page load eliminated.

**7.3 — DB pool size from env var**
- `src/db/mod.rs` — `connect()` now takes `max_connections: u32` parameter; hardcoded `10` removed.
- `src/main.rs` — `config.db_max_connections` passed through to both connect call sites.
- `.env.example` — `DB_MAX_CONNECTIONS=10` with sizing guidance comment.

160 tests, all passing.

---

## v1.3.0 — 17/04/2026 — CI & Developer Experience epic complete

Completes all five items of `TODOs/CI_DevEx.md` (excluding item 1.3 `.sqlx/` snapshot, which requires a user action against a live Postgres DB — see `TODOs/CI_DevEx.md` for instructions).

**4.4 — README tech stack table fix**
- Replaced `moka` with `dashmap`, removed `polars`, added `metrics` + `metrics-exporter-prometheus`, `tower_governor`. All rows now reflect actual `Cargo.toml` dependencies.

**4.1 — CI pipeline**
- `.github/workflows/ci.yml` (new) — push/PR triggers, `postgres:16-alpine` service with health check, steps: cargo-deny → rust toolchain (stable + clippy) → rust-cache → sqlx offline check → clippy -D warnings → cargo test → cargo build --release.

**4.2 — cargo-deny**
- `deny.toml` (new) — `version = 2`, advisories deny vulnerabilities/yanked, warn on unmaintained; license allow-list; `multiple-versions = "warn"`. Step wired into `ci.yml`.

**4.3 — DB integration tests**
- `RailPredict/tests/db_integration.rs` (new) — 11 `#[sqlx::test(migrations = "../migrations")]` functions covering `load_history`, `flush_history` (roundtrip + idempotent), `get_station` (missing + present), `departures_from` (empty / today / cross-date), `cheapest_fare` (empty / valid / future-validity).

**1.3 — `.sqlx/` offline snapshot — PENDING USER ACTION**
- The CI sqlx-check step will fail until the snapshot is committed. See `TODOs/CI_DevEx.md` for the one-time local generation command.

---

## v1.2.0 — 17/04/2026 — ProductionHardening epic complete

Completes all seven items of `TODOs/ProductionHardening.md`. The system is now safe to expose publicly: TLS is enforced on the Darwin connection, the ingestion pipeline auto-reconnects, public API endpoints are rate-limited and CRS-validated, the health check probes the DB, CORS is configurable, and secrets rotation is documented.

**1.2 — STOMP auto-reconnect**
- `src/ingestion/mod.rs` — `IngestionPipeline::run` returns `anyhow::Result<()>`; `PipelineContext` struct holds `Arc`-backed shared state (registry, broadcast tx, prediction engine, filter) so it survives reconnects.
- `src/main.rs` — Exponential backoff retry loop (2s → 120s cap) with `CancellationToken` select; WARN log per reconnect attempt including attempt count and retry delay.

**1.1 — STOMP TLS**
- `Cargo.toml` — `tokio-rustls = "0.26"`, `rustls = "0.23"`, `rustls-native-certs = "0.8"`.
- `src/ingestion/stomp_client.rs` — `StompConfig.tls: bool` from `DARWIN_TLS` env var (default `true`); `BoxReader` abstraction over plain/TLS streams; system CA certs via `rustls-native-certs`.
- `.env.example` — `DARWIN_TLS=true`.

**3.1 — CRS validation**
- `src/api/handlers.rs` — `fn validate_crs(crs: &str) -> Result<(), ApiError>`; exactly 3 ASCII letters; applied at entry of `departures_handler`.

**3.2 — Health endpoint DB probe**
- `src/api/handlers.rs` — `health_handler` runs `SELECT 1` with 1s timeout; returns 503 + `{"status":"degraded","detail":"db unreachable"}` on failure.
- `src/api/types.rs` — `HealthResponse` gains `detail: Option<&'static str>`.

**1.4 — CORS tightening**
- `src/config.rs` — `cors_allowed_origins: Option<Vec<String>>` from `CORS_ALLOWED_ORIGINS`; required in non-debug mode.
- `src/api/mod.rs` — `CorsLayer` built from allow-list; falls back to `permissive()` only in dev mode.

**1.5 — HTTP rate limiting**
- `Cargo.toml` — `tower_governor = { version = "0.4", features = ["axum"] }`.
- `src/api/mod.rs` — Public routes get `GovernorLayer`; `/health` and `/metrics` excluded via separate `infra_router`.
- `src/config.rs` — `http_rate_limit_per_sec: u64` from `HTTP_RATE_LIMIT_PER_SEC` (default 60; `0` disables).

**3.3 — SECURITY.md**
- `SECURITY.md` (new) — Rotation steps, verification, cadence for `GBR_API_KEY`, `DARWIN_PASSWORD`, `DB_PASSWORD`. CORS, rate limiting, TLS, and `.env` gitignore notes.

---

## v1.1.3 — 17/04/2026 — AdvancedAnalytics epic complete

Completes all four phases of `TODOs/AdvancedAnalytics.md`. Code is fully implemented; runtime effect of Phase 1 correlation depends on Improvements 2.1–2.2 (networking wired, GBR response parsed). Noted in `TODOs/AdvancedAnalytics.md`.

**Phase 1 — Preceding service correlation**
- `src/types/volatility.rs` — `CorrelationSignal { preceding_rid, preceding_delay_mins, weight }` struct; `correlation_signal: Option<CorrelationSignal>` field on `VolatilityContext`.
- `src/prediction/engine.rs` — `predict_and_update_with_correlation(status, registry_snapshot)` added alongside backward-compatible `predict_and_update`. Scans registry snapshot for services sharing `origin_crs` in `[departure - 20 mins, departure - 1 min]`; applies 0.6/0.4 weighted blend when preceding service has `reported_delay_mins > 5`. Weights defined as named constants.

**Phase 4 — Confidence decay for stale history**
- `src/prediction/engine.rs` — exponential decay `confidence * exp(-days_since/21.0)` applied when last `DelayRecord` is older than `STALENESS_THRESHOLD_DAYS = 21`. Raw and decayed confidence both surfaced in output.

**Phase 3 — Historical reliability per hour-of-day**
- `src/prediction/types.rs` — `departure_hour: u8` added to `ServicePattern`; splits history ring into 24 sub-patterns per service-day.
- `src/db/history.rs` — SQL partition, order, INSERT column list, and `ON CONFLICT` clause updated to include `departure_hour`.
- `migrations/20240417120005_add_departure_hour.sql` (new) — `ALTER TABLE delay_history ADD COLUMN IF NOT EXISTS departure_hour SMALLINT NOT NULL DEFAULT 0`; `CHECK (departure_hour BETWEEN 0 AND 23)`; drops and recreates both unique and pattern indices.

**Phase 2 — TIPLOC cascade (knock-on delay propagation)**
- `src/types/train_status.rs` — `calling_points: Vec<(String, DateTime<Utc>)>` added.
- `src/cache/train_registry.rs` — `tiploc_index: DashMap<String, Vec<TrainId>>` secondary field; `update_tiploc_index`, `trains_at_tiploc` (60-min window), `cascade_trains_for_tiploc` (±window_mins) methods; `remove()` and `evict_departed()` clean up stale TIPLOC entries.
- `src/ingestion/filter.rs` — `check_tiploc_cascade` async function (`CASCADE_DELAY_THRESHOLD_MINS = 5`, `CASCADE_WINDOW_MINS = 20`); TODO comment directs `ingestion/mod.rs` wiring.

**Observability — prediction accuracy metric**
- `src/prediction/engine.rs` — `metrics::histogram!("prediction_error_mins")` after predict_and_update where both predicted and reported delay are known.

158 unit tests + 5 integration tests, all passing.

---

## v1.1.2 — 17/04/2026 — Observability epic complete + Improvements 2.3 prereq

Completes all four phases of `TODOs/Observability.md`. Also completes `Improvements.md` item 2.3 (DB in AppState) as a prerequisite for the cache hit ratio metric. Item 2.3 is marked in `TODOs/Improvements.md`.

**Improvements 2.3 (prereq) — DB in AppState**
- `src/api/mod.rs` — `pub db: Db` and `pub prometheus: Arc<PrometheusHandle>` added to `AppState`.
- `src/main.rs` — `db: db_pool.clone()` wired into AppState construction.

**Phase 1 — Prometheus metrics endpoint**
- `Cargo.toml` — `metrics = "0.23"` and `metrics-exporter-prometheus = "0.15"` added.
- `src/main.rs` — `PrometheusBuilder::new().install_recorder()?` installed before any tasks spawn; handle stored in `Arc<PrometheusHandle>` passed into AppState.
- `src/api/mod.rs` — `GET /metrics` handler returns current scrape text; registered outside the CORS layer; gated by `METRICS_ENABLED` env var (default: enabled).

**Phase 2 — Core metrics instrumentation**
- `src/ingestion/mod.rs` — `darwin_messages_received_total`, `darwin_messages_dropped_total` (labelled by reason), `darwin_messages_applied_total`; cache hit ratio stubs commented with TODO pending Improvements 2.1–2.3.
- `src/networking/gbr_client.rs` — `gbr_api_latency_ms` histogram with `endpoint` and `status` labels; `rid`/`latency_ms`/`status_code` added as tracing span fields (Phase 4 log correlation).
- `src/networking/circuit_breaker.rs` — `circuit_breaker_state` gauge + `circuit_breaker_blocked_total` counter; `#[cold]` on `record_failure`.
- `src/main.rs` — `registry_train_count` gauge in 60s eviction task.
- `src/db/history.rs` — `db_flush_duration_ms` histogram + `db_flush_rows_inserted_total` counter.

**Phase 3 — Grafana + Prometheus in docker-compose**
- `docker-compose.yml` — `prometheus` (`prom/prometheus:latest`) and `grafana` (`grafana/grafana:latest`) services added under `--profile monitoring`; Grafana on port 3001; named volumes `prometheus_data` and `grafana_data`.
- `prometheus.yml` (new at repo root) — scrape config targeting `app:3000/metrics` every 15 s.

**Phase 4 — Structured log correlation**
- `src/ingestion/mod.rs` — `trace_id` from `Span::current().id()` on frame-processing spans.
- `src/networking/gbr_client.rs` — span fields enable Grafana spike → specific RID correlation in JSON logs.

---

## v1.1.1 — 17/04/2026 — FrontEndHardening epic complete

Completes all four phases of `TODOs/FrontEndHardening.md`.

**Phase 3 — SSE "Live Updates Paused" banner**
- `src/frontend/layout.rs` — existing hidden `#stale-banner` slot given `warning-banner` class and "⚠ Live updates paused — showing last known state" text; inline JS extended to listen to `htmx:sseError` (show banner + add `data-stale` to `#live-status`) and `htmx:sseOpen` (reverse on reconnect).
- `static/style.css` — `.warning-banner` (amber background, black text); `#live-status.data-stale` (40% grayscale + amber left border).

**Phase 1 — Stale data overlay**
- `src/api/types.rs` — `DepartureBoardEntry` gains `last_updated_secs_ago: Option<u64>` and `destination_name: Option<String>`.
- `src/frontend/search.rs` + `src/api/handlers.rs` — staleness computed from max `last_updated` across `actual_estimated_departure`, `reported_delay_mins`, `actual_platform`, `is_cancelled`; cards older than 120 s emit `data-stale="true"`.
- `static/style.css` — `.train-card[data-stale="true"]` gets 60% grayscale + 0.75 opacity + amber `"stale"` `::after` label.

**Phase 4 — Destination station on departure board cards**
- `src/types/train_status.rs` — `pub destination_crs: Option<String>` added; initialised to `None`.
- `src/ingestion/parser.rs` — Location handler overwrites `destination_crs` with each `tpl` so after all locations are processed it holds the final (destination) CRS.
- `src/frontend/search.rs` — `span .train-destination { "→ " (dest) }` rendered on card. DB name lookup stubbed (renders raw CRS) pending `AppState::db` wiring.

**Phase 2 — Optimistic departure board**
- `src/frontend/search.rs` — form trigger changed from `hx-trigger="submit"` to `hx-trigger="submit, every 30s"` with `hx-swap="innerHTML transition:true"`; board stays populated between refreshes.

---

## v1.1.0 — 17/04/2026 — Docker & Database epic complete (Phases 4–6)

Completes all six phases of `TODOs/Docker.md`. That file is now destroyed.

**Phase 4 — DB module**
- `src/db/mod.rs` — `connect(database_url)` creates a `PgPool` (max 10 connections) and runs `sqlx::migrate!` on startup. Exposes `type Db = PgPool`.
- `src/db/static_data.rs` — `Station`, `TimetableCall`, `Fare` structs with `sqlx::FromRow`; `get_station()`, `departures_from()`, `cheapest_fare()` async query functions.
- `src/db/history.rs` — `load_history(db)` uses a window function to rebuild `HistoricalStore` from the most recent MAX_SAMPLES rows per pattern on startup; `flush_history(db, store)` batch-inserts all records with `ON CONFLICT DO NOTHING` in 500-row chunks for idempotency.
- `PredictionEngine::with_store(Arc<HistoricalStore>)` constructor + `arc_store()` accessor added to support startup loading and shared flush.
- `main.rs` wired: DB connect → load history → `PredictionEngine::with_store` → 60s flush task → final flush on SIGTERM/Ctrl-C via `shutdown_signal()` (listens for both `SIGTERM` and Ctrl-C on Unix).

**Phase 5 — CLI + GTFS ingest**
- `src/cli.rs` — `Cli` struct (clap derive); `Commands::IngestStatic { source, url, file }` subcommand; `IngestSource` enum (`Gtfs`, `Cif`).
- `src/ingestion/gtfs.rs` — `GtfsStation` (serde-deserialised from `stops.txt`); `parse_stops(csv_bytes)` pure function (testable without I/O) filters to valid 3-letter uppercase CRS codes; `run_ingest(db, url)` downloads ZIP → extracts `stops.txt` → parses → upserts stations via `QueryBuilder`; `run_ingest_from_file` variant for local archives. CIF is a stub returning `unimplemented!()`.
- 5 unit tests for `parse_stops` and `is_valid_crs`.
- `main.rs` parses CLI args first; if a subcommand is present it runs and exits before starting the server.

**Phase 6 — Production hardening**
- `docker-compose.prod.yml` — added `read_only: true` on the `app` service; all writable state lives in the DB.
- `src/db/mod.rs` documents pool sizing (max 10 connections) inline.
- 150 tests total (145 unit + 5 integration), all passing.

---

## v1.0.1 — 17/04/2026 — Docker infrastructure + database foundations (Phases 1–3)

Partial progress on `TODOs/Docker.md` (Phases 1, 2, and 3 complete; Phases 4–6 pending).
Epic remains open — `TODOs/Docker.md` is not destroyed until all six phases are done.

**Phase 1 — Dockerfile**
- Multi-stage `Dockerfile` using `cargo-chef`: `planner` → `cacher` → `builder` → `runtime`. Dependency compilation is a cached layer; code-only rebuilds reuse it entirely.
- `SQLX_OFFLINE=true` set in the builder stage; `.sqlx/` snapshot directory to be committed after first `cargo sqlx prepare` run locally.
- Runtime stage: `debian:bookworm-slim` with `ca-certificates` + `libssl3`; non-root `railpredict` user; `HEALTHCHECK` wired to `GET /health`.
- `.dockerignore` excludes `target/`, `data/`, `.env`, `*.md`, `.git/`.

**Phase 2 — Docker Compose**
- `docker-compose.yml`: `db` (Postgres 16-alpine, named volume, healthcheck), `app` (waits on DB healthcheck, env from `.env`), `ingest` (profile-gated, exits after completion), `pgadmin` (profile-gated dev tool on `localhost:5050`).
- `docker-compose.prod.yml`: overrides `app` to pull a tagged registry image (`RAILPREDICT_IMAGE`), removes host-side DB port exposure, sets `stop_grace_period: 30s`, forces `LOG_FORMAT=json`.
- `.env.example` committed with every variable stubbed and commented.
- `.gitignore` confirmed to exclude `.env`.

**Phase 3 — Migrations**
- `migrations/` directory at repo root; five SQL files managed by `sqlx-migrate`.
- `20240417120000_create_stations` — `CHAR(3)` CRS primary key, GIN full-text index on name.
- `20240417120001_create_services` — UID + days bitmask (`SMALLINT`, 0–127), FKs to stations.
- `20240417120002_create_timetable_calls` — per (uid, date, location) call; indexed for departure-board and train-detail queries.
- `20240417120003_create_fares` — price in pence (no float), validity windows, composite PK.
- `20240417120004_create_delay_history` — Tier B persistence; unique composite index on `(uid, weekday, origin_crs, recorded_at)` prevents duplicate flush writes; no FK to services (intentional).

**Cargo.toml additions**
- `sqlx = "0.8"` with `runtime-tokio-rustls`, `postgres`, `migrate`, `chrono` features.
- `clap = "4"` with `derive` feature (for the ingest CLI subcommand in Phase 5).
- Project compiles cleanly with both new dependencies.

---

## v1.0.0 — 17/04/2026 — Unit test coverage pass

Added tests for all modules that were missing coverage. No logic changes.

- **`api/types.rs`** — 9 new tests: `ApiError` factory methods, HTTP status mapping (`NOT_FOUND`→404, `BAD_REQUEST`→400, `INTERNAL_ERROR`→500), JSON round-trips for `TrainSummary`, `DepartureBoardEntry`, `LiveUpdateEvent`.
- **`prediction/types.rs`** — 5 new tests: direct `HistoricalStore` coverage (unknown pattern returns `None`, insert/get round-trip, cap enforcement, oldest-eviction correctness, distinct patterns tracked independently).
- **`state_machine/train_state.rs`** — 9 new tests: boundary values at exactly 120/30/5 mins and one below each, `IncidentDetected` emergency promote for all base states, `Display` for all five variants. Boundary tests pin `now` to avoid sub-millisecond drift.
- **`ingestion/filter.rs`** — 7 new tests: SF/trainAlert/association/alarm classification, `forget` resets sequence guard, Drop message blocked on watched route, watched-route filter with no CRS.
- **`types/train_status.rs`** — 3 new tests: `Stamped::is_stale()` both directions, `Stamped::new` timestamp, `best_platform()` with both fields absent.
- **`ingestion/parser.rs`** — 5 new tests: multiple TS in one Pport, `at` attribute, missing `rid`/`ssd` → error, mixed TS+deactivated in one message.
- **`cache/train_registry.rs`** — 7 new tests: `update()` returns false for unregistered, `snapshot_all()`, empty snapshot, `warm()` empty iterator, `is_empty()`, eviction using `actual_estimated_departure`.
- **`types/train_id.rs`** — 7 new tests: headcode format rejections, equality within/across variants, `as_str` for all variants, UID display.
- 139 unit tests + 5 integration tests, all passing.

---

## v0.9.0 — 17/04/2026 — Epic 8: Tier B Prediction Engine

Completed all items in `TODOs/PredictionEngine.md`. Destroyed that file on completion.

- **`src/prediction/types.rs`** — `ServicePattern` struct keyed on `(uid, weekday, origin_crs)` — the stable recurring-service identity, not the daily-changing RID; `DelayRecord` (`delay_mins: i32, recorded_at: DateTime<Utc>`); `HistoricalStore` (`DashMap<ServicePattern, VecDeque<DelayRecord>>`, hard-capped at `MAX_SAMPLES = 90` per pattern — ~13 weeks of daily observations)
- **`src/prediction/engine.rs`** — `PredictionEngine` wraps `Arc<HistoricalStore>`, derives `Clone` for cheap task-boundary passing; `predict_and_update(&self, status: &mut TrainStatus)` derives the `ServicePattern`, computes trimmed-mean delay (drop top/bottom 10%) and confidence (`samples / MAX_SAMPLES`), writes `predicted_delay_mins` and `volatility.historical_reliability` — does nothing if fewer than 3 samples exist (no fabrication rule); `record_outcome()` appends a confirmed Darwin delay back into the store for incremental learning
- **`TrainStatus.uid`** — new field; populated from the first Darwin `TS` message carrying a `uid` attribute; `parser.rs` extracts it and propagates through `TsUpdate`
- **`ingestion/mod.rs`** — wires `PredictionEngine` into the pipeline: `record_outcome` then `predict_and_update` called on every TS update after `reported_delay_mins` is set; `PredictionEngine` passed into `IngestionPipeline` at construction time
- **`main.rs`** — `PredictionEngine::new()` constructed once at startup; `Arc` clone passed to `IngestionPipeline`
- 7 new unit tests (`predict_with_no_history_returns_none`, `predict_with_uniform_history_returns_that_value`, `trimmed_mean_ignores_outliers`, `record_outcome_caps_at_max_samples`, `confidence_scales_with_sample_count`, and two wiring tests); 90 tests total (85 unit + 5 integration), all passing

---

## v0.8.0 — 17/04/2026 — Epic 7: HTTP API + Rust/maud/htmx Frontend

Completed all items in `TODOs/UI.md`. Destroyed that file on completion.

**Sub-Epic A — axum HTTP API**

- **`src/api/types.rs`** — client-facing DTOs: `TrainSummary`, `DepartureBoardEntry`, `LiveUpdateEvent` (flat, serde-serialisable, no internal type leakage); `ApiError` with consistent `{ "error": "...", "code": "..." }` JSON envelope implementing `IntoResponse`; `HealthResponse` serving `CARGO_PKG_VERSION` at compile time
- **`GET /stations/{crs}/departures`** — Tier A: full registry snapshot filtered by origin CRS, sorted by scheduled departure, zero GBR calls
- **`GET /trains/{rid}`** — Tier B: registry lookup; `400` on invalid RID format, `404` if not found; `last_updated` derived from the most-recent field stamp on the `TrainStatus`
- **`GET /trains/{rid}/live`** — Tier C SSE: subscribes caller to the broadcast `state_change_tx`, filters for the requested RID, enriches each event with a live registry read, streams as `text/event-stream` JSON; 15s `KeepAlive` prevents proxy timeout; stream closes cleanly on `Terminal` state
- **`GET /health`** — always 200, returns version string
- `CorsLayer::permissive()` + `TraceLayer::new_for_http()` applied to all routes; API contract table documented in `src/api/mod.rs` header comment
- `AppState` carries `Arc<TrainRegistry>` and `broadcast::Sender<StateChangeEvent>`; passed as axum `State` extractor; no `Extension` indirection
- axum server spawned in `main.rs` topology alongside `PollManager`, `IngestionPipeline`, and eviction task; all share the `CancellationToken`

**Sub-Epic B — Rust/maud/htmx Frontend**

- **`src/frontend/layout.rs`** — base chrome: `<html>`, `<head>` (htmx CDN script tag, CSS link), `<body>` wrapper, stale-data banner slot (hidden by default, revealed by htmx SSE error handler)
- **`src/frontend/components.rs`** — `delay_badge(minutes, cancelled) -> Markup` (green / amber / red by value); `platform_chip(platform) -> Markup`
- **`src/frontend/search.rs`** — `/` full-page maud render with origin CRS `<input>` and `hx-get` departure board; `GET /ui/stations/departures?crs=XXX` htmx fragment handler returning `<ul>` of `DepartureBoardEntry` rows
- **`src/frontend/detail.rs`** — `/trains/:rid/view` full-page maud render; train summary card, SVG route diagram shell; `hx-ext="sse" sse-connect="/trains/:rid/live"` wired on the live section; `GET /ui/trains/:rid/live` SSE HTML fragment handler for htmx OOB swaps
- **`static/style.css`** — dark-first palette (`#1a1a1a` background, `#00c853` rail-green accent, amber/red for delay states); embedded at compile time via `rust-embed` — binary has zero filesystem dependency at runtime
- Page routes (`/`, `/trains/:rid/view`) and fragment routes (`/ui/...`) merged into the same axum `Router` as the JSON API routes; `rust-embed` static handler serves `/static/:path`

---

## v0.7.0 — 17/04/2026 — Epic 6: Runtime Wiring & Observability

Completed all items in `TODOs/RuntimeWiring.md`. Destroyed that file on completion.

- Added `tracing`, `tracing-subscriber` (env-filter + json), `tokio-util`, `dotenvy` to `Cargo.toml`
- Added `[lib]` target (`railpredict`) alongside binary so `tests/` can import modules directly
- **`src/config.rs`** — `Config::from_env()` collects all missing required vars into a single `ConfigError`; `Config::for_testing()` for zero-env unit tests; full env var inventory documented in table
- **`src/lib.rs`** — re-exports all modules as the public library surface
- **`src/main.rs`** — `#[tokio::main]`, `dotenvy::dotenv()`, fail-fast config load, `init_tracing()` (pretty/JSON from `LOG_FORMAT`), shared `CancellationToken`, spawns `PollManager`, `IngestionPipeline`, eviction task (60s tick); graceful `ctrl_c` drain with `tokio::join!`; starts cleanly without Darwin credentials (logs a warning and waits for shutdown)
- **Tracing throughout ingestion** — replaced `eprintln!` placeholder with structured `tracing::info/warn/error/debug/trace` calls; `rid` field carried on key events
- **Bug fix**: first TS message for an unregistered train silently dropped platform/departure fields — restructured `process_frame` to register then always apply, regardless of whether the entry pre-existed
- **`tests/integration.rs`** — 5 integration tests against the real public wiring: smoke (T1→T2→T3→deactivated), ascending-TS registry state, stale reconnect replay, graceful shutdown, cancellation event propagation
- 83 tests total (78 unit + 5 integration), all passing

---

## v0.6.0 — 17/04/2026 — Epic 5: In-Memory Cache

Completed all items in the Epic 5 cache section of `TODO.md`.

- `src/cache/train_registry.rs` — `TrainRegistry` backed by `dashmap::DashMap<TrainId, Arc<RwLock<TrainStatus>>>` for sharded concurrent access
- `update()` closure API applies mutations without replacing the full entry
- `evict_departed()` removes trains beyond a 5-minute post-departure buffer window
- `warm()` loads a batch of Tier A static statuses at startup
- 7 unit tests covering upsert, get, update, remove, eviction, and warm-up

---

## v0.5.0 — 17/04/2026 — Epic 4: Data Ingestion

Completed all items in `TODOs/DataIngestion.md`. Destroyed that file on completion.

- Added `quick-xml = "0.37"` and `dashmap = "6"` to `Cargo.toml`
- **STOMP client** (`stomp_client.rs`): `StompClient` trait + `LiveStompClient` (raw STOMP framing over `tokio::net::TcpStream`, no dormant crate dependency) + `MockStompClient` for tests; `StompConfig` reads from env vars
- **Filter** (`filter.rs`): full Darwin message taxonomy (`TS`=KEEP, `deactivated`=KEEP, `schedule`/`SF`=CONDITIONAL, `OW`/`trainAlert`/`association`/`alarm`=DROP); region/route CRS filter; `SequenceGuard` per-TrainId timestamp check prevents stale overwrites and handles STOMP reconnect replays
- **Parser** (`parser.rs`): `quick-xml` event-based parser (chosen over `serde-xml-rs` for namespace robustness and zero-copy streaming); handles `TS` and `deactivated` messages; extracts RID, platform, estimated/actual departure, cancellation flag
- **Pipeline** (`mod.rs`): `IngestionPipeline` wires STOMP → filter (pre-parse) → sequence guard → parser → registry write; emergency promotions (cancellation, major delay) broadcast on Epic 2 `mpsc` channel — no direct cross-module function call; `INGESTION_BUFFER=512` with documented rationale
- 74 unit tests total (31 new), all passing

---

## v0.4.0 — 17/04/2026 — Epic 3: Networking Layer

Completed all items in `TODOs/Networking.md`. Destroyed that file on completion.

- Added `reqwest` (rustls-tls, json) and `async-trait` to `Cargo.toml`
- `GbrClient` trait — testable interface; `LiveGbrClient` reads `GBR_API_KEY` from env, never hardcodes credentials; `MockGbrClient` for unit tests
- GBR endpoint constants documented in `gbr_client.rs`; JSON response parsing deferred pending Darwin credentials
- `RateLimiter` — token-bucket; `MAX_REQUESTS_PER_SECOND=10`, `BURST_CAPACITY=20` with source rationale documented
- `CircuitBreaker` — `Closed/Open/HalfOpen` state machine; `FAILURE_THRESHOLD=3`, `COOL_DOWN_SECS=30`; transitions fully documented
- `Coalescer` — `oneshot` fan-out; single HTTP call for N concurrent callers on same `TrainId`; lock held only briefly, never across await; key design decision documented
- 43 unit tests total (15 new), all passing; coalescer test verifies exactly 1 HTTP call for 5 concurrent requests

---

## v0.3.0 — 17/04/2026 — Epic 2: State Machine

Completed all six items in `TODOs/StateMachine.md`. Destroyed that file on completion.

- Added `tokio` (features = ["full"]) to `Cargo.toml`
- `TrainState` enum: `Dormant`, `Monitored`, `Active`, `Critical`, `Terminal` — full transition diagram documented in source
- Time-based `from_departure()` rule set: single authoritative function covering all promotion, demotion, and terminal cases
- `PromotionReason` enum: `TimeBased`, `VolatilityTriggered`, `IncidentDetected` (last arm deferred to Epic 4)
- `emergency_promote()` for volatility/incident bypass of time thresholds
- `PollEntry` min-heap type with correct `Ord` impl for `BinaryHeap`
- `PollManager` with `tokio::select!` loop racing sleep against registration channel
- Bounded `mpsc` channels: `STATE_CHANGE_BUFFER=256`, `REGISTRATION_BUFFER=64` with rationale documented
- `StateChangeEvent` broadcast on each poll fire
- 28 unit tests total (11 new), all passing

---

## v0.2.0 — 17/04/2026 — Epic 1: Core Data Types

Completed all six items in `TODOs/Structs.md`. Destroyed that file on completion.

- Created `src/types/` module with ownership model documented in `mod.rs`
- `TrainId` enum: `Rid` (15-char), `Uid` (6-char), `Headcode` (digit-letter-digit-digit) with validated constructors and `thiserror` error types
- `TrainStatus` struct: single source of truth; `Stamped<T>` wrapper provides per-field `last_updated` timestamps for stale-data detection
- Timestamp fields: `scheduled_departure`, `public_departure`, `actual_estimated_departure` (all `chrono::DateTime<Utc>`)
- `UpdateSource` provenance enum: `RestPoll`, `StompFirehose`, `PredictionEngine`
- `VolatilityContext` struct: `wind_speed_mph`, `historical_reliability`, `incident_flagged` — all optional pending live feed integration
- 17 unit tests, all passing
