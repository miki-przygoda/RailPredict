# TODOs/UI.md – HTTP API Layer & Uber-Style Frontend

Two sub-epics that must be built in sequence: the axum API server first (it defines
the contract), then the SvelteKit frontend that consumes it.

---

## Sub-Epic A — axum HTTP API

### Key considerations:

1. **SSE over WebSocket**
   The data flow is server-push only — the client never sends runtime updates back.
   Server-Sent Events are simpler (plain HTTP, no upgrade handshake, automatic
   reconnect built into the browser `EventSource` API). Reserve WebSocket for a
   future bidirectional feature (e.g. seat reservation).

2. **API contract before implementation**
   Define request/response types as Rust structs with `serde::Serialize` before writing
   any handler. These structs are the contract the frontend will code against. Changing
   them later is a two-repo change.

3. **The three-tier rule at the API boundary**
   - `GET /trains/{rid}` must serve from the registry (Tier B/C) without hitting GBR
     unless the registry has no entry. The handler must never call GBR directly — it
     goes through the coalescer.
   - `GET /stations/{crs}/departures` is Tier A: served from the static schedule, zero
     live calls on the search path.
   - `GET /trains/{rid}/live` (SSE) subscribes the caller to state-change events for
     one RID. It reads from the `state_change_tx` broadcast — it does not poll.

4. **CORS**
   The SvelteKit dev server runs on a different port. Configure `tower-http`'s `CorsLayer`
   from the start — retrofitting CORS after the frontend is built is always painful.

---

### Before Starting: Pre-work Subtasks (Sub-Epic A)

- [ ] **Cargo.toml: add API dependencies** — `axum` (with `macros` feature), `tower`,
  `tower-http` (with `cors` and `trace` features), `tokio-stream` (for SSE body
  streaming). Check version compatibility with the existing `tower` pulled in transitively.

- [ ] **Write the API contract as a markdown table before coding** — for every endpoint,
  document: HTTP method, path, path/query params, success response shape (field names
  and types), error response shape, which data tier it serves from, and whether it is
  synchronous or streaming. This table becomes the reference the frontend team works
  from. Store it as a comment block at the top of `src/api/mod.rs`.

- [ ] **Decide the SSE event format** — the browser `EventSource` API receives
  `data: ...\n\n` lines. Decide whether the event body is a JSON-serialised
  `StateChangeEvent` or a simplified client-facing DTO. The latter is better: the
  internal `StateChangeEvent` carries internal state (e.g. `TrainState` enum variants)
  that should not leak to the frontend as a stable contract.

- [ ] **Design error response shape** — a consistent `{ "error": "...", "code": "..." }`
  JSON envelope for all 4xx/5xx responses. Define this as a type that implements
  `axum::response::IntoResponse` before writing any handler — otherwise each handler
  invents its own error format.

- [ ] **Create the module skeleton** — `src/api/mod.rs`, `src/api/handlers.rs`,
  `src/api/sse.rs`, `src/api/types.rs`. Compile on stubs before adding logic.

---

### Implementation Tasks (Sub-Epic A)

- [ ] **`src/api/types.rs`** — client-facing DTOs: `TrainSummary`, `DepartureBoardEntry`,
  `LiveUpdateEvent`. Each is a flattened, serialisable view of the internal types —
  no `TrainId` enum, no `Stamped<T>` wrapper, just plain JSON-friendly fields.

- [ ] **`GET /stations/{crs}/departures`** — Tier A: query registry for all trains
  departing from the given CRS within a configurable window; return as
  `Vec<DepartureBoardEntry>` sorted by scheduled departure. No live GBR call.

- [ ] **`GET /trains/{rid}`** — Tier B: return the current `TrainSummary` from the
  registry. If the RID is unknown, trigger a coalesced GBR fetch and wait. Return 404
  only if GBR also returns not found.

- [ ] **`GET /trains/{rid}/live` (SSE)** — Tier C: subscribe to `state_change_tx` events
  for this RID and stream them as `LiveUpdateEvent` JSON. Heartbeat every 15s to keep
  the connection alive through proxies. Close the stream when the train reaches
  `Terminal` state.

- [ ] **CORS + tracing middleware** — `CorsLayer` (allow frontend origin), `TraceLayer`
  (log method, path, status, latency for every request).

- [ ] **Wire axum into `main.rs`** — pass `Arc<TrainRegistry>`, coalescer, and
  `state_change_tx` receiver handle to the router as `axum::Extension` state; spawn
  the server as a task alongside the poll manager and ingestion pipeline.

- [ ] **API integration test** — spin up the full axum server in a `#[tokio::test]`,
  POST a mock Darwin message to force a registry update, then `GET /trains/{rid}` and
  assert the response reflects it.

---

## Sub-Epic B — SvelteKit Frontend

### Key considerations:

1. **Uber-style progressive disclosure**
   The UI has three phases, each triggering at most one tier of data:
   - **Search** — type origin/destination, see a departure board instantly (Tier A cached).
   - **Detail** — tap a train, see predicted delay and platform (Tier B local compute),
     then SSE connection opens and live updates stream in.
   - **Checkout** — initiate booking; single Tier C call to lock the ticket.

2. **Map vs. list**
   Uber's signature is the map. For rail, the equivalent is a **live route diagram**:
   a stylised line showing the train's current position between stations, updating as
   SSE events arrive. This is the single highest-impact visual element. Use a `canvas`
   or SVG — do not embed a full tile map (unnecessary weight for a fixed rail route).

3. **Dark-first design**
   Uber's palette: near-black background (`#1a1a1a`), white primary text, electric
   accent (propose `#00c853` — a rail-green that reads as "on time"). Delay states use
   amber and red. Platform number gets large typographic treatment (Uber-style "your
   car is X" moment).

4. **Offline / stale state**
   When the SSE connection drops, the UI must not silently show stale data. Display a
   "Live updates paused" banner and a timestamp of the last known update. The browser
   `EventSource` reconnects automatically; dismiss the banner when it does.

---

### Before Starting: Pre-work Subtasks (Sub-Epic B)

- [ ] **Agree the API contract with Sub-Epic A first** — do not start frontend work
  until `src/api/types.rs` is finalised. The DTO shapes are the shared contract.

- [ ] **Design mockups for the three phases before coding** — sketch the Search page,
  Detail page, and Checkout trigger in a tool like Figma or even ASCII art. The
  progressive-disclosure flow must be agreed before component structure is chosen.
  One wrong assumption here refactors three components.

- [ ] **Decide state management approach** — SvelteKit's built-in stores are sufficient
  for this app. Do not introduce a Redux-style library. The SSE stream maps cleanly to
  a Svelte `writable` store: the SSE `onmessage` handler calls `store.set(event)` and
  every component that cares subscribes reactively.

- [ ] **Decide the build integration** — SvelteKit can be served as a static export
  (`adapter-static`) or as a Node SSR server (`adapter-node`). For this project,
  `adapter-static` output served by the axum binary itself (via `tower_http::ServeDir`)
  is the cleanest single-binary deployment. Decide before scaffolding.

---

### Implementation Tasks (Sub-Epic B)

- [ ] **Scaffold SvelteKit project** in `frontend/` at repo root (separate from the Rust
  crate). TypeScript, `adapter-static`, ESLint + Prettier.

- [ ] **`/` — Search page** — origin/destination autocomplete from a static CRS list;
  submit triggers `GET /stations/{crs}/departures`; renders departure board cards.
  Instant feel: show the static timetable immediately, overlay live status as it loads.

- [ ] **`/trains/[rid]` — Detail page** — train summary card (origin → destination,
  scheduled time, predicted delay, platform); opens SSE connection on mount, closes on
  unmount; live route diagram (SVG) showing position between stops; "Book" CTA.

- [ ] **Live route diagram component** — SVG component that accepts a list of stations
  and a current position indicator; updates position smoothly as SSE events arrive.
  Stateless and reusable — takes props, emits nothing.

- [ ] **Delay badge component** — reusable chip: green "On time", amber "N min delay",
  red "Cancelled". Colour and label driven purely by props. Used on both the departure
  board and the detail page.

- [ ] **SSE store** — a Svelte store factory `createLiveTrainStore(rid)` that opens an
  `EventSource`, populates the store on each event, and cleans up on destroy. Handles
  reconnection banner state.

- [ ] **Stale-data banner** — shown when SSE `onerror` fires; dismissed on reconnect.
  Displays "Last updated HH:MM:SS" timestamp.

- [ ] **`adapter-static` build + axum `ServeDir`** — `cargo build` produces a single
  binary that serves both the API and the compiled frontend from `frontend/build/`.
  Document the build order in `README.md`.
