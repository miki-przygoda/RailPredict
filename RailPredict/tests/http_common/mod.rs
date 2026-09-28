//! Shared zero-dependency HTTP test harness.
//!
//! Spins the *real* axum app up on an ephemeral port and hands back a `reqwest`
//! client to drive it — no `tower`/`axum-test`/`scraper`, only crates already in
//! `Cargo.toml`. Included via `mod http_common;` from each `tests/http_*.rs` file;
//! as a subdirectory module it is NOT compiled as its own (empty) test binary.

// Each test binary uses only a subset of these helpers, so cross-binary dead_code
// is expected and unavoidable for a shared test module.
#![allow(dead_code)]

use std::sync::Arc;

use railpredict::api::{router, AppState};
use railpredict::cache::{StationIndex, TrainRegistry};
use railpredict::config::Config;
use railpredict::ingestion::feed_health::FeedHealth;
use railpredict::ingestion::gtfs::IngestStatus;
use tokio::sync::{broadcast, watch};
use tokio_util::sync::{CancellationToken, DropGuard};

pub struct TestApp {
    pub base_url: String,
    pub client: reqwest::Client,
    pub db: sqlx::PgPool,
    /// The same `Arc<TrainRegistry>` the served app holds — seed it from a test
    /// (e.g. `app.registry.upsert(...)`) and the running handlers see the change.
    pub registry: Arc<TrainRegistry>,
    /// Cancels the spawned server when the `TestApp` is dropped.
    _shutdown: DropGuard,
}

impl TestApp {
    /// `GET {base}{path}` → `(status, body)`.
    pub async fn get(&self, path: &str) -> (reqwest::StatusCode, String) {
        let resp = self
            .client
            .get(format!("{}{}", self.base_url, path))
            .send()
            .await
            .expect("request send failed");
        let status = resp.status();
        let body = resp.text().await.expect("body read failed");
        (status, body)
    }

    /// `GET {base}{path}` → `(status, parsed JSON)`.
    pub async fn get_json(&self, path: &str) -> (reqwest::StatusCode, serde_json::Value) {
        let resp = self
            .client
            .get(format!("{}{}", self.base_url, path))
            .send()
            .await
            .expect("request send failed");
        let status = resp.status();
        let json = resp.json::<serde_json::Value>().await.unwrap_or(serde_json::Value::Null);
        (status, json)
    }
}

/// Spawn the real app on an ephemeral port using a migrated `pool`. `stations`
/// seeds the in-memory autocomplete index (`(crs, name)` pairs).
pub async fn spawn_app(pool: sqlx::PgPool, stations: Vec<(String, String)>) -> TestApp {
    spawn_app_with_feed(pool, stations, Arc::new(FeedHealth::default())).await
}

/// Like [`spawn_app`] but with an explicit Darwin feed-freshness tracker, so a
/// test can simulate a fresh, stale or disabled feed behind `/health`.
pub async fn spawn_app_with_feed(
    pool: sqlx::PgPool,
    stations: Vec<(String, String)>,
    feed_health: Arc<FeedHealth>,
) -> TestApp {
    let config = Config::for_testing();

    let registry = Arc::new(TrainRegistry::new());
    let (state_change_tx, _rx) = broadcast::channel(1024);

    // Build a Prometheus recorder WITHOUT installing the process-global one:
    // `install_recorder()` errors on the second call, which would break a run
    // that builds several test apps. `build_recorder().handle()` is local.
    let prometheus = Arc::new(
        metrics_exporter_prometheus::PrometheusBuilder::new()
            .build_recorder()
            .handle(),
    );

    let (ingest_tx, _ingest_rx) = watch::channel(IngestStatus::default());
    let station_index = Arc::new(StationIndex::build(stations));

    let state = AppState {
        registry: Arc::clone(&registry),
        state_change_tx,
        db: pool.clone(),
        prometheus,
        cors_allowed_origins: config.cors_allowed_origins.clone(),
        http_rate_limit_per_sec: config.http_rate_limit_per_sec,
        ingest_status: Arc::new(ingest_tx),
        station_index,
        feed_health,
    };

    let app = router(state);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");

    // Serve exactly as main.rs does (connect-info is required for routing).
    let token = CancellationToken::new();
    let guard = token.clone().drop_guard();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move { token.cancelled().await })
        .await
        .ok();
    });

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client");

    TestApp {
        base_url: format!("http://{addr}"),
        client,
        db: pool,
        registry,
        _shutdown: guard,
    }
}

/// Harness with no seeded stations (for tests that don't exercise autocomplete).
pub async fn spawn_app_empty(pool: sqlx::PgPool) -> TestApp {
    spawn_app(pool, Vec::new()).await
}
