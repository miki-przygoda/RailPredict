# Dashboard Phase 0 — Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish the "Signal Terminal" design foundation — vendored assets, design tokens, IBM Plex typography, an SVG chart-helper module, and an upgraded nav — and prove it on the existing dashboard, with no new pages yet.

**Architecture:** Pure server-rendered maud + htmx, no build step. New static assets (uPlot, IBM Plex woff2) are embedded via the existing `rust-embed` `StaticAssets` folder. A new `frontend/charts.rs` module returns reusable inline-SVG `Markup`. `layout::base` gains an active-nav parameter via a `NavPage` enum.

**Tech Stack:** Rust, axum, maud, htmx, rust-embed, inline SVG, uPlot (vendored, used in later phases).

**Spec:** `docs/superpowers/specs/2026-06-03-dashboard-overhaul-design.md`

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `RailPredict/static/vendor/uplot.iife.min.js`, `uplot.min.css` | Vendored uPlot (used Phase 4+) | Create (binary download) |
| `RailPredict/static/fonts/*.woff2` | IBM Plex Mono + Sans, Latin subset | Create (binary download) |
| `RailPredict/static/style.css` | Design tokens, `@font-face`, component CSS | Modify |
| `RailPredict/src/frontend/charts.rs` | Reusable inline-SVG chart helpers + tests | Create |
| `RailPredict/src/frontend/mod.rs` | Register `charts` module | Modify |
| `RailPredict/src/frontend/layout.rs` | `NavPage` enum, active nav, Dashboard link, SVG icon | Modify |
| `RailPredict/src/frontend/components.rs` | `time_range_picker` component | Modify |
| `RailPredict/src/frontend/dashboard.rs` | KPI strip proving the system | Modify |
| Callers of `layout::base` (dashboard/demo/detail/predictions/search + report) | Pass `NavPage` | Modify |
| `CLAUDE.md`, `README.md`, `TODO.md`, `CHANGELOG.md`, `RailPredict/Cargo.toml` | Version bump to 1.12.5 | Modify |

**Phase-0 charts.rs scope (YAGNI):** only `trend_arrow`, `sparkline`, `kpi_card`, `bar_cell`. `column_chart`, `heatmap`, `histogram`, `calibration_plot` are added in the phases that first use them (Overview / Predictions).

---

## Task 1: Vendor static assets (uPlot + IBM Plex fonts)

**Files:**
- Create: `RailPredict/static/vendor/uplot.iife.min.js`, `RailPredict/static/vendor/uplot.min.css`
- Create: `RailPredict/static/fonts/IBMPlexMono-Regular.woff2`, `IBMPlexMono-Medium.woff2`, `IBMPlexMono-SemiBold.woff2`, `IBMPlexSans-Regular.woff2`, `IBMPlexSans-Medium.woff2`, `IBMPlexSans-SemiBold.woff2`

- [ ] **Step 1: Create directories**

Run:
```bash
mkdir -p RailPredict/static/vendor RailPredict/static/fonts
```

- [ ] **Step 2: Download uPlot (pinned version 1.6.31)**

Run:
```bash
curl -fsSL https://cdn.jsdelivr.net/npm/uplot@1.6.31/dist/uPlot.iife.min.js -o RailPredict/static/vendor/uplot.iife.min.js
curl -fsSL https://cdn.jsdelivr.net/npm/uplot@1.6.31/dist/uPlot.min.css -o RailPredict/static/vendor/uplot.min.css
```
Expected: two non-empty files. Verify:
```bash
test -s RailPredict/static/vendor/uplot.iife.min.js && test -s RailPredict/static/vendor/uplot.min.css && echo OK
```
Expected output: `OK`

- [ ] **Step 3: Download IBM Plex woff2 (Latin subset) from the fontsource CDN**

Run:
```bash
base="https://cdn.jsdelivr.net/fontsource/fonts"
curl -fsSL "$base/ibm-plex-mono@latest/latin-400-normal.woff2" -o RailPredict/static/fonts/IBMPlexMono-Regular.woff2
curl -fsSL "$base/ibm-plex-mono@latest/latin-500-normal.woff2" -o RailPredict/static/fonts/IBMPlexMono-Medium.woff2
curl -fsSL "$base/ibm-plex-mono@latest/latin-600-normal.woff2" -o RailPredict/static/fonts/IBMPlexMono-SemiBold.woff2
curl -fsSL "$base/ibm-plex-sans@latest/latin-400-normal.woff2" -o RailPredict/static/fonts/IBMPlexSans-Regular.woff2
curl -fsSL "$base/ibm-plex-sans@latest/latin-500-normal.woff2" -o RailPredict/static/fonts/IBMPlexSans-Medium.woff2
curl -fsSL "$base/ibm-plex-sans@latest/latin-600-normal.woff2" -o RailPredict/static/fonts/IBMPlexSans-SemiBold.woff2
```
Verify all six exist and are non-empty:
```bash
ls -l RailPredict/static/fonts/*.woff2 | wc -l
```
Expected output: `6`

> If the `@latest` fontsource paths 404, fall back to: `https://cdn.jsdelivr.net/npm/@fontsource/ibm-plex-mono@5/files/ibm-plex-mono-latin-400-normal.woff2` (and the matching 500/600 + ibm-plex-sans variants). Confirm each file is >5 KB before continuing.

- [ ] **Step 4: Verify rust-embed picks them up (compile)**

Run:
```bash
cd RailPredict && cargo build 2>&1 | tail -5
```
Expected: build succeeds (the `#[folder = "static/"]` embed includes the new subfolders automatically).

- [ ] **Step 5: Commit**

```bash
git add RailPredict/static/vendor RailPredict/static/fonts
git commit -m "chore(ui): vendor uPlot + IBM Plex woff2 static assets"
```

---

## Task 2: Signal Terminal design tokens + fonts in `style.css`

**Files:**
- Modify: `RailPredict/static/style.css` (the `:root` block near the top, and the `body` font-family)

- [ ] **Step 1: Add `@font-face` declarations at the very top of `style.css`**

Insert above the existing `/* === RailPredict — Design System === */` banner:

```css
/* --- Fonts: IBM Plex (self-hosted, Latin subset) --- */
@font-face { font-family:"IBM Plex Sans"; font-style:normal; font-weight:400; font-display:swap; src:url("/static/fonts/IBMPlexSans-Regular.woff2") format("woff2"); }
@font-face { font-family:"IBM Plex Sans"; font-style:normal; font-weight:500; font-display:swap; src:url("/static/fonts/IBMPlexSans-Medium.woff2") format("woff2"); }
@font-face { font-family:"IBM Plex Sans"; font-style:normal; font-weight:600; font-display:swap; src:url("/static/fonts/IBMPlexSans-SemiBold.woff2") format("woff2"); }
@font-face { font-family:"IBM Plex Mono"; font-style:normal; font-weight:400; font-display:swap; src:url("/static/fonts/IBMPlexMono-Regular.woff2") format("woff2"); }
@font-face { font-family:"IBM Plex Mono"; font-style:normal; font-weight:500; font-display:swap; src:url("/static/fonts/IBMPlexMono-Medium.woff2") format("woff2"); }
@font-face { font-family:"IBM Plex Mono"; font-style:normal; font-weight:600; font-display:swap; src:url("/static/fonts/IBMPlexMono-SemiBold.woff2") format("woff2"); }
```

- [ ] **Step 2: Replace the colour/brand tokens inside `:root`**

In the existing `:root { ... }` block, replace the `/* Backgrounds */`, `/* Borders */`, `/* Text */`, and `/* Brand palette */` groups with the Signal Terminal tokens. Keep the existing radius/shadow groups unchanged. The replacement:

```css
  /* Backgrounds (OLED dark, layered) */
  --bg:          #0a0e12;
  --surface:     #12181f;
  --surface-2:   #1a2129;
  --surface-3:   #20212e;

  /* Borders */
  --border:      #232c36;
  --border-2:    #2c3744;
  --border-focus: rgba(245,166,35,0.50);

  /* Text */
  --text:        #e6edf3;
  --text-muted:  #9aa7b4;
  --text-dim:    #6b7785;

  /* Brand / interactive accent — amber. NEVER used to encode data. */
  --accent:      #f5a623;
  --accent-press:#d98c12;
  --accent-dim:  rgba(245,166,35,0.14);

  /* Semantic punctuality scale — data only */
  --ok:          #34d399;   /* on time */
  --ok-dim:      rgba(52,211,153,0.14);
  --warn:        #f2c14e;   /* minor delay (yellow, distinct from accent orange) */
  --warn-dim:    rgba(242,193,78,0.14);
  --bad:         #f04545;   /* severe / cancelled */
  --bad-dim:     rgba(240,69,69,0.14);
  --info:        #4a9eff;   /* neutral data */
  --info-dim:    rgba(74,158,255,0.14);

  /* Back-compat aliases (existing CSS references these names) */
  --green:       var(--ok);
  --green-dim:   var(--ok-dim);
  --amber:       var(--warn);
  --amber-dim:   var(--warn-dim);
  --red:         var(--bad);
  --red-dim:     var(--bad-dim);
  --blue:        var(--info);
  --blue-dim:    var(--info-dim);

  /* Type families */
  --font-sans: "IBM Plex Sans", -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  --font-mono: "IBM Plex Mono", ui-monospace, "SF Mono", "Menlo", monospace;
```

> Rationale for aliases: the existing 2,094-line stylesheet references `--green` as the brand colour in many places (e.g. `.nav-brand`). Aliasing `--green → --ok` keeps those rules valid while we migrate brand chrome to `--accent` incrementally. The nav brand is re-pointed to `--accent` in Task 4's CSS.

- [ ] **Step 3: Update `body` font + base numerics**

Replace the `body { ... font-family: ...; font-size: 15px; ... }` declaration's `font-family` line with:
```css
  font-family: var(--font-sans);
```
And add a rule directly after the `body` block:
```css
/* All figures use tabular mono so columns never shift */
.mono, .kpi-number, .num, table td.num, .bar-cell + .num { font-family: var(--font-mono); font-variant-numeric: tabular-nums; }
```

- [ ] **Step 4: Verify the CSS still serves (compile + smoke)**

Run:
```bash
cd RailPredict && cargo build 2>&1 | tail -3
```
Expected: build succeeds. (CSS is embedded; no separate lint.)

- [ ] **Step 5: Commit**

```bash
git add RailPredict/static/style.css
git commit -m "feat(ui): Signal Terminal design tokens + IBM Plex fonts"
```

---

## Task 3: `charts.rs` — inline-SVG helpers (TDD)

**Files:**
- Create: `RailPredict/src/frontend/charts.rs`
- Modify: `RailPredict/src/frontend/mod.rs`
- Test: inline `#[cfg(test)]` module in `charts.rs`

- [ ] **Step 1: Register the module**

In `RailPredict/src/frontend/mod.rs`, add after `pub mod components;`:
```rust
pub mod charts;
```

- [ ] **Step 2: Write the failing tests first**

Create `RailPredict/src/frontend/charts.rs` with ONLY the test module and empty stubs so it compiles-then-fails:

```rust
//! Reusable server-rendered inline-SVG chart helpers ("Signal Terminal").
//!
//! Every helper returns `maud::Markup` containing pure inline SVG/HTML — no JS,
//! no external requests, htmx-swappable. Colour comes from `currentColor` so the
//! caller controls semantics via CSS classes (`--ok` / `--warn` / `--bad`).

use maud::{Markup, html};

pub fn trend_arrow(_delta: f64) -> Markup { html! {} }
pub fn sparkline(_values: &[f64]) -> Markup { html! {} }
pub fn kpi_card(_label: &str, _value: &str, _unit: Option<&str>, _delta: Option<f64>, _spark: Option<&[f64]>) -> Markup { html! {} }
pub fn bar_cell(_fraction: f64) -> Markup { html! {} }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparkline_empty_has_no_polyline() {
        let s = sparkline(&[]).into_string();
        assert!(s.contains("<svg"), "still renders an svg frame: {s}");
        assert!(!s.contains("<polyline"), "no polyline for empty data: {s}");
    }

    #[test]
    fn sparkline_plots_all_points() {
        let s = sparkline(&[1.0, 5.0, 2.0, 8.0]).into_string();
        assert!(s.contains("<polyline"), "has a polyline: {s}");
        // 4 points => 3 spaces separating "x,y" tokens
        let pts = s.split("points=\"").nth(1).unwrap().split('"').next().unwrap();
        assert_eq!(pts.split(' ').count(), 4, "four coordinate pairs: {pts}");
    }

    #[test]
    fn trend_arrow_direction_classes() {
        assert!(trend_arrow(2.5).into_string().contains("trend-up"));
        assert!(trend_arrow(-2.5).into_string().contains("trend-down"));
        assert!(trend_arrow(0.0).into_string().contains("trend-flat"));
        assert!(trend_arrow(2.5).into_string().contains("+2.5"));
    }

    #[test]
    fn bar_cell_clamps_and_sets_width() {
        assert!(bar_cell(0.5).into_string().contains("width:50%"));
        assert!(bar_cell(1.7).into_string().contains("width:100%"));
        assert!(bar_cell(-0.3).into_string().contains("width:0%"));
    }

    #[test]
    fn kpi_card_renders_label_value_unit() {
        let m = kpi_card("On-time", "92.4", Some("%"), Some(-1.2), Some(&[1.0, 2.0, 3.0])).into_string();
        assert!(m.contains("On-time"));
        assert!(m.contains("92.4"));
        assert!(m.contains("kpi-unit"));
        assert!(m.contains("trend-down")); // -1.2
        assert!(m.contains("<polyline")); // spark present
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run:
```bash
cd RailPredict && cargo test --lib frontend::charts 2>&1 | tail -20
```
Expected: compiles, multiple FAILED (stubs return empty markup).

- [ ] **Step 4: Implement the helpers**

Replace the four stub functions with:

```rust
/// Small triangular trend indicator. Colour is set by the caller via the
/// `.trend-up` / `.trend-down` / `.trend-flat` classes (semantics are caller's
/// choice — for delays, "down" is good).
pub fn trend_arrow(delta: f64) -> Markup {
    let (cls, path) = if delta > 0.0 {
        ("trend-up", "M5 2 L9 8 L1 8 Z")
    } else if delta < 0.0 {
        ("trend-down", "M1 2 L9 2 L5 8 Z")
    } else {
        ("trend-flat", "M1 5 H9")
    };
    html! {
        span class=(format!("trend {cls}")) {
            svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true" {
                path d=(path) fill="currentColor" stroke="currentColor" {}
            }
            span .trend-val { (format!("{delta:+.1}")) }
        }
    }
}

/// Minimal sparkline (120×28 viewBox, non-scaling stroke). Renders an empty SVG
/// frame for empty input so layout never shifts.
pub fn sparkline(values: &[f64]) -> Markup {
    const W: f64 = 120.0;
    const H: f64 = 28.0;
    const PAD: f64 = 2.0;
    if values.is_empty() {
        return html! { svg .spark width=(W) height=(H) viewBox=(format!("0 0 {W} {H}")) role="img" aria-label="no data" {} };
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = if (max - min).abs() < f64::EPSILON { 1.0 } else { max - min };
    let n = values.len();
    let dx = if n > 1 { (W - 2.0 * PAD) / (n as f64 - 1.0) } else { 0.0 };
    let points = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = PAD + dx * i as f64;
            let y = PAD + (H - 2.0 * PAD) * (1.0 - (v - min) / range);
            format!("{x:.1},{y:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    html! {
        svg .spark width=(W) height=(H) viewBox=(format!("0 0 {W} {H}")) preserveAspectRatio="none" role="img" aria-label="trend sparkline" {
            polyline points=(points) fill="none" stroke="currentColor" stroke-width="1.5" vector-effect="non-scaling-stroke" {}
        }
    }
}

/// KPI stat card: big mono figure + optional unit, trend, and sparkline.
pub fn kpi_card(label: &str, value: &str, unit: Option<&str>, delta: Option<f64>, spark: Option<&[f64]>) -> Markup {
    html! {
        div .kpi-card {
            div .kpi-label { (label) }
            div .kpi-value {
                span .kpi-number { (value) }
                @if let Some(u) = unit { span .kpi-unit { (u) } }
            }
            div .kpi-foot {
                @if let Some(d) = delta { (trend_arrow(d)) }
                @if let Some(s) = spark { span .kpi-spark { (sparkline(s)) } }
            }
        }
    }
}

/// Inline horizontal bar for table cells. Fraction clamped to [0,1].
pub fn bar_cell(fraction: f64) -> Markup {
    let pct = (fraction.clamp(0.0, 1.0) * 100.0).round() as i64;
    html! {
        span .bar-cell { span .bar-fill style=(format!("width:{pct}%")) {} }
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run:
```bash
cd RailPredict && cargo test --lib frontend::charts 2>&1 | tail -12
```
Expected: `test result: ok. 5 passed`.

- [ ] **Step 6: Clippy clean**

Run:
```bash
cd RailPredict && cargo clippy --lib 2>&1 | grep -E "charts.rs|warning: " | head
```
Expected: no warnings referencing `charts.rs`.

- [ ] **Step 7: Commit**

```bash
git add RailPredict/src/frontend/charts.rs RailPredict/src/frontend/mod.rs
git commit -m "feat(ui): charts.rs inline-SVG helpers (kpi/sparkline/trend/bar)"
```

---

## Task 4: Upgraded nav — `NavPage`, active states, Dashboard link, SVG icon

**Files:**
- Modify: `RailPredict/src/frontend/layout.rs`
- Modify: every caller of `layout::base(` (found via grep)
- Test: inline test in `layout.rs`

- [ ] **Step 1: Add the `NavPage` enum + failing test in `layout.rs`**

At the top of `layout.rs` (after the `use` line), add:
```rust
/// Which top-level page is active — drives nav highlighting.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NavPage {
    Dashboard,
    Departures,
    Predictions,
    DevConsole,
    None,
}
```

Add at the bottom of `layout.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_page_is_marked() {
        let html = base("Test", NavPage::Predictions, maud::html! { p { "x" } }).into_string();
        assert!(html.contains("aria-current=\"page\""), "marks active link: {html}");
        // Dashboard link now exists
        assert!(html.contains(">Dashboard<") || html.contains("Dashboard"), "has dashboard link");
        // No emoji in the stale banner
        assert!(!html.contains('⚠'), "emoji replaced with svg");
    }
}
```

- [ ] **Step 2: Run the test to confirm it fails to compile**

Run:
```bash
cd RailPredict && cargo test --lib frontend::layout 2>&1 | tail -15
```
Expected: compile error — `base` takes 2 args, not 3.

- [ ] **Step 3: Update `base` signature, nav, and stale banner**

Replace the whole `pub fn base(...) -> Markup { ... }` body with:

```rust
pub fn base(title: &str, active: NavPage, content: Markup) -> Markup {
    let link = |href: &str, label: &str, page: NavPage| {
        let is_active = page == active;
        html! {
            a href=(href) class=(if is_active { "nav-link active" } else { "nav-link" })
                aria-current=[is_active.then_some("page")] { (label) }
        }
    };
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " — RailPredict" }
                link rel="preload" as="font" type="font/woff2" href="/static/fonts/IBMPlexMono-Regular.woff2" crossorigin;
                link rel="preload" as="font" type="font/woff2" href="/static/fonts/IBMPlexSans-Regular.woff2" crossorigin;
                link rel="stylesheet" href="/static/style.css";
                script src="https://unpkg.com/htmx.org@2.0.3" crossorigin="anonymous" {}
                script src="https://unpkg.com/htmx-ext-sse@2.2.2/sse.js" crossorigin="anonymous" {}
            }
            body {
                nav {
                    a .nav-brand href="/" {
                        span .nav-brand-dot {}
                        "RailPredict"
                    }
                    div .nav-links {
                        (link("/", "Dashboard", NavPage::Dashboard))
                        (link("/search", "Departures", NavPage::Departures))
                        (link("/predictions", "Predictions", NavPage::Predictions))
                        (link("/demo", "Dev Console", NavPage::DevConsole))
                    }
                }
                main { (content) }
                div #"stale-banner" .hidden."warning-banner" {
                    svg .warn-icon width="14" height="14" viewBox="0 0 24 24" fill="none"
                        stroke="currentColor" stroke-width="2" aria-hidden="true" {
                        path d="M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z" {}
                    }
                    " Live updates paused — showing last known state ("
                    span #"stale-timestamp" {}
                    ")"
                }
                script {
                    (PreEscaped(r#"
                        document.addEventListener('htmx:sseError', function() {
                            var banner = document.getElementById('stale-banner');
                            var ts = document.getElementById('stale-timestamp');
                            if (ts) ts.textContent = 'last seen ' + new Date().toLocaleTimeString();
                            if (banner) banner.classList.remove('hidden');
                            var live = document.getElementById('live-status');
                            if (live) live.classList.add('data-stale');
                        });
                        document.addEventListener('htmx:sseOpen', function() {
                            var banner = document.getElementById('stale-banner');
                            if (banner) banner.classList.add('hidden');
                            var live = document.getElementById('live-status');
                            if (live) live.classList.remove('data-stale');
                        });
                    "#))
                }
            }
        }
    }
}
```

Ensure the `use` line includes everything used: `use maud::{DOCTYPE, Markup, PreEscaped, html};` (already present).

- [ ] **Step 4: Find and update every `layout::base` caller**

Run:
```bash
cd RailPredict && grep -rn "layout::base(" src/
```
Expected call sites (update each, passing the matching `NavPage` as the new 2nd argument):
- `src/frontend/dashboard.rs` → `NavPage::Dashboard`
- `src/frontend/search.rs` → `NavPage::Departures`
- `src/frontend/predictions.rs` → `NavPage::Predictions`
- `src/frontend/demo.rs` → `NavPage::DevConsole`
- `src/frontend/detail.rs` → `NavPage::Departures` (detail is reached from departures)
- `src/api/handlers.rs` `report_handler` (if it calls `base`) → `NavPage::None`

For each, change `layout::base("Title", content)` to `layout::base("Title", crate::frontend::layout::NavPage::Dashboard, content)` (substituting the right variant). Add `use crate::frontend::layout::NavPage;` to each file's imports and use the short form `layout::base("Title", NavPage::Dashboard, content)`.

> If a caller does not import `layout` directly, match the existing call style. The signature change will surface every site as a compile error — fix until `cargo build` is clean.

- [ ] **Step 5: Build, then run the layout test**

Run:
```bash
cd RailPredict && cargo build 2>&1 | tail -5 && cargo test --lib frontend::layout 2>&1 | tail -8
```
Expected: build clean; `test result: ok. 1 passed`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(ui): active nav states + Dashboard link + SVG warn icon"
```

---

## Task 5: Component CSS + `time_range_picker`

**Files:**
- Modify: `RailPredict/static/style.css` (append a new component section)
- Modify: `RailPredict/src/frontend/components.rs`
- Test: inline test in `components.rs`

- [ ] **Step 1: Append the Signal Terminal component CSS**

Append to the END of `RailPredict/static/style.css`:

```css
/* ============================================================
   Signal Terminal — Phase 0 components
   ============================================================ */

.nav-brand { color: var(--accent); }
.nav-links .nav-link { color: var(--text-muted); padding: 0 0.85rem; height: 54px; display: inline-flex; align-items: center; border-bottom: 2px solid transparent; transition: color .18s ease, border-color .18s ease; }
.nav-links .nav-link:hover { color: var(--text); }
.nav-links .nav-link.active { color: var(--text); border-bottom-color: var(--accent); }

/* KPI strip */
.kpi-strip { display: grid; grid-template-columns: repeat(auto-fit, minmax(190px, 1fr)); gap: 14px; }
.kpi-card { background: var(--surface); border: 1px solid var(--border); border-radius: var(--r-md); padding: 14px 16px; display: flex; flex-direction: column; gap: 8px; }
.kpi-label { font-size: 12px; color: var(--text-muted); text-transform: uppercase; letter-spacing: .04em; }
.kpi-value { display: flex; align-items: baseline; gap: 4px; }
.kpi-number { font-family: var(--font-mono); font-variant-numeric: tabular-nums; font-size: 28px; font-weight: 600; line-height: 1; }
.kpi-unit { font-family: var(--font-mono); font-size: 14px; color: var(--text-muted); }
.kpi-foot { display: flex; align-items: center; justify-content: space-between; gap: 10px; min-height: 28px; }
.kpi-spark { color: var(--info); display: inline-flex; }

/* Trend indicator */
.trend { display: inline-flex; align-items: center; gap: 4px; font-family: var(--font-mono); font-variant-numeric: tabular-nums; font-size: 13px; }
.trend-up { color: var(--bad); }     /* more delay = bad by default */
.trend-down { color: var(--ok); }    /* less delay = good */
.trend-flat { color: var(--text-muted); }
/* Opt-in inversion for metrics where up is good (e.g. on-time %) */
.trend.good-up .trend-up { color: var(--ok); }
.trend.good-up .trend-down { color: var(--bad); }

/* Inline bar cell */
.bar-cell { display: inline-block; width: 100%; max-width: 120px; height: 8px; background: var(--surface-2); border-radius: var(--r-pill); overflow: hidden; vertical-align: middle; }
.bar-fill { display: block; height: 100%; background: var(--info); border-radius: var(--r-pill); }

/* Time-range picker */
.range-picker { display: inline-flex; border: 1px solid var(--border); border-radius: var(--r-md); overflow: hidden; }
.range-picker button { background: var(--surface); color: var(--text-muted); border: none; padding: 6px 12px; font: inherit; font-size: 13px; cursor: pointer; border-right: 1px solid var(--border); transition: background .15s ease, color .15s ease; }
.range-picker button:last-child { border-right: none; }
.range-picker button:hover { color: var(--text); }
.range-picker button.active { background: var(--accent-dim); color: var(--accent); }

@media (prefers-reduced-motion: reduce) {
  .nav-link, .range-picker button, .kpi-card { transition: none !important; }
}
```

- [ ] **Step 2: Add the `time_range_picker` component + failing test in `components.rs`**

At the bottom of `components.rs` add the failing test first:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_picker_marks_active_and_targets_path() {
        let m = time_range_picker("/", "7d").into_string();
        assert!(m.contains("range-picker"));
        assert!(m.contains("hx-get=\"/?range=24h\""));
        assert!(m.contains("hx-get=\"/?range=7d\""));
        // active one carries the class
        let active = m.split("range=7d").nth(1).unwrap_or("");
        assert!(m.contains("class=\"active\"") || active.contains("active") || m.contains(" active"));
    }
}
```

- [ ] **Step 3: Run it to confirm failure**

Run:
```bash
cd RailPredict && cargo test --lib frontend::components 2>&1 | tail -12
```
Expected: compile error (`time_range_picker` undefined).

- [ ] **Step 4: Implement `time_range_picker`**

Add to `components.rs` (above the test module):
```rust
/// Global time-range picker. `base_path` is the page it re-scopes (htmx swaps
/// the page `<main>`), `active` is one of "24h" | "7d" | "30d" | "all".
pub fn time_range_picker(base_path: &str, active: &str) -> Markup {
    let ranges = [("24h", "24h"), ("7d", "7d"), ("30d", "30d"), ("all", "All")];
    html! {
        div .range-picker role="group" aria-label="Time range" {
            @for (val, label) in ranges {
                button
                    class=(if val == active { "active" } else { "" })
                    hx-get=(format!("{base_path}?range={val}"))
                    hx-target="main"
                    hx-push-url="true"
                    aria-pressed=(if val == active { "true" } else { "false" })
                    { (label) }
            }
        }
    }
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run:
```bash
cd RailPredict && cargo test --lib frontend::components 2>&1 | tail -8
```
Expected: `test result: ok`.

- [ ] **Step 6: Commit**

```bash
git add RailPredict/static/style.css RailPredict/src/frontend/components.rs
git commit -m "feat(ui): Signal Terminal component CSS + time-range picker"
```

---

## Task 6: Prove the system on the dashboard + version bump

**Files:**
- Modify: `RailPredict/src/frontend/dashboard.rs`
- Modify: `CLAUDE.md`, `README.md`, `TODO.md`, `CHANGELOG.md`, `RailPredict/Cargo.toml`

- [ ] **Step 1: Read the current dashboard to find the insertion point**

Run:
```bash
cd RailPredict && sed -n '1,80p' src/frontend/dashboard.rs
```
Identify where the hero/metric grid is rendered and where `layout::base(...)` wraps it.

- [ ] **Step 2: Render a KPI strip using `charts.rs` near the top of the dashboard content**

Add `use crate::frontend::charts;` to the imports, and insert this block at the start of the dashboard's main content (inside the maud `html! {}` that becomes `content`, before the existing hero/metrics):

```rust
div .dash-header style="display:flex;align-items:center;justify-content:space-between;gap:16px;margin-bottom:18px;flex-wrap:wrap;" {
    h1 style="font-size:20px;font-weight:600;" { "Network Overview" }
    (crate::frontend::components::time_range_picker("/", "7d"))
}
div .kpi-strip style="margin-bottom:22px;" {
    (charts::kpi_card("On-time", "92.4", Some("%"), Some(1.1), Some(&[88.0,89.5,90.1,91.0,92.4])))
    (charts::kpi_card("Avg delay", "2.6", Some("min"), Some(-0.4), Some(&[3.4,3.1,2.9,2.7,2.6])))
    (charts::kpi_card("Prediction MAE", "4.06", Some("min"), Some(-0.1), Some(&[4.3,4.2,4.1,4.1,4.06])))
    (charts::kpi_card("Trains tracked", "1,284", None, Some(36.0), None))
}
```

> These are placeholder literals — Phase 2 wires them to live queries. The goal of Task 6 is purely to prove the design system renders end-to-end. Do NOT add new DB queries here.

- [ ] **Step 3: Build and visually verify**

Run:
```bash
cd RailPredict && cargo build 2>&1 | tail -3
```
Expected: clean build. Then launch and screenshot using the `run` skill (or `cargo run` + open `http://localhost:<port>/`) and confirm: amber nav active underline on "Dashboard", IBM Plex Mono numerals, KPI cards with sparklines, time-range picker. Confirm no console errors and the stale-banner icon is an SVG.

- [ ] **Step 4: Run the full test + clippy suite**

Run:
```bash
cd RailPredict && cargo test 2>&1 | tail -15 && cargo clippy --all-targets 2>&1 | tail -5
```
Expected: all tests pass; no new clippy warnings.

- [ ] **Step 5: Version bump to 1.12.5 across all five files**

Update the version header `**version = "1.12.4" -- 31/05/2026**` → `**version = "1.12.5" -- 03/06/2026**` in `CLAUDE.md`, `README.md`, `TODO.md`; bump `version = "1.12.4"` → `"1.12.5"` in `RailPredict/Cargo.toml`; and add a `CHANGELOG.md` entry:
```markdown
## [1.12.5] — 2026-06-03
### Added
- Dashboard overhaul **Phase 0 (Foundation)**: "Signal Terminal" design system — IBM Plex Mono/Sans, amber terminal accent with green/amber/red reserved for punctuality semantics, vendored uPlot, `frontend/charts.rs` inline-SVG helpers (KPI cards, sparklines, trend arrows, bar cells), upgraded nav with active states + Dashboard link, and a global time-range picker.
```

- [ ] **Step 6: Confirm Cargo.lock + build still consistent and commit**

```bash
cd RailPredict && cargo build 2>&1 | tail -2
git add -A
git commit -m "feat(ui): prove Signal Terminal on dashboard; bump v1.12.5"
```

---

## Self-Review

- **Spec coverage:** charts.rs (§4.5, scoped subset), design tokens + IBM Plex (§4.1/§4.2), nav active states + Dashboard link (§4.3), emoji→SVG (§4.3), time-range picker shell (§4.3), vendored uPlot (§2/§4.5), dark-only OLED surfaces (§4.1), reduced-motion (§4.4), versioning protocol (§3) — all covered. Operator data, the new pages, and the remaining chart helpers are explicitly deferred to Phases 1–5 per the spec's phasing (§7).
- **Placeholder scan:** the only literal placeholders are the dashboard KPI demo numbers in Task 6, which are explicitly labelled as proof-of-system and deferred to Phase 2 wiring — not a plan gap.
- **Type consistency:** `NavPage` variants, `base(title, NavPage, Markup)` signature, and the four `charts.rs` function signatures are referenced identically across Tasks 3–6.
