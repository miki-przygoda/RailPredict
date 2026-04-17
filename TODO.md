# TODO

The current version and last worked on date should be noted at the top of this file below this line:

**version = "0.7.0" -- 17/04/2026**

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

## Current Epics

### ~~Epic 1 — Core Data Types~~ — COMPLETE (v0.2.0)
### ~~Epic 2 — State Machine~~ — COMPLETE (v0.3.0)
### ~~Epic 3 — Networking Layer~~ — COMPLETE (v0.4.0)
### ~~Epic 4 — Data Ingestion~~ — COMPLETE (v0.5.0)
### ~~Epic 5 — In-Memory Cache~~ — COMPLETE (v0.6.0)

### ~~Epic 6 — Runtime Wiring & Observability~~ — COMPLETE (v0.7.0)

### Epic 7 — HTTP API + Rust Frontend (`TODOs/UI.md`)
Two sub-epics: axum REST/SSE API first, then Rust/maud/htmx frontend. No Node, no npm.

**Sub-Epic A — axum API**
- [ ] `src/api/types.rs` — client DTOs: `TrainSummary`, `DepartureBoardEntry`, `LiveUpdateEvent`
- [ ] `GET /stations/{crs}/departures` — Tier A, zero live calls; returns JSON or maud HTML fragment
- [ ] `GET /trains/{rid}` — Tier B from registry, coalesced GBR fallback
- [ ] `GET /trains/{rid}/live` — SSE stream of `LiveUpdateEvent` (maud OOB swap fragments), heartbeat every 15s
- [ ] CORS + `TraceLayer` middleware
- [ ] Wire axum into `main.rs` alongside existing tasks
- [ ] API integration test

**Sub-Epic B — Rust/maud/htmx frontend**
- [ ] `src/frontend/` module — `layout.rs`, `search.rs`, `detail.rs`, `components.rs`
- [ ] `/` Search page — maud full-page render; htmx departure board swap
- [ ] `/trains/:rid` Detail page — maud render + `hx-ext="sse"` live updates
- [ ] Live route diagram (maud SVG, position updated by htmx OOB swap on SSE)
- [ ] Delay badge component — `fn delay_badge(minutes, cancelled) -> Markup`
- [ ] Stale-data banner — htmx SSE error handler reveals/dismisses
- [ ] `rust-embed` bundles `static/style.css` into binary at compile time — no filesystem dep

---

## Strategic Notes (Carry-Forward)

These are cross-cutting design decisions to keep in mind across all epics:

- **The Identifier Problem:** Use `TrainID` everywhere. Never pass raw strings for train identifiers across module boundaries.
- **The Waiter Pattern:** The coalescer in Epic 3 is the most impactful single piece of work for latency. Prioritise it.
- **Backpressure:** GBR will revoke API keys for aggressive polling. The rate limiter and circuit breaker are not optional.
- **The Ingestion Filter:** Apply the region/route filter as step one of ingestion. CPU cost compounds fast on an unfiltered firehose.
