# TODOs/UI.md – HTTP API Layer & Rust Frontend

Two sub-epics that must be built in sequence: the axum API server first (it defines
the contract), then the Rust-rendered frontend that consumes it.

**Frontend stack decision:** pure Rust. No Node, no npm, no bundler.
- `maud` for server-side HTML templating (compile-time checked macros)
- `htmx` (single CDN script tag, no build step) for partial-page updates over SSE and AJAX
- Axum serves both the API and the HTML pages from the same binary
- CSS via a single hand-written stylesheet (no Tailwind / PostCSS toolchain)
- Result: one `cargo build` produces a fully self-contained binary

---

## Sub-Epic A — axum HTTP API

### Key considerations:

1. **SSE over WebSocket**
   The data flow is server-push only — the client never sends runtime updates back.
   Server-Sent Events are simpler (plain HTTP, no upgrade handshake, automatic
   reconnect built into `htmx`'s `hx-ext="sse"`). Reserve WebSocket for a
   future bidirectional feature (e.g. seat reservation).

2. **API contract before implementation**
   Define request/response types as Rust structs with `serde::Serialize` before writing
   any handler. These structs are the contract the frontend templates will render against.
   Changing them later is a two-location change (handler + template).

3. **The three-tier rule at the API boundary**
   - `GET /trains/{rid}` must serve from the registry (Tier B/C) without hitting GBR
     unless the registry has no entry. The handler must never call GBR directly — it
     goes through the coalescer.
   - `GET /stations/{crs}/departures` is Tier A: served from the static schedule, zero
     live calls on the search path.
   - `GET /trains/{rid}/live` (SSE) subscribes the caller to state-change events for
     one RID. It reads from the `state_change_tx` broadcast — it does not poll.

4. **Dual response format**
   Every data endpoint returns either JSON (`Accept: application/json`) or an HTML
   fragment (`Accept: text/html`). The HTML path returns a `maud` fragment for htmx
   to swap in; the JSON path is for direct API consumers and tests. A small
   `accept_header` extractor decides which branch to take.

---

### Before Starting: Pre-work Subtasks (Sub-Epic A)

- [ ] **Cargo.toml: add API dependencies** — `axum` (with `macros` feature), `tower`,
  `tower-http` (with `cors` and `trace` features), `tokio-stream` (for SSE body
  streaming), `maud` (with `axum` feature). Check version compatibility with the
  existing `tower` pulled in transitively.

- [ ] **Write the API contract as a markdown table before coding** — for every endpoint,
  document: HTTP method, path, path/query params, success response shape (field names
  and types), error response shape, which data tier it serves from, and whether it is
  synchronous or streaming. Store it as a comment block at the top of `src/api/mod.rs`.

- [ ] **Decide the SSE event format** — htmx's `hx-ext="sse"` expects named events with
  an `id:` line for reconnect. Decide whether the event body is raw HTML (an htmx OOB
  swap fragment) or JSON parsed by a small inline `<script>`. Raw HTML fragments are
  simpler and keep all rendering in Rust/maud.

- [ ] **Design error response shape** — a consistent `{ "error": "...", "code": "..." }`
  JSON envelope for all 4xx/5xx responses, plus a matching maud error fragment for
  htmx consumers. Define as a type implementing `axum::response::IntoResponse`.

- [ ] **Confirm module skeleton compiles** — `src/api/mod.rs`, `src/api/handlers.rs`,
  `src/api/sse.rs`, `src/api/types.rs` already scaffolded; ensure they compile on stubs
  before adding logic.

---

### Implementation Tasks (Sub-Epic A)

- [ ] **`src/api/types.rs`** — client-facing DTOs: `TrainSummary`, `DepartureBoardEntry`,
  `LiveUpdateEvent`. Each is a flattened, serialisable view of the internal types —
  no `TrainId` enum, no `Stamped<T>` wrapper, just plain JSON-friendly fields.

- [ ] **`GET /stations/{crs}/departures`** — Tier A: query registry for all trains
  departing from the given CRS within a configurable window; return as
  `Vec<DepartureBoardEntry>` sorted by scheduled departure. No live GBR call.
  HTML path returns a maud `departure-board` fragment for htmx swap.

- [ ] **`GET /trains/{rid}`** — Tier B: return the current `TrainSummary` from the
  registry. If the RID is unknown, trigger a coalesced GBR fetch and wait. Return 404
  only if GBR also returns not found.

- [ ] **`GET /trains/{rid}/live` (SSE)** — Tier C: subscribe to `state_change_tx` events
  for this RID and stream them as `LiveUpdateEvent` SSE. Each event body is a maud HTML
  fragment for htmx OOB swap. Heartbeat every 15s. Close stream on `Terminal` state.

- [ ] **CORS + tracing middleware** — `CorsLayer`, `TraceLayer` (method, path, status, latency).

- [ ] **Wire axum into `main.rs`** — pass `Arc<TrainRegistry>`, coalescer, and
  `state_change_tx` receiver handle as `axum::Extension` state; spawn server task
  alongside poll manager and ingestion pipeline.

- [ ] **API integration test** — spin up full axum server in `#[tokio::test]`, POST a
  mock Darwin message to force a registry update, then `GET /trains/{rid}` and assert
  the JSON response reflects it.

---

## Sub-Epic B — Rust/maud/htmx Frontend

### Key considerations:

1. **Uber-style progressive disclosure — same UX, different stack**
   - **Search** — type origin/destination, departure board renders instantly (Tier A, server-rendered maud fragment, htmx swap).
   - **Detail** — click a train, maud renders the detail page; htmx SSE connection opens automatically via `hx-ext="sse"` and swaps in live updates.
   - **Checkout** — "Book" button triggers a single Tier C call; htmx posts and swaps the confirmation fragment in-place.

2. **maud for all HTML**
   All HTML is generated by `maud` macros in `src/frontend/`. No template files, no
   runtime parsing — templates are checked at compile time. Each page and fragment is
   a Rust function returning `maud::Markup`. Layout chrome lives in a shared
   `src/frontend/layout.rs` base template.

3. **htmx for interactivity**
   - `hx-get` + `hx-target` for departure board search (replaces the results div).
   - `hx-ext="sse"` + `sse-connect` on the detail page to subscribe to the live endpoint.
   - `hx-swap="outerHTML"` OOB swaps for the delay badge and position indicator.
   - No JavaScript written by hand except a single `<script>` for the SVG route diagram
     position interpolation (≤50 lines).

4. **Dark-first design — same palette**
   Near-black background (`#1a1a1a`), white primary text, rail-green accent (`#00c853`)
   for on-time state. Amber and red for delay states. Platform number gets large
   typographic treatment. Single `static/style.css` file served by axum `ServeDir`.

5. **Single binary deploy**
   `static/` (CSS + htmx CDN-cached script) is embedded at compile time via
   `include_str!` or `rust-embed`. No `ServeDir` dependency on the filesystem at
   runtime — the binary is fully self-contained.

6. **Stale-data / offline state**
   When the SSE connection drops, htmx fires `htmx:sseError`. A small inline handler
   swaps in a "Live updates paused — last updated HH:MM:SS" banner. Dismissed
   automatically when the connection restores.

---

### Before Starting: Pre-work Subtasks (Sub-Epic B)

- [ ] **Agree the API contract with Sub-Epic A first** — do not start frontend work until
  `src/api/types.rs` is finalised. The DTO shapes drive what maud templates render.

- [ ] **Sketch the three page states** — ASCII-art or commented maud stubs for Search,
  Detail, and Checkout layouts. Agree the progressive-disclosure flow before writing
  any template logic.

- [ ] **Add maud + rust-embed to Cargo.toml** — `maud` (axum feature), `rust-embed`
  (for embedding `static/`). Confirm they compile alongside existing axum version.

- [ ] **Decide routing split** — API routes under `/api/v1/`, page routes at `/` and
  `/trains/:rid`. Both served from the same axum `Router`. Page routes return full
  `maud::Markup` documents; API routes return JSON or HTML fragments depending on
  `Accept` header.

---

### Implementation Tasks (Sub-Epic B)

- [ ] **`src/frontend/` module** — `mod.rs`, `layout.rs` (base chrome: nav, stale banner
  slot), `search.rs` (search page + departure board fragment), `detail.rs` (detail page
  + live update fragments), `components.rs` (delay badge, platform chip, route diagram
  shell).

- [ ] **`/` — Search page** — maud full-page render; origin CRS `<input>` with
  `hx-get="/api/v1/stations/{crs}/departures"` and `hx-target="#results"`; static
  timetable renders immediately, live status overlaid as it loads.

- [ ] **`/trains/:rid` — Detail page** — maud full-page render; train summary card
  (origin → destination, scheduled time, predicted delay, platform);
  `hx-ext="sse" sse-connect="/api/v1/trains/{rid}/live"` on the live section;
  SVG route diagram with station list and current position marker.

- [ ] **Live route diagram** — SVG rendered by maud with station nodes at fixed positions;
  current position marker updated by htmx OOB swap on each SSE event.
  Small inline `<script>` interpolates marker position smoothly between swaps (≤50 lines).

- [ ] **Delay badge component** — maud function `fn delay_badge(minutes: Option<i32>, cancelled: bool) -> Markup`; green / amber / red driven by value. Used in both search results and detail page.

- [ ] **Stale-data banner** — maud fragment; hidden by default; revealed by inline htmx
  SSE error handler; shows last-updated timestamp; auto-dismissed on reconnect.

- [ ] **`static/style.css`** — single hand-written stylesheet; dark palette; no preprocessor.
  Embedded at compile time via `rust-embed` so the binary has no filesystem dependency.

- [ ] **Route wiring in `src/api/mod.rs`** — add page routes alongside API routes in the
  same axum `Router`; serve embedded static assets via a `rust-embed` handler.

- [ ] **End-to-end test** — `#[tokio::test]` spins up the full server, GETs `/`, asserts
  the HTML response contains expected landmarks (search input, results target div).
