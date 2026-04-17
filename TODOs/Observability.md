# Observability — Mission Control for the Intelligent Buffer

_The system is running in Docker with structured JSON logs, but it is "blind" to its own_
_efficiency. This epic adds the metrics layer that proves the three-tier architecture_
_is actually working: that Tier A is absorbing the load, Tier B is reducing live calls,_
_and Tier C is only triggered at the right moment._

---

## Why Observability Matters Here Specifically

RailPredict's core claim is that it reduces GBR API calls by serving most requests from
local data. Without metrics, there is no way to verify this claim. The three key ratios
to track are:

- **Cache hit ratio**: What fraction of departure board requests were served from Tier A
  (DB) or Tier B (prediction) vs required a Tier C (live GBR) call?
- **Prediction accuracy**: How far off is `predicted_delay_mins` from `reported_delay_mins`
  on trains where both are known? Is the Tier B engine actually useful?
- **Darwin health**: How many messages per second is the firehose delivering, and what
  fraction are being dropped by the filter vs accepted?

Without these numbers, you're operating on faith.

---

## Phase 1 — Prometheus Metrics Endpoint

### Dependencies to add
```toml
metrics = "0.23"
metrics-exporter-prometheus = "0.15"
```
(`axum-prometheus` wraps this for axum; `metrics` provides the macros.)

### Setup in `main.rs`
```rust
use metrics_exporter_prometheus::PrometheusBuilder;

let recorder = PrometheusBuilder::new().install_recorder()?;
// Add GET /metrics route that renders the current scrape output.
```

Add `GET /metrics` to the router in `src/api/mod.rs`. This endpoint should NOT be
behind CORS — it is for internal scraping only. Consider gating it behind an
`METRICS_ENABLED` env var (default: off in production, on in dev) or restricting to
localhost-only via a separate listener.

---

## Phase 2 — Core Metrics to Instrument

### Darwin ingestion pipeline (`src/ingestion/mod.rs`)

```rust
// In process_frame:
metrics::counter!("darwin_messages_received_total").increment(1);
metrics::counter!("darwin_messages_dropped_total",
    "reason" => "taxonomy").increment(1);         // filter::should_parse = false
metrics::counter!("darwin_messages_dropped_total",
    "reason" => "stale").increment(1);             // filter::should_apply = false
metrics::counter!("darwin_messages_applied_total").increment(1);
```

Derive `darwin_message_rate_per_sec` in Grafana from
`rate(darwin_messages_received_total[1m])`.

### GBR API client (`src/networking/gbr_client.rs`)

```rust
// Wrap every LiveGbrClient::get_train_status call:
let start = std::time::Instant::now();
let result = /* http call */;
let latency_ms = start.elapsed().as_millis() as f64;
metrics::histogram!("gbr_api_latency_ms",
    "endpoint" => "train_status",
    "status"   => if result.is_ok() { "ok" } else { "error" }
).record(latency_ms);
```

### Circuit breaker (`src/networking/circuit_breaker.rs`)

```rust
// On state transition:
metrics::gauge!("circuit_breaker_state",
    "state" => format!("{:?}", new_state)).set(1.0);
// Track how many requests were blocked by an Open breaker:
metrics::counter!("circuit_breaker_blocked_total").increment(1);
```

### Cache / tier routing

In the HTTP handlers, once the tier-routing logic exists (Improvements.md item 2.3):
```rust
metrics::counter!("request_served_tier_total",
    "tier" => "A").increment(1);   // served from DB timetable
metrics::counter!("request_served_tier_total",
    "tier" => "B").increment(1);   // served from prediction
metrics::counter!("request_served_tier_total",
    "tier" => "C").increment(1);   // required live GBR call
```
The ratio `tier_A / (tier_A + tier_B + tier_C)` is the **cache hit ratio** — the single
most important metric for validating the architecture.

### Prediction engine accuracy (`src/prediction/engine.rs`)

After `predict_and_update`, if both `predicted_delay_mins` and `reported_delay_mins` are
known for the same train, record the error:
```rust
if let (Some(predicted), Some(reported)) =
    (status.predicted_delay_mins.value, status.reported_delay_mins.value) {
    let error_mins = (predicted - reported).abs() as f64;
    metrics::histogram!("prediction_error_mins").record(error_mins);
}
```

### Registry size

In the 60s eviction task in `main.rs`:
```rust
metrics::gauge!("registry_train_count").set(registry.len() as f64);
```

### DB flush

In `db::history::flush_history`:
```rust
let start = std::time::Instant::now();
// ... flush ...
metrics::histogram!("db_flush_duration_ms")
    .record(start.elapsed().as_millis() as f64);
metrics::counter!("db_flush_rows_inserted_total").increment(inserted_total as u64);
```

---

## Phase 3 — Grafana Dashboard

Once Prometheus is scraping `/metrics`, set up a Grafana dashboard with:

| Panel | Query | Purpose |
|-------|-------|---------|
| Darwin msg/s | `rate(darwin_messages_received_total[1m])` | Firehose health |
| Drop rate % | `rate(dropped) / rate(received)` | Filter effectiveness |
| GBR p99 latency | `histogram_quantile(0.99, gbr_api_latency_ms)` | API health |
| Circuit breaker state | `circuit_breaker_state` | Tier C gating |
| Cache hit ratio | `tier_A / (tier_A + tier_B + tier_C)` | Core architecture KPI |
| Prediction MAE | `histogram_quantile(0.5, prediction_error_mins)` | Tier B quality |
| Registry size | `registry_train_count` | Memory pressure indicator |
| DB flush latency | `histogram_quantile(0.99, db_flush_duration_ms)` | DB write health |

Add the Grafana service to `docker-compose.yml` under a `--profile monitoring` flag,
similar to how pgadmin is gated behind `--profile dev`.

---

## Phase 4 — Structured Log Correlation

`tracing` is already set up with JSON output in production (`LOG_FORMAT=json`). To make
logs and metrics correlate:

- Add a `trace_id` field to every log span that covers a Darwin message or a GBR call.
  Use `tracing::Span::current().id()` as a stable identifier within a request boundary.
- In the GBR client, log `rid`, `latency_ms`, and `status_code` as span fields so each
  slow call is findable by RID in the log stream.

This allows you to go from a Grafana spike in `gbr_api_latency_ms` → find the specific
RID that caused it in the logs → correlate with the Darwin message that triggered the
poll.

---

## Priority Order

1. Phase 1 (endpoint setup) — prerequisite for everything else
2. Phase 2 (instrument darwin + circuit breaker + registry size) — lowest effort, highest signal
3. Phase 2 (cache hit ratio) — requires Improvements.md 2.3 to be done first
4. Phase 3 (Grafana dashboard) — wiring Prometheus + Grafana in Compose
5. Phase 2 (prediction accuracy) — requires enough history to be meaningful
6. Phase 4 (log correlation) — polish, do last
