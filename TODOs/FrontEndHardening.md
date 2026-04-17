# Frontend Hardening — The "Instant" UX

_Current state: maud server-render + htmx SSE works. The goal here is to make the UI_
_feel native: no blank flashes, honest staleness signals, graceful degradation._

---

## Phase 1 — Stale Data Overlay

### Implementation Status
**Complete** — implemented 17/04/2026.
- `src/api/types.rs`: added `last_updated_secs_ago: Option<u64>` and `destination_name: Option<String>` to `DepartureBoardEntry` (with `#[serde(default)]` for backwards compat).
- `src/frontend/search.rs` (`departures_fragment`): computes `last_updated_secs_ago` from the max `last_updated` across `actual_estimated_departure`, `reported_delay_mins`, `actual_platform`, `is_cancelled`.
- `src/frontend/search.rs` (`departure_board_fragment`): emits `data-stale="true"` attribute on cards older than 120 s.
- `src/api/handlers.rs` (`departures_handler`): same staleness computation for the JSON API path.
- `static/style.css`: added `.train-card[data-stale="true"]` grayscale + "stale" pseudo-element styles.
- Deviation: used inline `chrono::Utc::now()` comparison rather than `Stamped::is_stale()` because we need the raw seconds value for the DTO field, not just a boolean.

### The problem
Every `TrainStatus` field is wrapped in `Stamped<T>` which carries a `last_updated`
timestamp and an `is_stale(max_age)` method. None of this is surfaced in the UI.
A train card on the departure board could be showing data that is 3 minutes old with
no visual indication — indistinguishable from live data.

### What needs to change

**Step 1: Add `last_updated` to `DepartureBoardEntry`.**
`DepartureBoardEntry` currently has no timestamp field. Add:
```rust
pub last_updated_secs_ago: Option<u64>,
```
Populate it in `departures_handler` and `departures_fragment` from the most recent
`last_updated` among the relevant `Stamped` fields on `TrainStatus`.

**Step 2: Emit a `data-stale` attribute on cards older than 120 seconds.**
In `search.rs::departure_board_fragment`, add a conditional attribute:
```rust
@let stale = entry.last_updated_secs_ago.map_or(false, |s| s > 120);
div .train-card[stale] data-stale=[stale] { ... }
```

**Step 3: Add CSS for the stale state.**
In `static/style.css`, add:
```css
.train-card[data-stale="true"] {
    filter: grayscale(60%);
    opacity: 0.75;
}
.train-card[data-stale="true"]::after {
    content: "stale";
    font-size: 0.65rem;
    color: var(--delay-amber);
    margin-left: 0.5rem;
}
```

**Note:** `Stamped::is_stale()` already exists in `src/types/train_status.rs` — use it
rather than re-implementing the comparison.

---

## Phase 2 — Optimistic Departure Board

### Implementation Status
**Complete** — implemented 17/04/2026.
- `src/frontend/search.rs` (`search_page`): changed form `hx-trigger="submit"` to `hx-trigger="submit, every 30s"` and added `hx-swap="innerHTML transition:true"`.
- No session handling required (Option B from spec).
- No deviations.

### The problem
When a user submits the station search form, the page shows nothing until the htmx
request completes. On slow connections this is a visible blank period.

### The htmx-native solution
htmx doesn't have built-in optimistic updates (showing cached content before a request
completes), but it can be approximated using two mechanisms:

**Option A: `hx-indicator` with a preserved skeleton.**
Add a `<div id="results-skeleton" class="skeleton-board">` below the search input that
is pre-populated server-side with the last-seen result (stored in the user's session or
a cookie). The `hx-indicator` attribute shows a spinner overlay on top while refreshing,
rather than replacing the board with a blank.

**Option B: Auto-refresh with preserved content.**
Change `hx-trigger="submit"` to `hx-trigger="submit, every 30s"` on the search form and
use `hx-swap="innerHTML transition:true"`. This keeps the board visible and fades in
each refresh, so the departure board stays populated at all times.

Option B is simpler and requires no session handling. Start here.

---

## Phase 3 — SSE "Live Updates Paused" Banner

### Implementation Status
**Complete** — implemented 17/04/2026.
- `src/frontend/layout.rs`: updated `#stale-banner` div to include `warning-banner` class and the "⚠ Live updates paused" text with a `#stale-timestamp` span; expanded the inline JS to also add/remove `data-stale` class on `#live-status` on `htmx:sseError` / `htmx:sseOpen`.
- `static/style.css`: added `.warning-banner` background/color/text-align rules and `#live-status.data-stale` grayscale + amber left-border rules.
- Deviation: kept the existing `display:none` / `.hidden` toggle mechanism (CSS class) rather than the spec's `style="display:none"` inline toggle, since the existing CSS already defines `#stale-banner.hidden { display: none }`. The JS removes/adds the `.hidden` class rather than setting `style.display` directly.

### The problem
When the SSE connection drops (network interruption, STOMP disconnect, circuit breaker
entering Cache Only mode), `div#live-status` on the detail page freezes on the last
received update. There is no visual signal that data is stale. The banner slot in
`layout.rs` exists but is always hidden.

### What needs to change

**Step 1: Wire the htmx SSE error handler in `detail.rs`.**
The live section div already has `hx-ext="sse"` and `sse-connect`. Add a short inline
script (or a handler in a `static/app.js` file embedded via rust-embed) that reacts to
htmx SSE lifecycle events:

```javascript
htmx.on("htmx:sseError", function(evt) {
    document.getElementById("stale-banner").style.display = "block";
    document.getElementById("live-status").classList.add("data-stale");
});
htmx.on("htmx:sseOpen", function(evt) {
    document.getElementById("stale-banner").style.display = "none";
    document.getElementById("live-status").classList.remove("data-stale");
});
```

**Step 2: Make the banner in `layout.rs` functional.**
`layout.rs` renders a hidden `#stale-banner` div (per the existing slot). Give it
visible content:
```rust
div #"stale-banner" style="display:none" .warning-banner {
    "⚠ Live updates paused — showing last known state"
}
```

**Step 3: CSS for the paused state.**
In `static/style.css`:
```css
.warning-banner {
    background: var(--delay-amber);
    color: #000;
    text-align: center;
    padding: 0.5rem;
    font-size: 0.85rem;
}
#live-status.data-stale {
    filter: grayscale(40%);
    border-left: 3px solid var(--delay-amber);
}
```

**Why this matters:** The `CircuitBreaker` on the backend already detects GBR failure and
enters Cache Only mode. The STOMP auto-reconnect (Improvements.md item 1.2) handles
Darwin disconnects. But if either of these trips, the user currently sees nothing.
This banner closes that loop — it's the client-side complement to the backend health
machinery.

---

## Phase 4 — Destination Station on Departure Board Cards

### Implementation Status
**Complete (with one known limitation)** — implemented 17/04/2026.
- `src/types/train_status.rs`: added `pub destination_crs: Option<String>` field after `origin_crs`; initialised to `None` in `TrainStatus::new`.
- `src/ingestion/parser.rs` (`TsUpdate`): added `destination_crs: Option<String>` field; initialised to `None`; updated the `Location` handler to always overwrite `destination_crs` with the current `tpl` attribute so that after all Location elements are processed, it holds the last (destination) station CRS.
- `src/ingestion/mod.rs`: captures `ts_destination_crs` before the registry update closure; propagates it to `status.destination_crs` inside the closure; also sets it on new-train registration.
- `src/api/types.rs`: added `destination_name: Option<String>` to `DepartureBoardEntry`.
- `src/frontend/search.rs` (`departures_fragment`): maps `status.destination_crs` directly to `destination_name` (CRS as fallback string, not resolved station name).
- `src/api/handlers.rs` (`departures_handler`): same — emits raw CRS until DB is wired.
- `static/style.css`: added `.train-destination` style (muted, 0.9rem).
- `src/frontend/search.rs` (`departure_board_fragment`): renders `span .train-destination { "→ " (dest) }` in the card left column.
- Known limitation: `AppState` does not expose `db: Db`, so `db::static_data::get_station` cannot be called. `destination_name` is populated with the raw CRS code (e.g. "MAN") until `AppState::db` is wired. Both `departures_fragment` and `departures_handler` have `// TODO: wire AppState::db` comments marking the integration point.

### The problem
Each departure card shows the scheduled time, delay badge, and platform — but not where
the train is going. A user at Leeds looking at 5 departures at 12:00 has no way to know
which one is theirs without clicking through to each detail page.

### What needs to change

**Step 1: Add `destination_crs` to `TrainStatus`.**
Populate it from the last `<Location>` element in the Darwin TS message (the call with
the highest `call_order`, or where `at` is the last entry). Currently `parser.rs`
processes only the first matching location.

**Step 2: Surface it in `DepartureBoardEntry`.**
```rust
pub destination_name: Option<String>,
```
Populate via a lookup in `db::static_data::get_station(db, &destination_crs)`.

**Step 3: Render it on the card in `search.rs`.**
```rust
@if let Some(dest) = &entry.destination_name {
    span .train-destination { "→ " (dest) }
}
```

---

## Priority Order

1. Phase 3 (SSE error banner) — smallest change, directly improves honesty of the UI
2. Phase 1 (stale data overlay) — `Stamped` already exists, just needs surfacing
3. Phase 4 (destination on cards) — high UX value, requires parser + DTO change
4. Phase 2 (optimistic board) — polish; do last
