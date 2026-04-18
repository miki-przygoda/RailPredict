# Production Hardening — Safe to Deploy

_Items that will cause silent failures, security holes, or broken connections the moment
the app is exposed publicly. None of these require architectural changes — they are
targeted fixes to existing modules._

_Prereqs: None. This epic is self-contained and can run in parallel with others._

---

## Items from Improvements.md

### 1.1 — STOMP TLS (production-blocking)
`LiveStompClient::subscribe` in `src/ingestion/stomp_client.rs` calls `TcpStream::connect`.
The real Darwin Push Port broker requires TLS (port 61614 direct TLS or 61613 + STARTTLS).
Plain TCP will be silently refused.

**Fix:**
- Add `tokio-rustls` to `Cargo.toml`.
- Wrap `TcpStream` in a `TlsConnector` before the STOMP CONNECT frame is sent.
- Add `DARWIN_TLS=true` env var (default: on) with an escape hatch for local mock brokers.
- Read the var in `StompConfig`; gate the TLS wrapping behind it.
- Update `StompConfig::from_env` and `.env.example`.

---

### 1.2 — STOMP auto-reconnect (production-blocking)
`IngestionPipeline::run` in `src/ingestion/mod.rs` exits as soon as the STOMP stream
closes. Darwin disconnects clients roughly every 30 minutes. After that, the pipeline
stops permanently and the in-memory registry freezes.

**Fix:**
- Wrap the `pipeline.run()` call in `main.rs` in a retry loop with exponential backoff
  (start: 2 s, cap: 120 s).
- The `SequenceGuard` already handles duplicate/replayed messages on reconnect — no
  changes needed there.
- Log each reconnect attempt with tracing at WARN level including the attempt count and
  next retry delay.

---

### 1.4 — CORS is permanently permissive
`CorsLayer::permissive()` in `src/api/mod.rs` allows any origin, any method, any header.

**Fix:**
- Add `CORS_ALLOWED_ORIGINS` env var (comma-separated list of origins).
- In `router()`, use `CorsLayer::new().allow_origin(...)` with the parsed origins.
- Default to `permissive()` only when `CORS_ALLOWED_ORIGINS` is unset AND `LOG_LEVEL=debug`
  (i.e. dev mode). In all other cases, an unset var should produce an error on startup.
- Update `config.rs`, `Config::from_env`, and `.env.example`.

---

### 1.5 — No HTTP API rate limiting
The public HTTP endpoints have no throttling. A single client can exhaust the server by
hammering `/stations/{crs}/departures` (500 sequential read-lock acquisitions per call).

**Fix:**
- Add `tower_governor` to `Cargo.toml`.
- Apply `GovernorLayer` as a middleware in `router()` in `src/api/mod.rs`.
- Limit: 60 requests/s per IP (configurable via `HTTP_RATE_LIMIT_PER_SEC` env var,
  default 60). Add the var to `config.rs` and `.env.example`.
- The `/metrics` and `/health` routes should be excluded from rate limiting.

---

### 3.1 — Validate CRS code format in all handlers
`/stations/{crs}/departures` accepts any string and uppercases it. A 500-char CRS
would pass through to the DB query.

**Fix:**
- Add a `validate_crs(crs: &str) -> Result<(), ApiError>` helper (or inline check) in
  `src/api/handlers.rs`.
- Exactly 3 ASCII letters (A–Z), case-insensitive input accepted then uppercased.
- Return `ApiError::bad_request("CRS must be exactly 3 letters")` on failure.
- Apply to every handler that accepts a CRS path parameter.

---

### 3.2 — Health endpoint should probe DB connectivity
`GET /health` always returns 200. An operator restarting a crashed DB would see a
"healthy" service silently serving stale data.

**Fix:**
- In the health handler in `src/api/handlers.rs`, run `sqlx::query("SELECT 1").execute(&state.db)`
  with a 1-second timeout (use `tokio::time::timeout`).
- Return 200 + `{ "status": "ok" }` on success.
- Return 503 + `{ "status": "degraded", "detail": "db unreachable" }` on failure.
- `state.db: Db` is already wired into `AppState` (completed as Improvements 2.3 prereq).

---

### 3.3 — Document secrets rotation procedure
`.env.example` lists `GBR_API_KEY`, `DARWIN_PASSWORD`, `DB_PASSWORD` but there is no
documented rotation procedure.

**Fix:**
- Create `SECURITY.md` at repo root.
- Document: which secrets exist, how to rotate each (re-deploy with updated env, no
  live-reload needed for stateless tokens), recommended rotation cadence.
- Add a brief note in README pointing to `SECURITY.md`.

---

## Priority Order

1. 1.2 (auto-reconnect) — smallest change, highest safety impact
2. 1.1 (STOMP TLS) — required for any real Darwin connection
3. 3.1 (CRS validation) — touches every public handler, trivial fix
4. 3.2 (health DB probe) — closes a silent failure mode
5. 1.4 (CORS tightening) — security hygiene
6. 1.5 (HTTP rate limiting) — protects the server under load
7. 3.3 (security docs) — ops documentation

---

## Files Expected to Change

- `RailPredict/Cargo.toml` (tokio-rustls, tower_governor)
- `RailPredict/src/ingestion/stomp_client.rs` (TLS wrapping)
- `RailPredict/src/ingestion/mod.rs` (reconnect loop — or handled in main.rs)
- `RailPredict/src/main.rs` (reconnect retry loop, rate limit config)
- `RailPredict/src/api/mod.rs` (CORS tightening, rate limit middleware)
- `RailPredict/src/api/handlers.rs` (CRS validation, health DB probe)
- `RailPredict/src/config.rs` (new env vars)
- `.env.example` (document new vars)
- `SECURITY.md` (new file)

---

## Implementation Status

### 1.2 — STOMP auto-reconnect ✓ COMPLETE

**Changed files:**
- `RailPredict/src/ingestion/mod.rs` — `IngestionPipeline::run` now returns `anyhow::Result<()>`; added `PipelineContext` struct (holds `Arc<Filter>`, registry, broadcast tx, prediction engine) and `from_context` constructor to support rebuild after reconnect.
- `RailPredict/src/main.rs` — Retry loop with exponential backoff (2s → 120s cap), `CancellationToken` select on both the run future and the sleep. Logs `attempt` + `retry_secs` at WARN on each reconnect.

**Deviation:** The reconnect loop is in `main.rs` as specified. `IngestionPipeline` was refactored to hold a `PipelineContext` (`Arc`-backed shared state) so the shared registry and filter sequence state survive across reconnects; only the STOMP client is replaced.

---

### 1.1 — STOMP TLS ✓ COMPLETE

**Changed files:**
- `RailPredict/Cargo.toml` — Added `tokio-rustls = "0.26"`, `rustls = "0.23"`, `rustls-native-certs = "0.8"`.
- `RailPredict/src/ingestion/stomp_client.rs` — `StompConfig.tls: bool` field (read from `DARWIN_TLS`, default `true`); `LiveStompClient::subscribe` wraps `TcpStream` in `TlsConnector` when `tls=true` using system CA certs from `rustls-native-certs`; `read_frame` made generic over `AsyncRead` to work with both plain and TLS streams.
- `.env.example` — `DARWIN_TLS=true` documented.

---

### 3.1 — CRS validation ✓ COMPLETE

**Changed files:**
- `RailPredict/src/api/handlers.rs` — `fn validate_crs(crs: &str) -> Result<(), ApiError>` added; applied at the top of `departures_handler`. Only `departures_handler` takes a CRS path parameter in the JSON API; SSE and train handlers use RID not CRS.

---

### 3.2 — Health endpoint DB probe ✓ COMPLETE

**Changed files:**
- `RailPredict/src/api/handlers.rs` — `health_handler` now takes `State(state): State<AppState>`, runs `sqlx::query("SELECT 1")` with 1s timeout, returns 200 on success or 503 with `{"status":"degraded","detail":"db unreachable"}` on failure.
- `RailPredict/src/api/types.rs` — `HealthResponse` gains `detail: Option<&'static str>` (skipped in JSON when `None`).

---

### 1.4 — CORS tightening ✓ COMPLETE

**Changed files:**
- `RailPredict/src/config.rs` — `cors_allowed_origins: Option<Vec<String>>` read from `CORS_ALLOWED_ORIGINS` (comma-separated); required in production (non-debug `LOG_LEVEL`), permissive when `None` and `LOG_LEVEL=debug`.
- `RailPredict/src/api/mod.rs` — `AppState` gains `cors_allowed_origins` + `http_rate_limit_per_sec`; `router()` builds `CorsLayer` from the allow-list or uses `permissive()` in dev mode.
- `RailPredict/src/main.rs` — Both `AppState` construction sites updated.
- `.env.example` — `CORS_ALLOWED_ORIGINS` documented.

---

### 1.5 — HTTP rate limiting ✓ COMPLETE

**Changed files:**
- `RailPredict/Cargo.toml` — Added `tower_governor = { version = "0.4", features = ["axum"] }`.
- `RailPredict/src/config.rs` — `http_rate_limit_per_sec: u64` read from `HTTP_RATE_LIMIT_PER_SEC` (default 60; `0` disables).
- `RailPredict/src/api/mod.rs` — `/health` and `/metrics` moved to a separate `infra_router` that does NOT get the `GovernorLayer`; public API routes get `GovernorLayer` when `rate_limit_per_sec > 0`.
- `.env.example` — `HTTP_RATE_LIMIT_PER_SEC=60` documented.

---

### 3.3 — SECURITY.md ✓ COMPLETE

**Changed files:**
- `SECURITY.md` (new) — Documents GBR_API_KEY, DARWIN_PASSWORD, DB_PASSWORD: rotation steps, verification after rotation, and recommended cadence. Also covers CORS, rate limiting, TLS, and `.env` gitignore note.
- `.env.example` — Brief references to secrets and new security vars.
