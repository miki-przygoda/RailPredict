//! RailPredict entry point.
//!
//! ## Task topology
//!
//! ```text
//! main
//!  ├── PollManager::run        — global BinaryHeap poll scheduler
//!  ├── IngestionPipeline::run  — Darwin STOMP firehose → registry writes
//!  ├── eviction_task           — 60s tick; calls registry.evict_departed()
//!  └── axum server             — HTTP API + SSE
//! ```
//!
//! All tasks share a `CancellationToken`. On `ctrl_c`, the token is cancelled and
//! each task's `tokio::select!` exits cleanly. Tasks are joined before the process
//! exits so the runtime flushes any in-flight async work.
//!
//! ## State-change channel
//! A single `broadcast::channel` is written by both `PollManager` (poll ticks) and
//! `IngestionPipeline` (emergency promotions). The axum SSE handler subscribes to it
//! via `AppState::state_change_tx.subscribe()` — one subscriber per open SSE connection.

use std::sync::Arc;

use chrono::Utc;
use tokio::sync::broadcast;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

use railpredict::api::{router, AppState};
use railpredict::cache::TrainRegistry;
use railpredict::config::{Config, LogFormat};
use railpredict::ingestion::stomp_client::LiveStompClient;
use railpredict::ingestion::IngestionPipeline;
use railpredict::prediction::PredictionEngine;
use railpredict::state_machine::PollManager;

fn init_tracing(config: &Config) {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.log_level));

    match config.log_format {
        LogFormat::Json => {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json())
                .init();
        }
        LogFormat::Pretty => {
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().pretty())
                .init();
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let config = Config::from_env().map_err(|e| anyhow::anyhow!("{e}"))?;

    init_tracing(&config);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "RailPredict starting");

    let registry = Arc::new(TrainRegistry::new());
    let prediction_engine = PredictionEngine::new();

    // Single broadcast channel shared by PollManager, IngestionPipeline, and SSE handlers.
    // Buffer of 1024: at peak (~400 msg/s Darwin) this gives ~2.5 seconds headroom before
    // lagged SSE receivers start dropping events (acceptable — they see a gap in updates).
    let (sc_tx, _initial_rx) = broadcast::channel(1024);
    // Drop _initial_rx; receivers are created on demand via sc_tx.subscribe().
    drop(_initial_rx);

    let token = CancellationToken::new();

    // --- Poll manager ---
    let (poll_manager, _pm_handles) = PollManager::new(sc_tx.clone());
    let pm_token = token.clone();
    let pm_task = tokio::spawn(async move {
        tokio::select! {
            _ = pm_token.cancelled() => tracing::info!("PollManager shutting down"),
            _ = poll_manager.run() => tracing::warn!("PollManager exited early"),
        }
    });

    // --- Ingestion pipeline ---
    let stomp = match LiveStompClient::from_env() {
        Ok(client) => Box::new(client),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Darwin credentials not configured — ingestion disabled. \
                 Set DARWIN_HOST, DARWIN_USERNAME, DARWIN_PASSWORD to enable."
            );
            let pipeline_token = token.clone();
            let _no_op = tokio::spawn(async move {
                pipeline_token.cancelled().await;
            });
            wait_for_shutdown(token, pm_task, registry, sc_tx, &config).await;
            return Ok(());
        }
    };

    let pipeline = IngestionPipeline::new(
        stomp,
        config.watched_routes.clone(),
        Arc::clone(&registry),
        sc_tx.clone(),
        prediction_engine,
    );

    let pipeline_token = token.clone();
    let pipeline_task = tokio::spawn(async move {
        tokio::select! {
            _ = pipeline_token.cancelled() => tracing::info!("Ingestion pipeline shutting down"),
            _ = pipeline.run() => tracing::warn!("Ingestion pipeline exited early"),
        }
    });

    // --- Cache eviction background task (60s tick) ---
    let eviction_registry = Arc::clone(&registry);
    let eviction_token = token.clone();
    let eviction_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = eviction_token.cancelled() => {
                    tracing::info!("Eviction task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    let before = eviction_registry.len();
                    eviction_registry.evict_departed(Utc::now()).await;
                    let evicted = before.saturating_sub(eviction_registry.len());
                    if evicted > 0 {
                        tracing::info!(evicted, "Evicted departed trains from registry");
                    }
                }
            }
        }
    });

    // --- axum HTTP server ---
    let app_state = AppState {
        registry: Arc::clone(&registry),
        state_change_tx: sc_tx,
    };
    let app = router(app_state);
    let bind_addr: std::net::SocketAddr = config
        .api_bind_addr
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid API_BIND_ADDR '{}': {e}", config.api_bind_addr))?;

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(addr = %bind_addr, "HTTP API listening");

    let api_token = token.clone();
    let api_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { api_token.cancelled().await })
            .await
            .ok();
        tracing::info!("HTTP API server stopped");
    });

    // Wait for shutdown signal, then drain all tasks.
    tokio::signal::ctrl_c().await?;
    tracing::info!("Ctrl-C received — initiating graceful shutdown");
    token.cancel();

    let _ = tokio::join!(pm_task, pipeline_task, eviction_task, api_task);
    tracing::info!("All tasks stopped. Goodbye.");

    Ok(())
}

async fn wait_for_shutdown(
    token: CancellationToken,
    pm_task: tokio::task::JoinHandle<()>,
    registry: Arc<TrainRegistry>,
    sc_tx: broadcast::Sender<railpredict::state_machine::poll_manager::StateChangeEvent>,
    config: &Config,
) {
    // Start the API server even without Darwin, so health + departure board are available.
    let app_state = AppState { registry, state_change_tx: sc_tx };
    let app = router(app_state);

    if let Ok(addr) = config.api_bind_addr.parse::<std::net::SocketAddr>() {
        if let Ok(listener) = tokio::net::TcpListener::bind(addr).await {
            tracing::info!(addr = %addr, "HTTP API listening (ingestion disabled)");
            let api_token = token.clone();
            tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async move { api_token.cancelled().await })
                    .await
                    .ok();
            });
        }
    }

    tokio::signal::ctrl_c().await.ok();
    tracing::info!("Ctrl-C received — initiating graceful shutdown");
    token.cancel();
    let _ = pm_task.await;
    tracing::info!("All tasks stopped. Goodbye.");
}
