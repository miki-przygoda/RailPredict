# UI Overhaul — Step 0 (Foundation) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reshape the product navigation to the Option-A IA and retire the `/demo` "Developer Console" framing — move its diagnostics to an off-nav `/dev` and replace the simulated ticket purchase with a "coming soon" stub — without changing the design system.

**Architecture:** Pure `maud`/`axum` frontend changes. The nav is defined once in `frontend/layout.rs`; pages call `base(title, NavPage, content)`. The demo surface lives in `frontend/demo.rs` (renamed to `frontend/dev.rs`) and is wired in `api/mod.rs`. The Signal Terminal design system (tokens, fonts, `charts.rs`, `time_range_picker`) already exists and is **not** touched here.

**Tech Stack:** Rust, axum 0.7, maud 0.26, sqlx 0.8 (Postgres), zero-dep HTTP tests (reqwest + `#[sqlx::test]`).

**Reference:** Spec `docs/superpowers/specs/2026-06-04-ui-overhaul-execution.md` (§3 IA, §4 retire `/demo`, §7 step 0). Ledger `docs/superpowers/specs/2026-06-04-visual-changes-plan.md`.

**Reconciliation note (read first):** The spec's "final 5-item nav" is `Overview · Operators · Predictions · Stations · Explore`. The Operators and Stations *pages* are built in later steps (2 and 4). To avoid shipping nav links that 404, step 0 sets the nav to the pages that exist **now** — `Overview · Predictions · Explore` — and adds the `Operators`/`Stations` `NavPage` enum variants so steps 2/4 only add one link line each. Departures (`/search`) leaves the top nav now (reachable from the dashboard's "Departure Board" card).

---

## Task 1: Reshape the top navigation

**Files:**
- Modify: `RailPredict/src/frontend/layout.rs` (NavPage enum ~lines 7-15; nav links ~lines 43-49; test ~lines 88-93)

- [ ] **Step 1: Update the failing unit test first**

In `RailPredict/src/frontend/layout.rs`, replace the existing `tests` module body's assertions so it asserts the new nav. Replace the `active_page_is_marked` test with:

```rust
    #[test]
    fn active_page_is_marked() {
        let html = base("Test", NavPage::Predictions, maud::html! { p { "x" } }).into_string();
        assert!(html.contains("aria-current=\"page\""), "marks active link: {html}");
        assert!(html.contains("Overview"), "has Overview nav link");
        assert!(html.contains(">Explore<"), "has Explore nav link");
        assert!(!html.contains("Dev Console"), "Dev Console removed from nav");
        assert!(!html.contains(">Departures<"), "Departures removed from nav");
        assert!(!html.contains('\u{26A0}'), "emoji replaced with svg");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd RailPredict && cargo test --lib frontend::layout::tests::active_page_is_marked`
Expected: FAIL — current nav still renders "Dev Console" / "Departures" and the label is "Dashboard" not "Overview".

- [ ] **Step 3: Add the new NavPage variants**

In `RailPredict/src/frontend/layout.rs`, change the `NavPage` enum to add `Operators` and `Stations` (keep `Departures` and `DevConsole` — they are still passed by `search.rs`, `detail.rs`, and `dev.rs`'s `base(...)` calls even though they no longer appear as top-nav links):

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NavPage {
    Dashboard,
    Departures,
    Operators,
    Predictions,
    Stations,
    Explore,
    DevConsole,
    None,
}
```

- [ ] **Step 4: Rewrite the nav links block**

In `RailPredict/src/frontend/layout.rs`, replace the `div .nav-links { ... }` block with the new set (Departures and Dev Console removed; Operators/Stations intentionally not linked yet — added in steps 2/4):

```rust
                    div .nav-links {
                        (link("/", "Overview", NavPage::Dashboard))
                        (link("/predictions", "Predictions", NavPage::Predictions))
                        (link("/explore", "Explore", NavPage::Explore))
                    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cd RailPredict && cargo test --lib frontend::layout::tests::active_page_is_marked`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add RailPredict/src/frontend/layout.rs
git commit -m "feat(nav): reshape top nav to Overview/Predictions/Explore (IA option A)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Retire the demo framing on the dashboard

**Files:**
- Modify: `RailPredict/src/frontend/dashboard.rs` (page title line 76; nav-card grid lines 199-205; icon consts lines 244-248)

- [ ] **Step 1: Rename the page title to "Overview"**

In `RailPredict/src/frontend/dashboard.rs`, change the non-htmx branch:

```rust
        base("Overview", NavPage::Dashboard, body)
```

- [ ] **Step 2: Swap the Dev Console nav card for a Query Explorer card**

In `RailPredict/src/frontend/dashboard.rs`, replace the `.dash-nav-grid` block (currently four cards ending in the `/demo` "Dev Console" card) with:

```rust
                div .dash-nav-grid {
                    (nav_card("/search", ICON_BOARD, "Departure Board", "Live departures from any UK station."))
                    (nav_card("/predictions", ICON_CHART, "Prediction analytics", "Accuracy, calibration & biggest errors."))
                    (nav_card("/operators", ICON_TROPHY, "Operators", "Per-operator punctuality league & drill-down."))
                    (nav_card("/explore", ICON_EXPLORE, "Query Explorer", "Build your own delay & prediction queries."))
                }
```

- [ ] **Step 3: Replace the gear icon with an explore icon**

In `RailPredict/src/frontend/dashboard.rs`, delete the `ICON_GEAR` const (now unused — would trip `dead_code`) and add `ICON_EXPLORE` in its place:

```rust
const ICON_EXPLORE: PreEscaped<&'static str> = PreEscaped(r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/></svg>"#);
```

- [ ] **Step 4: Verify it compiles with no dead-code warning**

Run: `cd RailPredict && cargo build 2>&1 | grep -E 'ICON_GEAR|warning|error' || echo "clean"`
Expected: `clean` (no reference to ICON_GEAR, no warnings from this file).

- [ ] **Step 5: Commit**

```bash
git add RailPredict/src/frontend/dashboard.rs
git commit -m "feat(dashboard): swap Dev Console card for Query Explorer, title 'Overview'

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Rename the `demo` module to `dev` and re-point its routes

**Files:**
- Rename: `RailPredict/src/frontend/demo.rs` → `RailPredict/src/frontend/dev.rs`
- Modify: `RailPredict/src/frontend/mod.rs` (module decl line 18 + doc line 12)
- Modify: `RailPredict/src/api/mod.rs` (route table lines 167, 171-180)

- [ ] **Step 1: Rename the module file (preserve history)**

```bash
git mv RailPredict/src/frontend/demo.rs RailPredict/src/frontend/dev.rs
```

- [ ] **Step 2: Update the module declaration and doc**

In `RailPredict/src/frontend/mod.rs`, change `pub mod demo;` to `pub mod dev;` and update the doc line:

```rust
//! - `dev`         — `/dev`              internal diagnostics console (+ `/ui/dev/*`)
```
```rust
pub mod dev;
```

- [ ] **Step 3: Rename the public handler symbols in `dev.rs`**

In `RailPredict/src/frontend/dev.rs`, rename every public handler so the `demo_` prefix becomes `dev_`. The handlers are:
`demo_page→dev_page`, `demo_status_fragment→dev_status_fragment`, `demo_predictions_fragment→dev_predictions_fragment`, `demo_registry_fragment→dev_registry_fragment`, `demo_ingest_freshness→dev_ingest_freshness`, `demo_ingest_start→dev_ingest_start`, `demo_ingest_stream→dev_ingest_stream`, `demo_events_sse→dev_events_sse`.

(The three purchase handlers `demo_journeys_fragment`, `demo_checkout_fragment`, `demo_purchase_fragment` are **deleted** in Task 4, not renamed.)

- [ ] **Step 4: Re-point the htmx paths inside `dev.rs` markup**

In `RailPredict/src/frontend/dev.rs`, change every occurrence of the string `/ui/demo/` to `/ui/dev/` (these appear in `hx-get`/`hx-post` attributes — `status`, `predictions`, `registry`, `ingest/freshness`, `ingest/start`, `ingest/stream`, `events`). Leave `/ui/stations/search` (the autocomplete tester) unchanged.

- [ ] **Step 5: Update the route table in `api/mod.rs`**

In `RailPredict/src/api/mod.rs`, replace the demo routes. Change `.route("/demo", get(demo::demo_page))` to `.route("/dev", get(dev::dev_page))`, and replace the `/ui/demo/*` block — **dropping the three purchase routes** (`journeys`, `checkout`, `purchase`) — with:

```rust
        .route("/ui/dev/status",      get(dev::dev_status_fragment))
        .route("/ui/dev/predictions", get(dev::dev_predictions_fragment))
        .route("/ui/dev/registry",    get(dev::dev_registry_fragment))
        .route("/ui/dev/events",          get(dev::dev_events_sse))
        .route("/ui/dev/ingest/freshness", get(dev::dev_ingest_freshness))
        .route("/ui/dev/ingest/start",    post(dev::dev_ingest_start))
        .route("/ui/dev/ingest/stream",   get(dev::dev_ingest_stream))
```

Also update the `use` import at the top of `api/mod.rs`: change `use crate::frontend::{... demo ...}` to reference `dev` (find the import line listing `demo` and rename it to `dev`).

- [ ] **Step 6: Verify it compiles (purchase fns now unused — expected to fail until Task 4)**

Run: `cd RailPredict && cargo build 2>&1 | tail -20`
Expected: compile errors/warnings only about the now-unreferenced `demo_journeys_fragment`/`demo_checkout_fragment`/`demo_purchase_fragment`/`PurchaseForm` (removed in Task 4) and the `Form`/purchase imports. This task is committed together with Task 4; do not commit yet.

---

## Task 4: Drop the simulated purchase, add a "Ticketing — coming soon" stub

**Files:**
- Modify: `RailPredict/src/frontend/dev.rs` (delete purchase fns + struct; replace the RIGHT PANEL block; strip Tier badges/wording/emoji)

- [ ] **Step 1: Delete the purchase handlers and form struct**

In `RailPredict/src/frontend/dev.rs`, delete these three functions in full — `dev`'s purchase fragments — and the form struct:
- `pub async fn demo_journeys_fragment(...) { ... }`
- `pub async fn demo_checkout_fragment(...) { ... }`
- `pub async fn demo_purchase_fragment(...) { ... }`
- `pub struct PurchaseForm { ... }`

Remove any now-unused imports they pulled in (`axum::Form`, `pence_to_pounds` if only used there, the coalescer/gbr purchase imports). Let the compiler tell you which (`cargo build` lists unused imports).

- [ ] **Step 2: Replace the RIGHT PANEL — Ticket Purchase Demo block with a stub**

In `dev_page` (`RailPredict/src/frontend/dev.rs`), find the markup comment `// RIGHT PANEL — Ticket Purchase Demo` and delete its entire `div .demo-col { ... }` sibling block (the right column). Replace it with this stub column:

```rust
                // ── RIGHT PANEL — Ticketing (stub) ────────────────────────
                div .demo-col {
                    div .demo-section {
                        div .demo-section-title { "Ticketing" }
                        p .demo-hint {
                            "Live ticket purchase is not wired in this build. "
                            "The circuit-breaker, idempotency layer, and purchase ledger "
                            "exist; the GBR Retail write endpoint is pending commercial access."
                        }
                        p .demo-hint { "Status: " strong { "coming soon" } "." }
                    }
                }
```

- [ ] **Step 3: Strip the Tier badges and demo wording from `dev_page`**

In `RailPredict/src/frontend/dev.rs` `dev_page`:
- Change the header `h1 { "Developer Console" }` to `h1 { "Diagnostics" }`.
- Change the subtitle line `"Feature Lab  ·  Purchase Demo  ·  "` to `"Internal diagnostics  ·  "`.
- Delete the `span .demo-badge .badge-tier-a ... { "Tier B" }` after the "Predicted vs Actual" title (keep the title text itself).
- Delete the `span .demo-badge .badge-tier-a { "Tier A" }` in the departure-board tester section.
- Remove the `// LEFT PANEL — Feature Lab` comment wording (rename to `// LEFT PANEL — diagnostics`).

- [ ] **Step 4: Confirm no purchase/emoji/Tier remnants remain**

Run: `cd RailPredict && grep -nE 'Tier [ABC]|Feature Lab|Purchase Demo|🔒|badge-tier|PurchaseForm|demo_purchase|demo_checkout|demo_journeys' src/frontend/dev.rs || echo "clean"`
Expected: `clean`

- [ ] **Step 5: Verify the whole crate builds with no warnings**

Run: `cd RailPredict && cargo build 2>&1 | grep -E 'warning|error' || echo "clean"`
Expected: `clean`

- [ ] **Step 6: Commit Tasks 3 + 4 together**

```bash
git add RailPredict/src/frontend/dev.rs RailPredict/src/frontend/demo.rs RailPredict/src/frontend/mod.rs RailPredict/src/api/mod.rs
git commit -m "refactor(dev): retire /demo console to /dev, drop simulated purchase

Rename frontend::demo to frontend::dev; route /demo->/dev and /ui/demo/*->
/ui/dev/* (dropping the purchase fragments). Replace the Ticket Purchase
Demo with a 'Ticketing — coming soon' stub and strip Tier badges, Feature
Lab wording, and the lock emoji. Diagnostics probes (status, predictions,
registry, ingest, events) are preserved off-nav.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Update internal references and the HTTP smoke test

**Files:**
- Modify: `RailPredict/src/main.rs` (line ~296 user-facing message)
- Modify: `RailPredict/src/db/predictions.rs` (doc lines ~12, ~145)
- Modify: `RailPredict/src/ingestion/gtfs.rs` (doc line ~508)
- Modify: `RailPredict/tests/http_smoke.rs`

- [ ] **Step 1: Update the `/demo` mentions in code comments and messages**

- `RailPredict/src/main.rs`: change the message `"Stations table is empty — set GTFS_URL or use /demo to ingest timetable data"` to `"... or use /dev to ingest timetable data"`. Update the two nearby comments mentioning "demo SSE" to "dev SSE".
- `RailPredict/src/db/predictions.rs`: change the two doc comments `/demo/predictions` to `/dev/predictions`.
- `RailPredict/src/ingestion/gtfs.rs`: change the doc `POST /ui/demo/ingest/start` to `POST /ui/dev/ingest/start`.

- [ ] **Step 2: Add the failing smoke assertions**

In `RailPredict/tests/http_smoke.rs`, inside `pages_and_endpoints_respond`, after the predictions-page block, add:

```rust
    // Diagnostics console moved to /dev; the old /demo route is gone.
    let (s, b) = app.get("/dev").await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.contains("Diagnostics"), "dev console markup unexpected");
    assert!(!b.contains("Tier C"), "tier framing removed");
    assert!(!b.contains("Confirm & Pay"), "simulated purchase removed");

    let (s, _) = app.get("/demo").await;
    assert_eq!(s, StatusCode::NOT_FOUND, "/demo retired");

    // Top nav no longer advertises the dev console.
    let (_s, home) = app.get("/").await;
    assert!(home.contains(">Overview<"), "nav has Overview");
    assert!(!home.contains("Dev Console"), "nav no longer shows Dev Console");
```

- [ ] **Step 3: Run the smoke test to verify the new assertions pass**

Run: `cd RailPredict && DATABASE_URL=postgresql://railpredict:railpredict@localhost:5432/railpredict cargo test --test http_smoke`
Expected: PASS (requires the local Postgres from `docker compose up -d db`).

- [ ] **Step 4: Commit**

```bash
git add RailPredict/src/main.rs RailPredict/src/db/predictions.rs RailPredict/src/ingestion/gtfs.rs RailPredict/tests/http_smoke.rs
git commit -m "test(http): assert /dev replaces /demo; update stale /demo references

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Full verification + version bump

**Files:**
- Modify: `CLAUDE.md`, `README.md`, `TODO.md`, `CHANGELOG.md`, `RailPredict/Cargo.toml` (version `1.12.13` → `1.12.14`)
- Modify: `docs/superpowers/specs/2026-06-04-visual-changes-plan.md` (mark the retire-`/demo` items [DONE])

- [ ] **Step 1: Run the full test + lint suite**

Run: `cd RailPredict && cargo clippy --all-targets -- -D warnings && DATABASE_URL=postgresql://railpredict:railpredict@localhost:5432/railpredict cargo test`
Expected: clippy clean, all tests pass.

- [ ] **Step 2: Bump the version in all five files**

Set the version string to `1.12.14` (date `04/06/2026`) in: `CLAUDE.md` header, `README.md` (`**v1.12.14 — June 2026**`), `TODO.md` header, `RailPredict/Cargo.toml` (`version = "1.12.14"`), and add a `CHANGELOG.md` entry:

```markdown
## v1.12.14 — 04/06/2026

UI overhaul step 0 (foundation). Reshaped the top nav to the Option-A product IA
(Overview · Predictions · Explore; Departures and the dev console off-nav).
Retired the `/demo` "Developer Console" to an off-nav `/dev` diagnostics page,
dropped the simulated ticket purchase for a "coming soon" ticketing stub, and
removed Tier badges / Feature Lab framing. No design-system changes.
```

- [ ] **Step 3: Mark the visual-changes-plan items done**

In `docs/superpowers/specs/2026-06-04-visual-changes-plan.md`, change the `/demo` retirement items (§1, §2 `/demo` section) and the emoji/Tier-badge items from **[PROPOSED]** to **[DONE]**.

- [ ] **Step 4: Commit the version bump**

```bash
git add CLAUDE.md README.md TODO.md CHANGELOG.md RailPredict/Cargo.toml docs/superpowers/specs/2026-06-04-visual-changes-plan.md
git commit -m "chore(release): bump v1.12.14 (UI overhaul step 0 — nav + retire /demo)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 5: Push the branch**

```bash
git push origin ui/page-improvements
```

---

## Done criteria

- Top nav shows `Overview · Predictions · Explore`; no Departures or Dev Console items; Operators/Stations variants exist for steps 2/4.
- `/dev` serves the diagnostics console (status, predictions, registry, ingest, events); `/demo` 404s.
- No "Tier", "Feature Lab", "Purchase Demo", or lock-emoji strings remain in `dev.rs`; the simulated purchase is gone.
- Dashboard advertises Departure Board / Prediction analytics / Operators / Query Explorer (no Dev Console card).
- `cargo clippy -D warnings` clean; `cargo test` green; version at `1.12.14` across all five files; branch pushed.
```
