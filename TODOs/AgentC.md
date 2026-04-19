# Agent C — Weather Volatility + Push Notifications

_Items: 5.4, 5.5_

**File ownership — Agent C only:**
- `RailPredict/src/weather/mod.rs` (new file)
- `RailPredict/src/lib.rs`
- `RailPredict/src/main.rs`
- `RailPredict/src/config.rs`

**No overlap with Agent A or Agent B.** Do not touch any other files.

---

## 5.4 — Weather-driven volatility promotions

`VolatilityContext` in `src/types/volatility.rs` has a `wind_speed_mph: Option<f32>` field
and `incident_flagged: bool`. Nothing sets these from live data. This item wires in
Open-Meteo (free, no API key) to fetch wind speed every 10 minutes and trigger
`emergency_promote` on affected trains.

### Step 1 — New module `src/weather/mod.rs`

```rust
//! Weather integration: fetches wind speed from Open-Meteo every 10 minutes
//! and writes results into a shared VolatilityStore.

use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use serde::Deserialize;

/// Maps route_id (a CRS code representing a route anchor point) to
/// current wind speed in mph. Written by the weather task; read by the
/// poll consumer when evaluating state promotions.
pub type VolatilityStore = Arc<DashMap<String, f32>>;

/// A (lat, lon, route_id) tuple defining a bounding-box centre to poll.
#[derive(Debug, Clone)]
pub struct WeatherAnchor {
    pub route_id: String,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Deserialize)]
struct OpenMeteoResponse {
    hourly: OpenMeteoHourly,
}

#[derive(Debug, Deserialize)]
struct OpenMeteoHourly {
    windspeed_10m: Vec<f32>,
}

/// Fetch current wind speed (mph) for a given lat/lon from Open-Meteo.
///
/// Open-Meteo returns values in km/h by default; pass `wind_speed_unit=mph` to get mph.
/// The hourly array covers the next 168 hours — index 0 is the current hour.
pub async fn fetch_wind_mph(
    client: &reqwest::Client,
    lat: f64,
    lon: f64,
) -> anyhow::Result<f32> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast\
         ?latitude={lat}&longitude={lon}\
         &hourly=windspeed_10m&wind_speed_unit=mph&forecast_days=1"
    );
    let resp: OpenMeteoResponse = client.get(&url).send().await?.json().await?;
    resp.hourly.windspeed_10m.first().copied()
        .ok_or_else(|| anyhow::anyhow!("Open-Meteo returned empty windspeed array"))
}

/// Run the weather polling loop: refresh every 10 minutes, write into the store.
///
/// Spawned as a background task. If a fetch fails, log and continue — stale
/// data is better than a panic. Wind speed stays at the last known value.
pub async fn run_weather_task(
    store: VolatilityStore,
    anchors: Vec<WeatherAnchor>,
    client: reqwest::Client,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(600));
    loop {
        interval.tick().await;
        for anchor in &anchors {
            match fetch_wind_mph(&client, anchor.lat, anchor.lon).await {
                Ok(mph) => {
                    tracing::debug!(route = %anchor.route_id, wind_mph = mph, "weather update");
                    store.insert(anchor.route_id.clone(), mph);
                }
                Err(e) => {
                    tracing::warn!(route = %anchor.route_id, error = %e, "weather fetch failed");
                }
            }
        }
    }
}
```

### Step 2 — `src/lib.rs`

Add `pub mod weather;` alongside the other module declarations.

### Step 3 — `src/config.rs`

Add two new optional fields to `Config` (no new required vars — these are opt-in):

```rust
// Push notifications
/// ntfy.sh topic URL (e.g. https://ntfy.sh/my-topic). None = disabled.
pub ntfy_url: Option<String>,
/// Gate for push notifications. Must be true AND ntfy_url set for notifications to fire.
pub notifications_enabled: bool,

// Weather
/// Comma-separated "route_id:lat:lon" tuples for weather polling.
/// Example: "LDS:53.796:-1.548,MAN:53.488:-2.242"
/// When empty, weather polling is disabled (no task spawned).
pub weather_anchors: Vec<(String, f64, f64)>,
```

Parse them in `Config::from_env`:
```rust
let ntfy_url = std::env::var("NTFY_URL").ok().filter(|s| !s.is_empty());
let notifications_enabled = std::env::var("NOTIFICATIONS_ENABLED")
    .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
    .unwrap_or(false);
let weather_anchors = parse_weather_anchors(
    &std::env::var("WEATHER_ANCHORS").unwrap_or_default()
);
```

Add the parser as a private fn:
```rust
fn parse_weather_anchors(raw: &str) -> Vec<(String, f64, f64)> {
    raw.split(',')
        .filter_map(|entry| {
            let parts: Vec<&str> = entry.trim().splitn(3, ':').collect();
            if parts.len() == 3 {
                let lat = parts[1].parse::<f64>().ok()?;
                let lon = parts[2].parse::<f64>().ok()?;
                Some((parts[0].to_string(), lat, lon))
            } else {
                None
            }
        })
        .collect()
}
```

Update the env var table comment at the top of `config.rs` with the new variables:
```
| NTFY_URL               | no  | —      | ntfy.sh topic URL for push notifications            |
| NOTIFICATIONS_ENABLED  | no  | false  | Set to true to enable push notifications            |
| WEATHER_ANCHORS        | no  | ""     | Comma-separated "route_id:lat:lon" route anchors    |
```

Update `Config::for_testing()` to set:
```rust
ntfy_url: None,
notifications_enabled: false,
weather_anchors: Vec::new(),
```

### Step 4 — `src/main.rs`

**Add VolatilityStore to AppState** — the weather data needs to be reachable by the poll
consumer. `AppState` is defined in `src/api/mod.rs` (Agent B's file) — BUT Agent C must NOT
edit `api/mod.rs`. Instead, pass the store into the poll consumer task directly as a captured
variable (it's already an `Arc<DashMap>` so it's cheap to clone into the task).

**Wire the weather task:**
```rust
if !config.weather_anchors.is_empty() {
    let store = Arc::clone(&volatility_store);
    let anchors: Vec<WeatherAnchor> = config.weather_anchors.iter()
        .map(|(id, lat, lon)| WeatherAnchor { route_id: id.clone(), lat: *lat, lon: *lon })
        .collect();
    let http_client = reqwest::Client::new();
    tokio::spawn(railpredict::weather::run_weather_task(store, anchors, http_client));
}
```

Declare `let volatility_store: railpredict::weather::VolatilityStore = Arc::new(DashMap::new());`
near the top of `main()` alongside the other state instantiation.

**Wire promotion trigger in the poll consumer task** (already exists in main.rs from v1.6.0 —
add a check after each GBR write):
```rust
// After applying GBR update to registry...
if let Some(wind_mph) = volatility_store.get(route_id) {
    if *wind_mph > 50.0 {
        registry.update(&train_id, |status| {
            status.volatility.wind_speed_mph = Some(*wind_mph);
            status.volatility.incident_flagged = true;
        });
        // emergency_promote is on TrainState — call it to force Critical
        // state via the existing state_change_tx broadcast.
    }
}
```

Note: `route_id` for a train is its `origin_crs` (or a route corridor CRS code). For the
initial implementation, match `wind_mph > 50.0` for any anchor whose `route_id` appears in
the train's `origin_crs` field. Keep it simple.

---

## 5.5 — Push notifications on Critical promotions

When a train transitions to `TrainState::Critical`, send a POST to the ntfy topic.

### In `src/main.rs`

Add a notification task (separate `tokio::spawn` from the poll consumer — cleaner separation):

```rust
if config.notifications_enabled {
    if let Some(ntfy_url) = config.ntfy_url.clone() {
        let mut notif_rx = state_change_tx.subscribe();
        let http_client = reqwest::Client::new();
        tokio::spawn(async move {
            loop {
                match notif_rx.recv().await {
                    Ok(event) if event.new_state == railpredict::state_machine::TrainState::Critical
                              && event.old_state != railpredict::state_machine::TrainState::Critical => {
                        let title = format!("Train {} now Critical", event.train_id);
                        let body  = format!("State promotion detected — check live updates");
                        let result = http_client
                            .post(&ntfy_url)
                            .header("Title", &title)
                            .body(body)
                            .send()
                            .await;
                        if let Err(e) = result {
                            tracing::warn!(error = %e, "ntfy push notification failed");
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "notification receiver lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}
```

The condition `new_state == Critical && old_state != Critical` prevents repeat notifications
for trains that are already Critical and get re-queued.

---

## Validation

After implementing:
1. `cargo build` passes with zero warnings.
2. `cargo clippy -- -D warnings` passes.
3. `cargo test --lib` passes.
4. `Config::for_testing()` compiles with the new fields set.
5. With `WEATHER_ANCHORS` unset, the weather task is not spawned (verify with a tracing
   log or by checking the config field is empty). No panic on missing anchors.
6. With `NOTIFICATIONS_ENABLED=false` (or unset), no HTTP calls to ntfy are made.

Write at least one unit test for `parse_weather_anchors` covering valid input, malformed
entries (missing lat/lon), and empty string.
