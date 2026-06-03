# RailPredict Dashboard Overhaul — Design Spec

**Date:** 2026-06-03
**Branch:** `ui/page-improvements`
**Status:** Approved direction, pending spec review
**Supersedes UI portions of:** current `frontend/` pages

---

## 1. Goal

Turn the RailPredict website from a thin set of functional pages into a **dense, numbers-first analytics dashboard** — a place where someone who loves data can explore UK rail delays, prediction accuracy (predicted vs actual), and per-operator punctuality on clean, instrument-grade pages.

The exploration that informed this spec lives in `data/dashboard-exploration/` (gitignored): five reports covering the current frontend, the data inventory, the prediction analytics surface, the operator-data gap, and dashboard design directions.

## 2. Locked Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Operator analytics | **Invest in real TOC data** | No real operator identity is stored today (`operator` = `uid[0]`, a coarse proxy). Real league tables need genuine TOC codes. |
| Visual character | **Dense terminal** (Bloomberg/Grafana) | Best fit for a numbers-loving audience; aligns with the existing SVG-chart + leaderboard patterns. |
| Delivery | **Phased** — one spec, reviewable phases | Large surface (charting infra + design system + ~8 pages + data work). |
| Charting | **SVG-first + vendored uPlot** | ~90% server-rendered inline SVG (zero JS, htmx-swappable); uPlot (~17KB) only for interactive zoom/pan time-series. |
| Brand accent | **Amber/orange terminal accent** | Frees green & red to mean *only* punctuality. Trading-terminal feel. |
| Typography | **IBM Plex Mono + IBM Plex Sans** | Distinctive technical family; Plex Mono for all numerics sells the terminal feel. |

## 3. Hard Constraints (non-negotiable)

- **Frontend stack:** `maud` (compile-checked Rust HTML) + `htmx` + minimal vanilla JS. **No JS framework, no build step.** uPlot is the only new JS dependency, vendored as a static file.
- **No live GBR calls in hot paths** (unchanged architecture rule).
- **Versioning protocol:** every completed TODO point bumps patch; each phase/epic bumps minor; update the version string in all five files (`CLAUDE.md`, `README.md`, `TODO.md`, `CHANGELOG.md`, `RailPredict/Cargo.toml`) simultaneously, and note completion in `CHANGELOG.md`.
- **Accessibility:** WCAG AA minimum — contrast ≥4.5:1 body / ≥3:1 large & UI glyphs, visible focus rings, color never the sole signal (always paired with text/icon), `prefers-reduced-motion` respected, charts have a text/table fallback.

## 4. Design System — "Signal Terminal"

### 4.1 Color tokens (CSS custom properties, dark-only)

| Token | Hex | Role |
|---|---|---|
| `--bg` | `#0a0e12` | Page base (near-black, OLED) |
| `--surface` | `#12181f` | Cards / panels |
| `--surface-2` | `#1a2129` | Raised / hover surfaces |
| `--border` | `#232c36` | Hairline dividers, table rules |
| `--text` | `#e6edf3` | Primary text (AAA on `--bg`) |
| `--text-dim` | `#9aa7b4` | Secondary text (≥3:1) |
| `--text-faint` | `#6b7785` | Labels, axis ticks |
| `--accent` | `#f5a623` | **Brand/interactive only** — links, focus, active nav, primary CTA, key highlights. Never used to encode data. |
| `--accent-press` | `#d98c12` | Accent active/pressed |
| `--ok` | `#34d399` | On-time (punctuality good) |
| `--warn` | `#f2c14e` | Minor delay — a distinct *yellow*, deliberately separated from the orange `--accent` |
| `--bad` | `#f04545` | Severe delay / cancelled |
| `--info` | `#4a9eff` | Neutral data series, non-judgemental metrics |

Brand orange (`--accent`) lives on **chrome/interaction**; the green→yellow→red ramp lives on **data**. They are never adjacent in the same role, which keeps the amber/yellow distinction unambiguous.

### 4.2 Typography

- **Headings + all numerics/data:** IBM Plex Mono (`tabular-nums` always on for figures).
- **Body / labels / prose:** IBM Plex Sans.
- Delivered via self-hosted woff2 embedded through `rust-embed` (preferred, no external dependency) or Google Fonts `@import` as a fallback. Self-hosting avoids a CDN round-trip and keeps the no-build promise.
- Type scale: `12 / 13 / 14 / 16 / 20 / 28 / 40` px. Weights: 600–700 headings, 500 labels, 400 body.

### 4.3 Layout & components

- **12-column tile grid**, dense by default; generous internal padding on the 8px rhythm (`8 / 16 / 24 / 32`).
- **KPI stat card:** big mono figure + label + delta arrow (▲/▼ via inline SVG, colored by direction) + optional sparkline.
- **League/data table:** zebra-free, hairline rules, `tabular-nums`, inline horizontal bars in cells, sortable headers with `aria-sort`.
- **Global time-range picker** (24h / 7d / 30d / all) that re-scopes every analytics page via htmx `hx-get` swaps.
- **Nav:** persistent top bar with the full page set and **active-state highlighting** (current page underlined in `--accent`) — fixes today's missing dashboard link / no active state.
- **Icons:** inline SVG (Lucide-style, single stroke width), replacing all emoji.

### 4.4 Motion

150–300ms ease-out on hover/focus/htmx swaps; one staggered page-load reveal (`animation-delay` per tile); animate `transform`/`opacity` only; `prefers-reduced-motion` disables non-essential motion. Data is readable immediately without animation.

### 4.5 Charting infrastructure (`frontend/charts.rs`)

Reusable maud helpers returning `Markup`, all server-rendered SVG:
- `kpi_card(label, value, delta, spark)`
- `sparkline(series)` / `column_chart(series, opts)` / `bar_chart` (horizontal, for league tables)
- `heatmap(grid)` (e.g. delay by hour×weekday)
- `histogram(buckets)` (delay distribution)
- `calibration_plot(points)` (predicted vs actual)
- `trend_arrow(delta)` / `league_row(...)`

uPlot is reserved for the 2–3 genuinely interactive time-series (zoom/pan accuracy-over-time, operator trend). It is loaded only on pages that need it.

## 5. Information Architecture

### Rebuilt / expanded (existing routes)
| Route | Becomes |
|---|---|
| `/` | **Overview cockpit** — KPI strip, network-state panel (counts by state, live worst delays, cancellations), mini operator league, prediction-accuracy sparkline; all re-scopable by the global time-range picker. Absorbs the explorer's "Trends" + "Model-health" ideas as panels rather than separate pages. |
| `/predictions` | **Predictions-vs-Actual explorer** — calibration curve, confidence-vs-error, day-ahead vs real-time split, accuracy-over-time, and lead-time convergence (reads the currently-unused `prediction_snapshots` table). |
| `/trains/:rid/view` | Reskin + per-train convergence chart from `prediction_snapshots`. |
| `/search`, `/demo`, `/report` | Reskinned into Signal Terminal; functionally unchanged. |

### New routes
| Route | Purpose |
|---|---|
| `/operators` | Operator **league table** — avg delay, on-time %, PPM, % cancelled, best/worst routes, prediction MAE per operator. |
| `/operators/:toc` | Operator **drill-down** — its routes/stations, punctuality trend, delay distribution, prediction accuracy. |
| `/stations/:crs` | Station/route **explorer** — delay-by-hour heatmap, busiest services, reliability, predicted vs actual. |

YAGNI: no standalone "Trends" or "Data/Model-health" pages — folded into the overview + the global time-range control.

## 6. Data Work (prerequisite for operator analytics)

The headline operator feature is blocked until real TOC identity exists. Required changes (Phase 1):

1. **Ingest operator identity** — parse GTFS `agency.txt` + `routes.txt` + `route_id` in `ingestion/gtfs.rs` to derive a per-UID TOC code (and, where available, parse the Darwin `toc` attribute on schedule messages). Document exactly which source is authoritative.
2. **Schema** — add a `services.toc` column via a new sqlx migration; index it.
3. **Backfill** — populate `toc` for historic rows by joining on `uid`, retroactively labelling the ~4.7M `delay_history` rows and the `prediction_outcomes` ledger (both keyed by/joinable on `uid`).
4. **Reference data** — a small `toc → friendly name + brand colour` map (sourced from the public RDG/ATOC operator list), held as static reference data.
5. (Stretch) Persist cancellations (currently parsed but not stored) so "% cancelled" is real rather than estimated.

Also unblock the prediction explorer: add a **read path for `prediction_snapshots`** (`db/predictions.rs`) for lead-time/convergence views.

## 7. Phasing

Each phase is independently reviewable, ends green (tests + clippy), and bumps the version per protocol.

- **Phase 0 — Foundation.** `frontend/charts.rs` reusable SVG helpers; Signal Terminal design-system pass on `layout.rs` / `components.rs` / `style.css` (tokens, IBM Plex fonts, nav with active states, emoji→SVG icons, global time-range picker shell); vendor uPlot as a static asset. No new pages yet — proves the system on the existing dashboard.
- **Phase 1 — Operator data plumbing.** GTFS/Darwin TOC ingestion, `services.toc` migration + backfill, TOC reference map, `prediction_snapshots` read path. Kicked off early so backfill is ready before the UI needs it.
- **Phase 2 — Overview cockpit.** Rebuild `/` into the metrics cockpit.
- **Phase 3 — Operators.** `/operators` league + `/operators/:toc` drill-down.
- **Phase 4 — Predictions-vs-Actual explorer.** Rebuild `/predictions`; add per-train convergence to the detail page.
- **Phase 5 — Stations/routes explorer + reskin** of `/search`, `/demo`, `/report`, `/trains/:rid/view`.

## 8. Testing

- Rust: existing `cargo test` (integration + `sqlx::test` DB tests) stays green each phase; add unit tests for chart-helper output (SVG structure / numeric scaling) and for the TOC-derivation logic and backfill query.
- Manual: verify each page renders under the `run`/`verify` skills at 1440 / 1024 / 768 / 375 px, with `prefers-reduced-motion`, and confirm contrast on dark surfaces.
- No chart renders a blank frame: every widget has an explicit empty state ("No data yet").

## 9. Out of Scope (this spec)

- Tier C live GBR purchase wiring (unchanged, still stubbed).
- Light mode (dark-only by decision; tokens are structured so a `[data-theme]` light map could be added later without refactor).
- Retraining/altering the ML models — we surface their outputs, we don't change them.
- Mobile-native app.

## 10. Open Risks

- **TOC derivation accuracy.** GTFS `agency`/`route` → UID mapping may not be 1:1 for every service; backfill must record coverage and leave un-mapped rows clearly "Unknown operator" rather than mis-attributing.
- **Self-hosted font weight.** IBM Plex Mono + Sans woff2 subsets add embedded bytes; subset to Latin to keep it small.
- **uPlot scope creep.** Keep it to the 2–3 interactive charts; everything else stays pure SVG.
