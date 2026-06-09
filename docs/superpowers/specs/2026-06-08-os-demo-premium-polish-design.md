# RailPredict OS — premium polish pass (design)

**Date:** 2026-06-08
**Branch:** `main`
**Status:** Approved design → implementation plan next
**Scope decision:** *Every surface to product-grade* (user-approved). Shell cohesion first (fast, dramatic), then per-app data-viz, then a consistency/QA sweep.
**Companion docs:** [[demo-site-sale]] memory (the IP-sale context), `docs/superpowers/specs/2026-06-06-railpredict-os-design.md` (the original OS build).

## Goal

`docs/os.html` (the offline "RailPredict OS" desktop) is the flagship artifact for the IP sale to a rail-ticket retailer — a numbers-loving CEO will open **every** app. The demo is functionally complete and heavily iterated; the job here is to lift it from "impressive prototype" to "shipped premium product" so nothing a buyer opens reads as unfinished. Out of scope: the model's `+1 min` prediction clustering (that's a retrain, not polish) — it is shown honestly, never papered over.

Hard constraints (unchanged): fully **offline / self-contained** (no external assets or fetches), **no JS framework, no build step**, **no new dependencies**, vanilla JS only. Generated `docs/*.html` stay **uncommitted** (regenerated via `make os` / `make map-day`). The live `/map` dashboard page reuses `map_render.js`, so map changes must stay compatible with it.

## Architecture context (where each surface's data comes from)

The OS bakes two data channels into one offline file (`src/export/os.rs` → `os_template.html`):

| Channel | Source | Feeds |
|---|---|---|
| `window.MAP` (`OsData`) | Rust `gather_os` = `gather_demo` + `gather_map`, queried at export time over a `--days` window | Map KPIs, Predictions/Reliability KPI cards (`[data-fill]`), `MAP.operators` (Reliability list), `MAP.trains` (map nodes) |
| `window.MAP_*` JSON assets | `scripts/build_map_data.py`, rebuilt per **day/range** from the DB → `assets/*.json` baked via `__PLACEHOLDER__` | `MAP_OPS_DAY` (Operators), `MAP_REPLAY` (Replay), `MAP_JOURNEYS`/`MAP_EDGES`/`MAP_STATIONS` (Map), `MAP_ABOUT` (About) |

Rendering is split: `map_render.js` (`window.RailPredictMap`) draws the Map; `os_apps.js` (`window.OsApps`) initialises the five windowed apps; `os_shell.js` is the window manager. All are vanilla JS baked inline.

---

## Workstream 1 — Icon system (replace all emoji)

**Problem:** dock + desktop + the coming-soon fallback render emoji (`🗺️ 🚆 🎯 📊 ▶️ ⓘ`). Emoji render inconsistently across platforms and read as "demo," not "product." This is the single biggest tell.

**Design:** a cohesive **inline-SVG** icon set, fully offline.
- **Visual language:** macOS-app-tile style — a rounded-rect tile per app with a subtle per-app gradient (a single rail-blue base hue, small per-app hue shift) and one crisp white line-glyph at a consistent stroke weight. Glyphs: route-pin (Map), train (Operators), target/bullseye (Predictions), bar-chart (Reliability), play-triangle (Replay), info-circle (About). Restrained, flat-ish, not skeuomorphic — matches the "restrained/premium impresses, gimmicky doesn't" steer from the CEO.
- **Mechanics:** define the glyphs once as an inline SVG `<symbol>` sprite in `os_template.html` (`<svg style="display:none"><symbol id="ic-map" viewBox="0 0 24 24">…</symbol>…</svg>`). The app tile (gradient rounded-rect) is a CSS class; the glyph is `<svg class="glyph"><use href="#ic-map"/></svg>`. `APPS[].icon` in `os_shell.js` becomes an **icon id** (e.g. `"ic-map"`), not an emoji string. Dock items, desktop icons, and the coming-soon fallback all render the tile+`<use>`. The menu-bar brand keeps its small mark.
- **Sizes:** dock ~30px tiles, desktop ~34px, consistent corner radius.

**Acceptance:** zero emoji anywhere in the shell; all six icons share one visual family; renders identically offline in Chromium/WebKit (no font/emoji dependency).

## Workstream 2 — Window lifecycle animation

**Problem:** windows pop into existence and vanish instantly (`createWindow` appends; `closeWin` removes synchronously). Static = prototype.

**Design (pure CSS transitions + class toggles, rAF-triggered — no library):**
- **Open:** start at `scale(.92)`, `opacity 0`, `translateY(6px)` → animate to identity over ~180ms ease-out. Triggered by adding an `.opening` class then removing it on the next frame.
- **Close:** add `.closing` (reverse), remove the element on `transitionend` (with a timeout fallback so a dropped event can't leak the node). Update `closeWin` to defer removal.
- **Minimise/restore:** scale+fade toward / from the dock item (a light "genie" — translate toward the dock-item centre + scale down). Restore reverses.
- **Map window** (opens maximised): same fade/scale, slightly faster.
- **Reduced motion:** wrap all transitions so `@media (prefers-reduced-motion: reduce)` disables them (premium + a11y detail).

**Acceptance:** open/close/minimise all animate smoothly; no leaked DOM nodes on rapid open/close; reduced-motion users get instant, non-animated transitions; the existing AbortController listener cleanup still fires on close.

## Workstream 3 — Per-app data-viz (every window as finished as the Map)

All charts are **client-side SVG** drawn in `os_apps.js` from already-baked real data (the Map already establishes this SVG pattern). Numbers are real; nothing fabricated.

- **Operators** (`MAP_OPS_DAY`, already has `{name, j, otp}`): turn the flat text rows into a **ranked bar league** — each row gets an inline on-time-% bar (width = `otp`, banded green/amber/red) behind the name/figures, with subtle rank emphasis on the top 3. No new data.
- **Reliability:** give it a *distinct* identity so it isn't a second operator list.
  - A **delay-by-hour profile** — 24 mini-bars, "when does the network run late?", coloured by delay band. *Needs the one new baked asset* (`hourly.json`, below).
  - Plus a **recovery vs punctuality** pair (arrival-on-time % and recovery %, already in `window.MAP` as `arrival_on_time_pct` / `recovered_pct`) rendered as two labelled progress bars / gauges instead of bare numbers.
- **Predictions** (compute client-side from `MAP_REPLAY` `{p, a}` pairs — no new data): a **prediction-error histogram** — bucket `|predicted − actual|` into `0 · 1 · 2 · 3–5 · 6–10 · >10 min` and draw a bar chart titled "how far each prediction landed from the real outcome." Honest framing: the model's tight clustering shows as a tall low-error bar = "most predictions within a minute of reality," which sells rather than embarrasses. Keep the three existing KPI cards above it (`mae_mins`, `within_5_pct`, `on_time_pct`).
- **Map / Replay:** already strong — light touches only (consistent empty states, card-update smoothing). No structural change.

**New baked asset (the only new data):** `assets/hourly.json` = `[{ "h": 0..23, "avg": <avg delay min>, "n": <count> }, …]` for the demo day, produced by `build_map_data.py` (it already has `psql` + the day window). Wire it like the other assets: `__HOURLY__` placeholder in `os_template.html` → `const HOURLY = include_str!(...)` + `.replace("__HOURLY__", …)` in `os.rs` → `window.MAP_HOURLY`. Computed over **all** journeys for the day (rigorous, matches the About-app full-day numbers), not the dense-stop map subset.

**Acceptance:** Operators, Reliability, Predictions each render a real SVG chart from real data; Reliability is visibly distinct from Operators; all charts degrade to a clean empty state when their data is absent; offline preserved.

## Workstream 4 — Shell detail & cohesion sweep

- **Menu bar:** remove the dead `File / View / Help` (they do nothing on click). Keep the brand; add a right-side **status chip** "● Replaying yesterday · `<date>`" beside the clock (date from `MAP_ABOUT.date`). No dead affordances.
- **Boot cue:** keep the deliberate bare-desktop boot, but add a **non-intrusive, dismiss-on-first-interaction** affordance pointing at the flagship — a soft pulse-ring on the dock's Live Map icon + a faint "Start with Live Map" pill that fades on the first dock/desktop click. *Not* auto-open.
- **Consistency:** unify empty states (`—` / `…` → one muted placeholder helper); refine the CSS-hatch resize grip to a cleaner corner; make per-app default window sizes feel intentional (analytics apps a touch narrower/taller; Map maximised); minor typography rhythm pass.

**Acceptance:** no control in the shell does nothing; a first-time viewer is gently guided to the Map without being forced; empty states and chrome read consistently across all windows.

---

## Sequencing

1. **Shell cohesion** — W1 (icons) + W2 (window animation) + W4 (menu/boot/grip/empty-states). Fast, dramatic, zero data dependencies.
2. **App data-viz** — W3: Operators bars → Reliability (hourly asset + recovery gauges) → Predictions histogram.
3. **Final consistency + premium-QA** — open every app, check transitions/spacing/typography/empty states; regenerate and eyeball.

## Files

- **New:** `RailPredict/src/export/assets/hourly.json` (committed asset, like the others).
- **Edited:** `os_template.html` (SVG sprite, tile CSS, window-animation CSS, menu bar, `__HOURLY__`, resize grip, empty-state class, reduced-motion), `os_shell.js` (icon ids, open/close/minimise animations, boot cue), `os_apps.js` (Operators bars, Reliability hourly+gauges, Predictions histogram, empty-state helper), `os.rs` (`__HOURLY__` include/replace), `scripts/build_map_data.py` (write `hourly.json`).
- **Light-touch:** `map_render.js` (empty-state consistency only — must stay compatible with the live `/map` page).
- **Version protocol (5 files + CHANGELOG):** `CLAUDE.md`, `README.md`, `TODO.md`, `CHANGELOG.md`, `RailPredict/Cargo.toml`.

## Verification

- `cargo clippy --all-targets -- -D warnings` clean; existing export unit tests (`os.rs`, `map.rs`, `demo.rs`) green; add an `os.rs` test asserting the `__HOURLY__` placeholder is replaced.
- Regenerate the day assets + HTML and open in a browser (the user's visual-review workflow):
  ```
  DATABASE_URL=postgresql://railpredict:railpredict@localhost:5432/railpredict_v2 \
    python3 scripts/build_map_data.py --date <day>
  DATABASE_URL=…railpredict_v2 make os    # and make map
  ```
  Manual QA: open every app; confirm crafted icons, window animations, the three new charts, the status chip, the boot cue, and that the file is still fully offline (no network requests) and small (~1.2 MB order).
- **Out of scope, stated:** the `+1 min` prediction clustering (retrain, separate follow-up).

## Versioning

Multi-workstream epic → **minor bump on completion**: `v1.19.0 → v1.20.0`, all five files + a `CHANGELOG.md` entry, per the strict protocol. (Optionally patch-bump per workstream if landed separately.)
