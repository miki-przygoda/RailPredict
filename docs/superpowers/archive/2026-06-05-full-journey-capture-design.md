# Full-Journey Capture — Design Spec

> **Status:** Approved 2026-06-05. Build mode: LIVE (no-rebuild lifted).
> **Background:** `docs/darwin-data-audit.md` (what we drop), `docs/reason-code-feature.md` (reason prototype).

**Goal:** Stop distilling each Darwin service to a single origin-delay integer. Parse every
`<Location>` and the `schedule` message, accumulate the whole journey in the registry while a
train is live, and snapshot a rich record to disk on deactivation — producing a queryable
feature matrix for regression/relationship analysis *and* richer dashboards.

**Approach (chosen):** Breadth-first ("Scope A") — capture everything we already receive,
persist a fat record, derive relationships later. **Hybrid schema:** a wide `journeys` header
(one row per service) + a `journey_calls` child (one row per stop). The new tables are
**purely additive** — `delay_history` remains the live predictor's untouched hot path.

---

## Scope

**In (this feature):**
- Parse the `schedule` (SC) message → `toc`, train category, full ordered calling pattern.
- Parse every TS `<Location>` → per-stop `<arr>`/`<dep>` (et/at/delayed), `plat` +
  `platsup/platsrc/conf`, per-stop `can`.
- Parse TS `<LateReason>` / `<CancelReason>` code (+ tiploc), classified Structural/Exogenous.
- Accumulate a per-call journey on `TrainStatus` (merge across partial TS messages).
- Persist `journeys` + `journey_calls` on deactivation; compute cheap per-journey rollups.

**Out (deferred, noted for later):**
- `fL` (per-coach loading / crowding) and `OW` (NRCC incident text) — currently *dropped at the
  filter*, noisier, need filter changes. Out of this pass.
- `SF` (formation) — out of first pass; can be a fast-follow once the journey skeleton exists.
- Official Darwin reason-code reference — start with the prototype's Structural/Exogenous map;
  swap the full reference in as a follow-up.
- Wiring any of the new signals into the live ML feature vector — additive dataset only for now.

---

## Phasing

Each phase produces working, shippable software.

- **Phase 1 — Capture (this plan):** schema + parser + accumulation + persistence. On rebuild,
  data accrues immediately. No UI.
- **Phase 2 — First win:** light up `/operators` with real `toc` from `journeys`.
- **Phase 3 — Surface:** arrival-delay + "why late" chip on detail; per-train trajectory
  (reuse the convergence chart); reliability + recovery on overview.

---

## Data model (Phase 1)

Two new migrations. weekday: 0=Mon..6=Sun (matches `delay_history`). No FK to `services`
(a service may be retired while its history stays valid) — consistent with existing tables.

### `journeys` — one row per finalised RID

| Column | Type | Source |
|---|---|---|
| `rid` | CHAR(15) PK | TS |
| `uid` | VARCHAR(8) NOT NULL | TS |
| `ssd` | DATE NOT NULL | TS |
| `weekday` | SMALLINT NOT NULL CHECK 0..6 | derived from `scheduled_departure` |
| `departure_hour` | SMALLINT NOT NULL CHECK 0..23 | derived |
| `toc` | CHAR(2) | schedule |
| `train_category` | VARCHAR(8) | schedule |
| `origin_tpl` | VARCHAR(8) NOT NULL | first call |
| `destination_tpl` | VARCHAR(8) | last call |
| `scheduled_departure` | TIMESTAMPTZ NOT NULL | origin call |
| `actual_departure` | TIMESTAMPTZ | origin call |
| `origin_delay_mins` | INTEGER | origin call |
| `arrival_delay_mins` | INTEGER | **destination call (NEW)** |
| `late_reason_code` | SMALLINT | TS LateReason |
| `cancel_reason_code` | SMALLINT | TS CancelReason |
| `reason_tiploc` | VARCHAR(8) | TS reason |
| `reason_class` | SMALLINT | derived (0=unknown,1=structural,2=exogenous) |
| `was_cancelled` | BOOLEAN NOT NULL DEFAULT false | TS `can` |
| `partial_cancel` | BOOLEAN NOT NULL DEFAULT false | any per-stop `can` |
| `n_calls` | SMALLINT | count |
| `max_delay_mins` | INTEGER | rollup over calls |
| `min_delay_mins` | INTEGER | rollup over calls |
| `recovered_mins` | INTEGER | `max_delay_mins - arrival_delay_mins` |
| `origin_platform` | VARCHAR(4) | origin call |
| `platform_confirmed` | BOOLEAN | origin call conf |
| `wind_mph` | REAL | volatility context |
| `finalised_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | — |

Indexes: PK(`rid`); `(uid)`; `(toc)`; `(weekday, departure_hour)`; `(finalised_at DESC)`.

### `journey_calls` — one row per stop

| Column | Type |
|---|---|
| `id` | BIGSERIAL PK |
| `rid` | CHAR(15) NOT NULL |
| `seq` | SMALLINT NOT NULL |
| `tpl` | VARCHAR(8) NOT NULL |
| `sched_arr` / `actual_arr` / `arr_delay_mins` | TIMESTAMPTZ / TIMESTAMPTZ / INTEGER |
| `sched_dep` / `actual_dep` / `dep_delay_mins` | TIMESTAMPTZ / TIMESTAMPTZ / INTEGER |
| `platform` | VARCHAR(4) |
| `plat_confirmed` | BOOLEAN |
| `is_cancelled` | BOOLEAN NOT NULL DEFAULT false |
| `dwell_secs` | INTEGER (`actual_dep - actual_arr`) |

Constraints/indexes: `UNIQUE(rid, seq)`; index `(rid)`; index `(tpl)` for cross-service
"where is delay incurred" queries.

---

## Ingestion changes

### Parser (`ingestion/parser.rs`)

- New variant `ParsedUpdate::Schedule(ScheduleUpdate)` where
  `ScheduleUpdate = { rid: TrainId, uid: Option<String>, ssd: NaiveDate, toc: Option<String>,
  train_category: Option<String>, calls: Vec<PlannedCall> }` and
  `PlannedCall = { tpl, seq, ptd, pta, wtd, wta, activity }`.
  - **Confirm the filter passes `schedule`** (audit: Conditional). If not, add it to the
    Conditional set in `ingestion/filter.rs`.
- `TsUpdate` grows: `calls: Vec<CallUpdate>` (every `<Location>`, in document order, each with
  `tpl, pta, ptd, arr_et, arr_at, dep_et, dep_at, delayed, plat, plat_confirmed, can`),
  `late_reason: Option<ReasonRef>`, `cancel_reason: Option<ReasonRef>` where
  `ReasonRef = { code: i32, tiploc: Option<String> }`.
  - Keep the existing scalar fields (`scheduled_departure`, `estimated_departure`, etc.) so the
    live predictor path is unchanged — `calls` is *additional*.
- `plat_confirmed`: true when `platsup="true"` or `conf="true"` (best-effort from the attrs we
  see). Read `platsup`/`platsrc`/`conf` off the `<plat>` element.

### Reason classification

- Port `ReasonClass { Structural, Exogenous }` + the illustrative `reason_info` map from
  `tests/reason_code_prototype.rs` into a real module `prediction/reason.rs` (or
  `ingestion/reason.rs`). `reason_class` stored as SMALLINT (0 unknown / 1 structural /
  2 exogenous). Marked clearly as illustrative pending the official Darwin reference.

### Accumulation (`types/train_status.rs`)

- Add to `TrainStatus`: `toc: Option<String>`, `train_category: Option<String>`,
  `journey: std::collections::BTreeMap<u16, CallObservation>` (replacing the dead
  `calling_points`; update the TIPLOC-index callers if any rely on `calling_points`).
- `CallObservation = { tpl, seq, sched_arr, actual_arr, sched_dep, actual_dep, platform,
  plat_confirmed, is_cancelled }` — all `Option` except tpl/seq/is_cancelled.
- **Merge semantics:** a method `apply_call(&mut self, c: CallUpdate, seq_hint)` upserts by seq,
  filling newest non-null actuals (never nulling an existing actual). TS messages are partial —
  a message may carry only one stop's forecast.
- **Seq assignment:** the `schedule` message establishes canonical `tpl→seq`. If a TS arrives
  before any schedule, assign seq by first-seen document order; reconcile to schedule seq if a
  schedule arrives later (match on tpl).

### Wiring (`ingestion/mod.rs::process_frame`)

- Handle `ParsedUpdate::Schedule` → `registry.update(rid, |s| { s.toc = ...; s.train_category =
  ...; seed s.journey with planned calls })`. Register the train if not present (mirrors the TS
  registration path).
- In the TS branch, after the existing live-field application, fold `ts_update.calls` into
  `status.journey` via `apply_call`, and stash `late_reason`/`cancel_reason` on the status (new
  optional fields, or onto the journey header struct at finalisation).
- In the **deactivation branch**, before `registry.remove`, read the accumulated journey under
  the existing read-lock, build a `JourneyRecord` + `Vec<JourneyCallRecord>`, and fire-and-forget
  `tokio::spawn` a `db::journeys::insert_journey(...)` — same pattern as `finalise_outcome` /
  `record_cancellation`. Compute rollups (max/min/recovered, arrival_delay, n_calls) at build time.

### Persistence (`db/journeys.rs` — new)

- `insert_journey(db, &JourneyRecord, &[JourneyCallRecord])`: one INSERT into `journeys`
  (`ON CONFLICT (rid) DO NOTHING`) + one batched multi-row INSERT into `journey_calls`
  (`ON CONFLICT (rid, seq) DO NOTHING`). Idempotent, mirrors `flush_history`.
- Add `pub mod journeys;` to `db/mod.rs`.

---

## Derived signals

- **Cheap per-journey rollups** (`max/min/recovered_mins`, `arrival_delay_mins`, `n_calls`):
  computed at finalisation, stored on the header.
- **Cross-journey analytics** (reliability = mean/std per pattern, reason-conditioned recovery,
  per-segment delay hotspots, dwell anomalies): **query-time** aggregates feeding dashboards in
  Phase 3 — no precompute, no new ingestion cost.

---

## Risks / non-functional

- **Memory:** per-call `BTreeMap` per active train (tens of entries). Bounded — accrues only while
  live, dropped on eviction. Add a gauge for journey-map size if cheap.
- **Hot-path cost:** parsing every `<Location>` is more work per TS but quick-xml is streaming;
  persistence is off-path (spawned). The live predictor still reads the same scalar fields.
- **Partial messages:** merge-not-replace is mandatory; covered by tests.
- **Backward compat:** `calling_points` removal — audit `train_registry.rs` for users
  (`update_tiploc_index`) and migrate them to `journey` (or keep a thin compatibility accessor).

---

## Testing

- Parser: `schedule` arm (toc + ordered calls); multi-Location TS (per-stop arr/dep/plat/can);
  reason-code extraction (element + attribute forms); partial-cancel.
- Accumulation: merge across two partial TS messages keeps both stops; later actual doesn't get
  nulled by a later partial; seq reconciliation when schedule arrives after TS.
- Persistence: `insert_journey` idempotent; rollups correct (recovered/arrival_delay/max/min).
- Integration: feed schedule + TS×N + deactivated through `process_frame`; assert a `journeys`
  row + N `journey_calls` rows with correct arrival delay.

---

## Versioning

New epic → **minor bump to `1.15.0`** on Phase-1 completion (working capture+persist end to end),
per the 5-file protocol (`CLAUDE.md`, `README.md`, `TODO.md`, `CHANGELOG.md`, `Cargo.toml`).
Phases 2–3 get their own bumps.
