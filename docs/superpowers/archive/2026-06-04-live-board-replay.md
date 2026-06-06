# Live Board + Replay — Design Spec

**Date:** 2026-06-04
**Branch:** `ui/page-improvements`
**Status:** Approved (mockup signed off); mechanics spec for build
**Companion:** memory `live-board-replay-feature`; mockup `live.html`

## 1. Goal

An ambient, good-looking `/live` board that shows each train's **prediction as it appears** and its **predicted-vs-actual as it settles** — glanceable enough to leave running, useful enough to read per train/operator/route. Plus a **capture → standalone replay** path so the exact board can be replayed with **no server, no Darwin, no model**.

## 2. The architecture that ties it together

**One renderer, two data sources.** The board is rendered by a single vanilla-JS module (`board.js`) that consumes **snapshots**. It is fed either:
- **Live:** `board.js` polls `GET /ui/live/snapshot` every ~3 s for the current board JSON.
- **Replay:** `replay.js` feeds recorded snapshots from a JSON file on a clock.

Both call the same `renderBoard(snapshot)`, so live and replay look identical. The `/live` page is a maud shell; the board *content* is JS-rendered from JSON — a deliberate, documented exception to "htmx-only", required so the replay can run server-free. No build step (plain ES module loaded with `<script>`).

**Why polling, not SSE (v1):** a 3 s JSON snapshot gives a smooth ambient feel, fits capture/replay trivially (record the snapshots), and avoids deep ingestion surgery. SSE can be a later enhancement; the renderer wouldn't change.

## 3. Snapshot schema (the contract)

`GET /ui/live/snapshot` → JSON:

```json
{
  "t": 1780600000,
  "tracking": [
    { "rid": "...", "headcode": "1A23", "toc": "VT", "operator": "Avanti West Coast",
      "brand": "#d70428", "origin": "EUS", "dest": "MAN", "scheduled": "14:30",
      "predicted": 8, "confidence": 0.82 }
  ],
  "settled": [
    { "rid": "...", "headcode": "9C12", "toc": "GR", "operator": "LNER", "brand": "#ce0e2d",
      "origin": "KGX", "dest": "EDB", "predicted": 5, "actual": 7, "delta": 2 }
  ]
}
```

- `tracking` = active trains in the registry that carry a prediction (cap ~24, soonest-departing first).
- `settled` = most recent finalised `prediction_outcomes` (cap ~12, newest first), `delta = |predicted − actual|`.
- `operator`/`brand` fall back to the TOC code and neutral grey when `services.toc` is unpopulated (so the board works now; brand fills in after backfill). `predicted`/`actual` are integer minutes; `predicted` may be negative (early).

A **capture file** is `{ "meta": { "captured_at", "app_version" }, "frames": [ <snapshot>, ... ] }`.

## 4. Components

| Piece | Location | Responsibility |
|---|---|---|
| `/live` page | `frontend/live.rs` (new) | maud shell: header (live pill, filter chips, Record button), empty `#board`, loads `board.js`. Server-renders the *first* snapshot for instant paint. |
| Snapshot API | `api` route `GET /ui/live/snapshot` → handler in `live.rs` | Builds the snapshot JSON from the registry (tracking) + `predictions::recent_predictions` (settled). |
| Registry access | `cache/train_registry.rs` | A read method returning active trains with prediction + identity for the tracking list (extend `network_summary` or add `tracking_board(limit)`). |
| `board.js` | `static/board.js` | `renderBoard(snapshot)` (diff + animate cards into Tracking / Just-settled); poll loop; client-side **Record** (buffer frames) + **Download** (emit capture JSON); filter chips. |
| `replay.html` | `static/replay.html` | Standalone page: file-picker (or embedded sample), play/pause/speed, `#board`; loads `board.js` + `replay.js`. **No server calls.** |
| `replay.js` | `static/replay.js` | Load capture JSON → play `frames` on a clock → `renderBoard` each frame. |
| Board CSS | `static/style.css` | `.tcard` etc. (from the approved mockup). Shared by `/live` and `replay.html`. |
| Nav | `frontend/layout.rs` | Add **Live** (`NavPage::Live`). |

## 5. Filtering

Client-side in `board.js`: filter chips (All / per-operator / from-origin) hide non-matching cards. Operates on the rendered snapshot; no server round-trip. Works in replay too.

## 6. Testing

- HTTP smoke: `GET /live` → 200 (shell + `#board`); `GET /ui/live/snapshot` → 200, `content-type: application/json`, parses, has `tracking`/`settled` arrays.
- Rust unit: snapshot builder maps a registry train + a finalised outcome into the schema (delta = |pred−act|; operator fallback).
- `board.js`/`replay.js`: no Rust test harness; keep them small and defensive. Manual: load `replay.html` with a captured JSON, confirm it plays with the network throttled / server stopped.
- `cargo clippy -D warnings` + full suite stay green.

## 7. Out of scope (v1)

- SSE (polling is enough for the ambient feel; revisit later).
- Server-side recording (capture is client-side per the locked decision).
- Operator brand on cards before the `services.toc` backfill (graceful TOC/grey fallback).
- Persisting captures server-side — Download gives a file the user keeps.

## 8. Risks

- **Snapshot field availability.** `prediction_outcomes` may not carry origin/dest/headcode for settled cards — confirm during build; degrade gracefully (show rid/uid + pred/actual) rather than block.
- **Empty live data.** If the Darwin feed is quiet, Tracking is sparse — render a calm empty state, not a broken board.
- **Replay realism.** Replay must look identical to live → keep ALL board markup/CSS in the shared renderer, never server-only.
