# RailPredict Demo Site — Design Spec

**Status:** Approved (storyboard signed off 2026-06-06) — ready for the implementation plan.

## Goal

A single, self-contained, **static demo site** that sells RailPredict to the CEO of a UK
rail **ticket retailer** (multiple TOCs + Uber transit booking). It must impress a
numbers-loving, visuals-driven exec by (a) proving the engine works on real live-rail data
and (b) showing the customer-experience value his ticketing data would unlock — honestly,
without overclaiming. Doubles as a credibility artifact for an IP/codebase sale.

## Audience & context

- **Buyer:** a rail ticket retailer/distributor — sells across several UK TOCs and powers
  Uber's train booking. Their edge is sitting on ticketing data + Darwin + GBR data.
- **Deal:** sell the IP/codebase; the author then joins to expand it. The artifact must
  impress enough to buy + hire, and be clean enough to hand over.
- **Demo moment:** shown to the CEO in a pitch / due-diligence setting. Must land the
  punchline fast and read as a polished product, not a portfolio piece.
- **The honesty boundary:** the killer value (delay × ticketing → CX) needs the buyer's
  ticketing data, which we do not hold. The demo proves the engine on our **real**
  Darwin-derived data, and frames ticketing-CX strictly as **labelled projected vision**.

## Decisions (locked)

- **Format:** scrollytelling — one long, self-contained page; the story reveals on scroll.
  (Not a deck, not a tabbed mini-site.)
- **Look & feel:** corporate-neutral / **exec** — light theme, generous whitespace, a blue
  accent. Distinct from the product's dark "Signal Terminal" UI; a **new theme**, not a
  reuse of `static/style.css`.
- **Honesty model:** real data is shown as real; every projected/illustrative figure is
  unmistakably badged **"PROJECTED · with your data"** and footnoted "modelled, not measured".
- **Delivery:** self-contained static artifact (CSS/JS/data all inlined). Shareable as a
  link or a single file; runs with no server, DB, or Darwin connection.

## The narrative — 6 beats

Legend: **real** = our live data · **projected** = labelled vision · **product** = the offer.

1. **Hero** (real) — positioning line ("Every delay is a customer moment.") + one big real
   headline number from the live corpus.
2. **The gap** (framing) — a ticket retailer sells millions of journeys; disruption is a
   refund / complaint / trust event learned about *after* the customer. The lag is the
   opportunity.
3. **Proof — the engine works** (real) — live KPIs (departures analysed, on-time %,
   prediction MAE, % within 5 min) + operator league + route reliability, from the running
   system.
4. **Watch it work — the replay** (real, centrepiece) — auto-playing predicted→actual board:
   trains appear with a prediction, then settle to the actual + an accuracy chip. A real
   recorded window, baked in, plays on a clock; auto-starts when scrolled into view.
5. **With your ticketing data →** (projected) — labelled illustrative CX scenarios: tickets
   sold on routes predicted to be disrupted → proactive notify/rebook; point-of-sale
   reliability; automated Delay Repay; churn saved. Delay data → CX → revenue.
6. **Own it + the ask** (product) — Docker appliance out of the box; ingests Darwin + your
   ticketing + GBR; retrains on your data. "Buy the IP, I build it out with you." + contact.

Notes: beat 5's figures are illustrative-but-concrete, each carrying the PROJECTED badge and
the "modelled, not measured" footnote. Beat 3 may later split into headline-KPIs +
reliability-deep-cut — it is **built as one section first** and trimmed/split after review,
per the "build then clean up, possibly remove sections" plan.

## Architecture

Mirrors the existing `export-site` pattern (`src/export/`), which already bakes real data
into a single self-contained HTML via a template + a JSON literal.

- **New CLI subcommand `export-demo`** (added to `src/cli.rs`, dispatched in `src/main.rs`)
  → `src/export/demo.rs::export_demo(db, output_path, days)`.
- **Data sources (all real, read-only):**
  - KPIs / daily / accuracy: reuse `export`'s summary queries (`delay_history` +
    `prediction_outcomes`).
  - Operator league + reliability: summaries from `db::operators`, `db::overview`,
    `db::stations`.
  - **Replay frames:** reconstructed at export time from real settled predictions
    (`prediction_outcomes` joined to `journeys`) into a predicted→actual frame timeline, so
    the baked replay is real and refreshes on every export. **Fallback** (if reconstruction
    proves fiddly): bake a single real capture JSON recorded once from the live `/live`
    board. Primary path is DB reconstruction.
- **Template & theme:** a new `src/export/demo_template.html` with an inlined light/exec CSS
  theme (separate from `style.css`) and inlined board-render + scrollytelling JS. Everything
  inlined → one self-contained file.
- **Charts:** prefer inline SVG / a tiny inlined helper over a CDN so the file is fully
  offline. (The older `export-site` used a Chart.js CDN; the demo should not depend on the
  network.)
- **Board renderer:** adapt the render logic from `static/board.js` into the inlined,
  light-themed renderer so the live board and the demo replay stay structurally consistent.
- **Output:** a single file, default `docs/demo.html` (overridable via `--output`).

## Data honesty rules

- Real figures are pulled live at export; never hard-coded.
- Projected/illustrative numbers (beat 5) are visually badged "PROJECTED" and carry a
  one-line "modelled, not measured" note.
- No synthetic data is presented as real; the replay uses real settled outcomes only.

## Out of scope (follow-ups)

- Docker / handover polish (already a good standard) — a separate, lighter track.
- Post-build trim: cut/merge sections after the first full build (the user's stated plan).
- Real ticketing integration (needs the buyer's data) — product roadmap, not the demo.

## Testing

- **Unit:** replay-frame reconstruction from sample outcome rows (ordering, predicted→actual
  transition, accuracy bucketing); KPI/summary query shaping.
- **Build/integration:** `export-demo` produces a non-empty self-contained file with all
  assets inlined and valid JSON literals, and opens with no network/DB access.
- **Manual:** scroll-reveal + replay auto-start verified in a browser; PROJECTED labels
  present on every illustrative figure.
