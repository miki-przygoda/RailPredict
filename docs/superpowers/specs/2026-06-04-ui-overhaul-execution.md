# RailPredict UI Overhaul — Execution Spec

**Date:** 2026-06-04
**Branch:** `ui/page-improvements`
**Status:** Approved direction, pending spec review
**Builds on:** `docs/superpowers/specs/2026-06-03-dashboard-overhaul-design.md` (the "Signal Terminal" design system — the source of truth for tokens, type, charts, motion, accessibility). This spec does **not** re-decide the design system; it sequences the execution.
**Companion ledger:** `docs/superpowers/specs/2026-06-04-visual-changes-plan.md` (per-page visual notes; fold items in as pages are built).

---

## 1. Where we start (current reality)

The Signal Terminal **foundation already shipped**: design tokens, self-hosted IBM Plex Mono/Sans (Latin subset), vendored uPlot, `frontend/charts.rs` SVG helpers, and a nav with active-state highlighting. The **data layer is also built** — query modules exist for every target page (`db/overview.rs`, `db/operators.rs`, `db/stations.rs`, `db/analytics.rs`, `db/predictions.rs`, `db/cancellations.rs`).

What is **missing** is the surfacing: the operator and station *pages* don't exist (their `/operators` nav links currently 404), several pages need rebuilding/reskinning, and `/demo` still reads as a developer test bench rather than a product.

This spec covers that surfacing work — the visible overhaul — not new data plumbing.

## 2. Locked decisions

| Decision | Choice |
|---|---|
| Information architecture | **Option A — full analytics product.** Build the missing pages; retire the demo framing. |
| Top navigation (5 items) | `Overview` · `Operators` · `Predictions` · `Stations` · `Explore` |
| Departures (`/search`) | **Kept and reskinned, off the top nav.** Reachable from Overview and from each Station page. |
| Off-nav surfaces | `/dev` (stripped registry/health probes, for own debugging), `/report` (export), `/trains/:rid/view` (train detail) |
| `/demo` | **Retired** — split into product-shaped pieces (see §4). No simulated purchase presented as a product feature. |
| Workflow | **Mockup-first for new/rebuilt pages** (browser sign-off before code); **direct reskin** for the rest. |
| Design system | Inherit "Signal Terminal" unchanged (2026-06-03 spec). |

## 3. Final information architecture

| Route | Status | Becomes |
|---|---|---|
| `/` | rebuild | **Overview cockpit** — KPI strip, network-state panel, live-network widget, mini operator league, prediction-accuracy sparkline; global time-range picker re-scopes the page. |
| `/operators` | **new** | Operator **league table** — on-time %, avg delay, sample count, brand-coloured. |
| `/operators/:toc` | **new** | Operator **drill-down** — headline KPIs, punctuality trend, delay distribution, best/worst routes. |
| `/predictions` | rebuild | **Predicted-vs-actual explorer** — calibration curve, confidence-vs-error, error distribution, accuracy-over-time, lead-time accuracy. |
| `/stations/:crs` | **new** | Station **explorer** — summary KPIs, delay-by-hour×weekday heatmap, busiest services. |
| `/trains/:rid/view` | reskin + add | Reskin; add per-train **convergence chart** (`convergence_for_rid`). |
| `/search` | reskin | Departure board — Signal Terminal reskin, off-nav. |
| `/report` | reskin | Export view — light reskin, off-nav. |
| `/dev` | new (folded) | Stripped registry/health/event-monitor probes salvaged from `/demo`. Off-nav. |
| `/demo` | **remove** | Retired (see §4). |

## 4. Retiring `/demo`

`frontend/demo.rs` (~1350 lines) is the single biggest piece of demo framing. Disposition:

- **Live event monitor** → a compact **"Live network"** panel on the Overview cockpit.
- **Health / registry probes** → off-nav **`/dev`** (functional, unstyled-minimal; ops not marketing).
- **Simulated purchase flow** → replaced by a **"Ticketing — coming soon"** stub. Do not present a fake checkout as a feature. (Tier C stays stubbed per the unchanged architecture rule.)
- **Remove** Tier badges (`.badge-tier-a` "Tier B" hacks), "Feature Lab" wording, colour emoji (🔒), and dingbat status glyphs — replaced by the shared inline-SVG icon pattern.
- The `demo_*` fragment routes are removed or re-pointed at `/dev` equivalents as needed.

## 5. New / rebuilt pages and their backing data

Each of these gets a **browser mockup → sign-off → build**:

- **Overview `/`** — `overview::headline_metrics`, `daily_series`, `coverage_counts`; `predictions::accuracy_summary`; `operators::operator_league` (top N); registry network summary for the live panel.
- **Operators `/operators`** — `operators::operator_league`.
- **Operator `/operators/:toc`** — `operators::operator_detail`, `operator_daily_series`, `operator_delay_distribution`, `operator_routes`.
- **Predictions `/predictions`** — `analytics::calibration_curve`, `confidence_error`, `error_distribution`, `accuracy_over_time`; `predictions::leadtime_accuracy`.
- **Stations `/stations/:crs`** — `stations::station_summary`, `station_heatmap`, `station_busiest_services`.
- **Detail convergence** — `predictions::convergence_for_rid`.

Rendering uses the existing `charts.rs` SVG helpers (sparkline, column/bar, heatmap, histogram, calibration plot); uPlot reserved for the 2–3 genuinely interactive time-series only.

## 6. Direct reskins (no mockup)

Departures (`/search`), train detail, `/report`, the nav/active-state polish, and the **cross-page consistency sweep**: one `components::empty_state(msg)` / `loading(msg)` / `status_card(label, value, kind)` helper set, emoji→inline-SVG, and a single `→` arrow class (collapsing `.arrow`/`.es-arrow`/`.purchase-arrow`/`.ticket-arrow`).

## 7. Build order

0. **Foundation pass** — nav final shape (5 items, Departures off-nav), retire `/demo` → `/dev` + "coming soon" ticketing stub, global time-range picker shell. Small; removes the demo framing and (once Operators lands) the broken `/operators` links.
1. **Overview cockpit** (`/`) — establishes the cockpit pattern and exercises `charts.rs`.
2. **Operators** — `/operators` league + `/operators/:toc` drill-down (lands the dashboard operator links live).
3. **Predictions** explorer + detail convergence chart.
4. **Stations** explorer (`/stations/:crs`).
5. **Reskin sweep** — Departures, detail, report, consistency helpers.

## 8. Per-page workflow loop

For each step:
1. (New/rebuilt pages) Mock the layout in the visual companion → user signs off.
2. Build the page/handler + route + maud markup, reusing `charts.rs` and `components.rs`.
3. Every widget has an explicit **empty state** ("No data yet") — no blank frames.
4. `cargo test` + `cargo clippy` green.
5. Verify render at 1440 / 1024 / 768 / 375 px and with `prefers-reduced-motion`; confirm contrast on dark surfaces.
6. Version bump per protocol (patch per page; minor for the completed overhaul epic), update all five version files, note in `CHANGELOG.md`.
7. Move the corresponding visual-changes-plan items from **[PROPOSED]** to **[DONE]**.

## 9. Data caveat (designed around, not fixed here)

`/operators` and `/stations` read through `services.toc`. Until the operator backfill runs (GTFS `agency`/`routes` derivation, and/or loading the new `imports/rds_toc.csv` reference map), these pages show **correct empty / "Unknown operator" states** rather than mis-attributed data. The backfill is **out of scope** for this UI overhaul (it is data plumbing tracked in `docs/tech-debt.md` §C); the pages are built to light up when it runs.

## 10. Testing

- `cargo test` (integration + `sqlx::test` DB tests) stays green each step.
- Add unit tests for new chart-helper output (SVG structure / numeric scaling) where helpers are added.
- Add zero-dep HTTP e2e smoke for each new route (`/operators`, `/operators/:toc`, `/stations/:crs`) following the existing `tests/http_*` harness pattern — assert 200 + key markup, including the empty-state path.
- No widget renders a blank frame; every chart has a text/table fallback (accessibility).

## 11. Out of scope

- Operator/station data backfill (`services.toc`) — separate data plumbing.
- Tier C live GBR purchase wiring — unchanged, still stubbed.
- Light mode — dark-only by decision.
- ML model changes — we surface outputs, not alter models.

## 12. Risks

- **Empty pages read as broken.** Mitigation: deliberate, branded empty states with a one-line "why" ("Operator data populates after the timetable ingest"), not blank panels.
- **`/demo` removal breaks links/tests.** Mitigation: grep for `/demo` and `demo_*` references (nav, dashboard cards, tests) and re-point/remove in the same step.
- **Mockup churn.** Mitigation: keep mockups to layout/structure fidelity; settle each before building.
- **uPlot scope creep.** Keep it to the 2–3 interactive charts; everything else stays pure SVG.
