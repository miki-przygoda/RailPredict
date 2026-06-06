# RailPredict OS — Demo Redesign Design Spec

**Status:** Approved (concept + key decisions signed off 2026-06-06). Ready for staged
implementation plans, starting with Stage 1.

## Goal

Replace the scrollytelling demo (v1.17.x) with **"RailPredict OS"** — a polished, fully
offline, **macOS-style desktop** that presents RailPredict as a *suite of apps* (windows),
with a **live GB delay-map command-centre** as the flagship. It is the sales/handover
showpiece for the CEO of a ticket retailer; the map renderer is reused on a live `/map`
dashboard page so the same engine serves demo and product.

## Audience & context

Shown to a numbers-loving, visuals-driven CEO during an IP/codebase sale. The desktop
metaphor reframes RailPredict from "a delay viz" into a **platform/suite** — each window a
sellable capability — which suits an IP purchase. The user has confirmed a polished,
restrained macOS-grade execution will read as confident, not gimmicky.

## Decisions (locked)

- **Form:** a browser desktop OS — wallpaper, menu bar + clock, desktop folder/app icons,
  dock, draggable windows with traffic-light minimise / maximise / close.
- **Style:** macOS-grade — light, clean, premium, restrained; on-brand blue accent.
- **Replaces** the scrollytelling demo entirely.
- **Full suite** of apps (six, below); **Live Map** is the flagship and auto-opens maximised
  on boot so the wow lands immediately.
- **Map engine:** D3 + TopoJSON (fully offline, free, small, single-file-friendly). *Not*
  Mapbox/MapLibre — those need tiles/keys/network and break offline. (See research in the
  2026-06-06 brainstorm.)
- **Offline:** no network, no API key, no CDN at view time. Vanilla JS only (no framework).
- **Built by** an expanded `export-demo` that bakes shell + apps + data into the bundle.

## The apps (windows)

1. **Live Map** *(flagship)* — the command-centre: tall portrait GB map (delay-coloured
   train dots moving along routes), compact predicted→actual banners on the left, KPIs +
   plain-English "what you're seeing" on the right. Auto-opens maximised on boot.
2. **Operators** — the operator league (`db::operators`).
3. **Predictions** — the predicted-vs-actual accuracy explorer.
4. **Reliability** — station / journey reliability (`db::stations`, `db::overview`).
5. **Replay** — the predicted→actual board (today's replay, as a windowed app).
6. **About RailPredict** — the value story + "what your data unlocks" (the retired scroll
   demo's narrative, condensed into a read-me window).

## Architecture

- **Generator:** the expanded `export-demo` CLI queries the DB and bakes one offline bundle:
  the desktop **shell** (HTML/CSS + a small vanilla-JS window manager), each **app view**,
  and a single **data payload** (KPIs, replay frames, operators, predictions, reliability,
  and TIPLOC→lat/lon for the map). D3 + TopoJSON + topojson-client are inlined.
- **Shell components:** wallpaper; menu bar + clock; desktop icons/folders; dock; a window
  manager owning open/close/minimise/maximise/drag/focus/z-order and boot behaviour
  (auto-open Map maximised). One focused module; no framework.
- **App model:** each app is a self-contained view rendered into a window from baked
  HTML + the data payload. Apps are cheap to add (one registry entry + a view fn).
- **Shared map renderer:** a single D3 + TopoJSON module ("one renderer, two drivers" — the
  pattern already used for the live board ↔ replay). Driven by **baked frames** in the demo
  Map app, and by the **live snapshot feed** on the dashboard `/map` page.
- **Offline packaging:** single self-contained HTML preferred. If the full suite (D3 ~250 KB
  + GB TopoJSON ~300 KB + six app views + data) is uncomfortably large for one file, fall
  back to a small static folder — still fully offline (no network), just multiple local
  files. Decide at build time; default to single-file while it stays reasonable.

## Data

- Extend the demo gather to bake: KPIs, replay frames, operator highlights, prediction
  accuracy, reliability metrics (reusing existing `db::*` queries), plus a **TIPLOC→lat/lon**
  lookup limited to the codes that actually appear in the baked data.
- **Coordinate sourcing (one-time loader — Stage 1):** join **NaPTAN RailReferences**
  (TIPLOC→CRS, OGL v3) to our existing `stations` CRS→lat/lon; fall back to
  **fasteroute/national-rail-stations** `stations.json` (TIPLOC→lat/lon, OGL) for codes
  NaPTAN misses; OSM Overpass `ref:tiploc` for remaining gaps. Persist as a `tiploc_coords`
  lookup (DB table or committed JSON). Trains whose origin/dest can't be resolved are
  omitted from the map (logged), not faked.

## Aesthetic

macOS-grade: light desktop, subtle wallpaper, traffic-light window controls, soft window
shadows, a dock with gentle depth, clean typography. Restrained and premium — never
cartoonish. Blue on-brand accent; delay colours (green/amber/red) reserved for data.

## Build decomposition (each stage = its own implementation plan)

1. **TIPLOC→coordinate data** — the one-time loader + `tiploc_coords` lookup.
2. **Map renderer + command-centre** — the shared D3 + TopoJSON module + the flagship view,
   standalone and offline-renderable (verifiable before any OS chrome).
3. **OS shell + Map app** — desktop, dock, window manager; boots to the flagship Map window.
   First visible "OS" milestone.
4. **Remaining apps** — Operators, Predictions, Reliability, Replay, About, each baked as a
   window.
5. **Live `/map` dashboard page** — the shared renderer wired to the live snapshot feed,
   added to the product nav.

Stages ship in order; each is usable on its own. Stage 1 is the immediate start.

## Licensing (all free for commercial; attribution in the OS "About")

D3 (ISC), TopoJSON / topojson-client (ISC), GB TopoJSON basemap (OGL/CC — attribution),
NaPTAN (OGL v3), fasteroute station data (National Rail OGL), OSM (ODbL — attribution).

## Out of scope (for now)

- Street-level realism (MapLibre + PMTiles) — deferred; revisit only if offline/single-file
  is relaxed and tiles are self-hosted.
- A true small-screen/mobile desktop metaphor — degrade to a simple app launcher on narrow
  viewports; low priority (demo is shown on a laptop).

## Testing

- **Rust/unit:** the coordinate join + any pure data-shaping logic; the expanded gather.
- **In-browser (manual checklist):** window open/close/min/max/drag/focus, boot-to-Map, the
  map renderer animation, each app renders with real baked data, and an **offline check**
  (loads with the network off; no external requests).
- Each stage carries its own tests; the shared map renderer is exercised by both the demo
  Map app and the live `/map` page.
