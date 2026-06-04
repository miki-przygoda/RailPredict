# Visual Changes Plan — seed for the UI improvement plan

**Date:** 2026-06-04
**Branch:** `ui/page-improvements`
**Status:** Working notes — feeds the UI plan the user regenerates in the evening
**Companion docs:** `docs/superpowers/specs/2026-06-03-dashboard-overhaul-design.md` (Signal Terminal design system), `data/refactor-audit/frontend.md` (full audit)

## How to use this doc
This is the running ledger of **visual / IA changes** — both the ones already made and the ones proposed but deferred (because they need eyes-on-a-screen, which wasn't available while working remote). Nothing here changes pixels silently: every item is logged so it can be folded into the regenerated UI plan. Functional/non-visual refactors are tracked separately in git history, not here.

Legend: **[PROPOSED]** not yet built · **[DONE]** already shipped on this branch · **[DECIDE]** needs a product call in the evening session.

---

## 1. The headline pivot — retire the "test my Tier A/B/C features" framing

**Goal:** the site should read as a real UK-rail delay & prediction **analytics product**, not a developer demo of "here are the capabilities I built, organised by tier." The internal three-tier architecture (Static / Predictive / Transactional) stays an engineering concept; it should disappear from the *user-facing* surface.

### What currently signals "demo / test bench"
- **`/demo` "Developer Console"** (`frontend/demo.rs`, 1372 lines): a left "Feature Lab — interactive tests for every major capability (health, autocomplete, departure board, registry probe, live event monitor)" and a right "Ticket Purchase Demo" that simulates the not-yet-wired Tier C flow. This is the single biggest piece of demo-framing.
- **Tier badges** (`.badge-tier-a`, the inline-hacked "Tier B", references to "Tier C purchase flow") scattered through demo/detail.
- **Wording**: "Feature Lab", "Purchase Demo", "what the real API call would contain", "backlog item 2.8".

### Proposed production IA **[DECIDE]**
Replace the demo console with product surfaces that already have backing data/queries:

| Today | Proposed production surface | Backing (already built) |
|---|---|---|
| `/` overview cockpit | **keep** — it's already production-shaped | `db/overview.rs`, `operator_league` |
| `/demo` Developer Console | **retire / fold.** Split its genuinely-useful bits: live event monitor → a small "Live network" widget on `/` or a `/live` page; health/registry probes → a minimal `/status` (ops, not marketing). Drop "Feature Lab" framing entirely. | registry `network_summary` |
| `/demo` Purchase Demo | **move behind a "Coming soon: ticketing" stub** or remove until Tier C is real. Don't present a simulated purchase as a product feature. | (Tier C stubbed) |
| `/predictions` | **keep & rebuild** as the predicted-vs-actual explorer (Phase 4) | `db/analytics.rs` |
| (new) `/operators`, `/operators/:toc` | **build** (Phase 3) | `db/operators.rs` |
| (new) `/stations/:crs` | **build** (Phase 5) | `db/stations.rs` |
| `/report` | fold into the product nav or keep as an export view | `export/` |

**Open question [DECIDE]:** does the developer console disappear entirely, or move to a clearly-separate `/dev` namespace (not linked from the product nav) for your own debugging? Recommendation: keep a stripped `/dev` for yourself, off the main nav, and make `/` → `/operators` → `/predictions` → `/stations` the product story.

---

## 2. Per-page visual notes

### `/` overview cockpit — mostly production-ready
- **[PROPOSED]** Remove/relocate the two **broken `/operators` links** (`dashboard.rs:178, 207`) OR land them at the same time as the Phase 3 `/operators` page so they aren't 404s. (Functional defect, but the fix is visual — a nav card + panel link.)
- **[PROPOSED]** `--text-faint` is a byte-dup of `--text-dim` (`style.css:30`). Alias it (`var(--text-dim)`) or drop it — cosmetic only.

### `/predictions` — rebuild (Phase 4)
- **[PROPOSED]** `hour_chart()` hardcodes data-colour hex (`predictions.rs:324-329, 351`) instead of `currentColor` + `--ok/--warn/--bad` classes. Move to the token system during the rebuild.

### `/demo` — retire/fold (see §1)
- **[PROPOSED]** 🔒 **colour emoji** in the "Confirm & Pay" button (`demo.rs:910`) violates the no-emoji/inline-SVG rule → inline lock SVG (or removed with the purchase flow).
- **[PROPOSED]** Dingbat status glyphs `✓ ✗ ⚠ ▶` used as icons across demo fragments → shared inline-SVG status-icon helper (reuse the dashboard `ICON_*` `PreEscaped` pattern).
- **[PROPOSED]** `.badge-tier-a` labelled "Tier B" with an inline `rgba()` override (`demo.rs:137`) → add a real `.badge-tier-b` token class (or remove with the tier framing).
- **[PROPOSED]** Inline `style="color:var(--red|--green)"` for ingest status (`demo.rs:1124-1125`) → use the existing `.status-ok`/`.status-error` classes.

### Cross-page consistency
- **[PROPOSED]** Six different empty-state / loading class names (`no-results`, `panel-empty`, `pred-no-data`, `demo-error`, `demo-loading`, `not-found`) → one `components::empty_state(msg)` / `loading(msg)` helper.
- **[PROPOSED]** The `→` arrow styled by four separate CSS classes (`.arrow`, `.es-arrow`, `.purchase-arrow`, `.ticket-arrow`) → one class.
- **[PROPOSED]** `demo_status_card` markup repeated ~10× in `demo.rs` → one `status_card(label, value, kind)` helper.

---

## 3. Functional items already done that touch these files (for context, not visual)
- Shared `pence_to_pounds` + `compact_count` now live in `components.rs` (dedup; no render change).
- `frontend/mod.rs` route doc corrected.
- `--text-faint` token defined earlier (the undefined-var bug is already fixed; the dup remains, see above).

---

## 4. Security/correctness with a visual touch (do during rebuild)
- **[PROPOSED]** `demo_events_sse` builds raw HTML via `format!()` and interpolates `train_id` unescaped (`demo.rs:1349`). `TrainId::rid()` validates length only, so this is the one dynamic-data path bypassing maud's auto-escape. Server-sourced so low practical risk, but rebuild the row as a `maud` fragment when this page is reworked.

---

## 5. Suggested order for the evening
1. Decide the **IA** (§1 `[DECIDE]`): does `/demo` die, fold, or move to `/dev`?
2. Build the **`/operators`** pages (Phase 3) and land the dashboard links at the same time (kills the broken-link defect).
3. Rebuild **`/predictions`** (Phase 4) on the token system (fix the hardcoded hex).
4. Do the **consistency sweep** (empty-state/loading/status-card/arrow helpers, emoji→SVG) as one pass once the page set is settled.
5. Build **`/stations/:crs`** (Phase 5) + reskin remaining pages.
