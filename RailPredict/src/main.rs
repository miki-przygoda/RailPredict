//! RailPredict entry point.
//!
//! ## Task topology
//!
//! ```text
//! main
//!  ├── db::connect + load_history
//!  ├── PollManager::run        — global BinaryHeap poll scheduler
//!  ├── IngestionPipeline::run  — Darwin STOMP firehose → registry writes
//!  ├── eviction_task           — 60s tick; calls registry.evict_departed()
//!  ├── db_flush_task           — 60s tick; flushes delay history to DB
//!  └── axum server             — HTTP API + SSE
//! ```
//!
//! All tasks share a `CancellationToken`. On SIGTERM or Ctrl-C, the token is
//! cancelled, each task's `tokio::select!` exits cleanly, and a final history
//! flush runs before the process exits.

use std::sync::Arc;

use chrono::Utc;
use clap::Parser;
use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::sync::broadcast;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

use railpredict::api::{router, AppState};
use railpredict::cache::TrainRegistry;
use railpredict::cli::{Cli, Commands, IngestSource};
use railpredict::config::{Config, LogFormat};
use railpredict::db;
use railpredict::ingestion::gtfs;
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

    // --- CLI subcommands ---
    let cli = Cli::parse();
    if let Some(command) = cli.command {
        let db_pool = db::connect(&config.database_url).await?;
        match command {
            Commands::IngestStatic { source, url, file } => match source {
                IngestSource::Gtfs => {
                    if let Some(path) = file {
                        let count = gtfs::run_ingest_from_file(&db_pool, &path).await?;
                        tracing::info!(stations = count, "GTFS ingest from file complete");
                    } else {
                        let url = url.ok_or_else(|| {
                            anyhow::anyhow!("Either --url or --file must be provided for GTFS ingest")
                        })?;
                        let count = gtfs::run_ingest(&db_pool, &url).await?;
                        tracing::info!(stations = count, "GTFS ingest complete");
                    }
                }
                IngestSource::Cif => {
                    unimplemented!("CIF ingest is not yet implemented");
                }
            },
        }
        return Ok(());
    }

    // --- Install Prometheus metrics recorder (Phase 1) ---
    // Must be installed before any metrics::* macros are called.
    let prometheus_handle = PrometheusBuilder::new()
        .install_recorder()
        .map_err(|e| anyhow::anyhow!("Failed to install Prometheus recorder: {e}"))?;
    let prometheus_handle = Arc::new(prometheus_handle);
    tracing::info!("Prometheus metrics recorder installed — GET /metrics enabled");

    // --- Normal server startup ---
    let db_pool = db::connect(&config.database_url).await?;

    // Load Tier B history from DB into in-memory store.
    let history_store = db::history::load_history(&db_pool).await?;
    let history_store = Arc::new(history_store);
    let prediction_engine = PredictionEngine::with_store(Arc::clone(&history_store));

    let registry = Arc::new(TrainRegistry::new());

    // Single broadcast channel shared by PollManager, IngestionPipeline, and SSE handlers.
    let (sc_tx, _initial_rx) = broadcast::channel(1024);
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
    // Build the initial STOMP client to verify credentials are present before spawning.
    let initial_stomp = match LiveStompClient::from_env() {
        Ok(client) => Box::new(client),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Darwin credentials not configured — ingestion disabled."
            );
            wait_for_shutdown(
                token, pm_task, registry, sc_tx, &config,
                Arc::clone(&history_store), db_pool, Arc::clone(&prometheus_handle),
            )
            .await;
            return Ok(());
        }
    };

    let initial_pipeline = IngestionPipeline::new(
        initial_stomp,
        config.watched_routes.clone(),
        Arc::clone(&registry),
        sc_tx.clone(),
        prediction_engine,
    );

    let pipeline_token = token.clone();
    let pipeline_task = tokio::spawn(async move {
        // Exponential-backoff reconnect loop.
        // Darwin disconnects clients roughly every 30 minutes; the sequence guard in
        // filter.rs handles replayed messages on reconnect — no duplicate-suppression
        // changes needed here.
        let mut delay = Duration::from_secs(2);
        let mut attempt: u32 = 0;

        // Extract the shared context before the first run so we can rebuild after
        // `run()` consumes the pipeline.
        let pipeline_ctx = initial_pipeline.context();
        let mut pipeline = initial_pipeline;

        loop {
            tokio::select! {
                _ = pipeline_token.cancelled() => {
                    tracing::info!("Ingestion pipeline shutting down");
                    break;
                }
                result = pipeline.run() => {
                    match result {
                        Ok(()) => {
                            // Clean shutdown (channel drained without error).
                            tracing::info!("Ingestion pipeline exited cleanly");
                            break;
                        }
                        Err(e) => {
                            attempt += 1;
                            tracing::warn!(
                                error = %e,
                                attempt,
                                retry_secs = delay.as_secs(),
                                "STOMP stream closed, reconnecting"
                            );
                        }
                    }
                }
            }

            // Wait for the backoff delay or cancellation — whichever comes first.
            tokio::select! {
                _ = pipeline_token.cancelled() => {
                    tracing::info!("Ingestion pipeline shutting down during backoff");
                    break;
                }
                _ = tokio::time::sleep(delay) => {}
            }
            delay = (delay * 2).min(Duration::from_secs(120));

            // Rebuild the STOMP client for the next attempt, reusing the same context.
            match LiveStompClient::from_env() {
                Ok(new_stomp) => {
                    pipeline = IngestionPipeline::from_context(
                        pipeline_ctx.clone(),
                        Box::new(new_stomp),
                    );
                }
                Err(e) => {
                    tracing::error!(error = %e, "Cannot rebuild STOMP client — giving up reconnect");
                    break;
                }
            }
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
                    let after = eviction_registry.len();
                    let evicted = before.saturating_sub(after);
                    if evicted > 0 {
                        tracing::info!(evicted, "Evicted departed trains from registry");
                    }
                    // Phase 2: registry size gauge — updated every 60s alongside eviction.
                    metrics::gauge!("registry_train_count").set(after as f64);
                }
            }
        }
    });

    // --- DB flush background task (60s tick) ---
    let flush_store = Arc::clone(&history_store);
    let flush_db = db_pool.clone();
    let flush_token = token.clone();
    let flush_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = flush_token.cancelled() => {
                    tracing::info!("DB flush task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    if let Err(e) = db::history::flush_history(&flush_db, &flush_store).await {
                        tracing::error!(error = %e, "DB flush failed");
                    }
                }
            }
        }
    });

    // --- axum HTTP server ---
    let app_state = AppState {
        registry: Arc::clone(&registry),
        state_change_tx: sc_tx,
        db: db_pool.clone(),
        prometheus: Arc::clone(&prometheus_handle),
        cors_allowed_origins: config.cors_allowed_origins.clone(),
        http_rate_limit_per_sec: config.http_rate_limit_per_sec,
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

    // Wait for SIGTERM or Ctrl-C.
    shutdown_signal().await;
    tracing::info!("Shutdown signal received — initiating graceful shutdown");
    token.cancel();

    let _ = tokio::join!(pm_task, pipeline_task, eviction_task, flush_task, api_task);

    // Final flush before exit.
    if let Err(e) = db::history::flush_history(&db_pool, &history_store).await {
        tracing::error!(error = %e, "Final DB flush failed");
    }

    tracing::info!("All tasks stopped. Goodbye.");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.ok() };

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm.recv() => {},
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await;
}

async fn wait_for_shutdown(
    token: CancellationToken,
    pm_task: tokio::task::JoinHandle<()>,
    registry: Arc<TrainRegistry>,
    sc_tx: broadcast::Sender<railpredict::state_machine::poll_manager::StateChangeEvent>,
    config: &Config,
    history_store: Arc<railpredict::prediction::types::HistoricalStore>,
    db_pool: db::Db,
    prometheus_handle: Arc<metrics_exporter_prometheus::PrometheusHandle>,
) {
    let app_state = AppState {
        registry,
        state_change_tx: sc_tx,
        db: db_pool.clone(),
        prometheus: prometheus_handle,
        cors_allowed_origins: config.cors_allowed_origins.clone(),
        http_rate_limit_per_sec: config.http_rate_limit_per_sec,
    };
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

    shutdown_signal().await;
    tracing::info!("Shutdown signal received — initiating graceful shutdown");
    token.cancel();
    let _ = pm_task.await;

    if let Err(e) = db::history::flush_history(&db_pool, &history_store).await {
        tracing::error!(error = %e, "Final DB flush failed");
    }

    tracing::info!("All tasks stopped. Goodbye.");
}
