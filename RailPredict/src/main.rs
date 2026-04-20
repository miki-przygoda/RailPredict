//! RailPredict entry point.
//!
//! ## Task topology
//!
//! ```text
//! main
//!  ├── db::connect + load_history
//!  ├── registry warm-up        — pre-register today's timetable (Tier A)
//!  ├── PollManager::run        — global BinaryHeap poll scheduler
//!  ├── IngestionPipeline::run  — Darwin STOMP firehose → registry writes
//!  ├── eviction_task           — 60s tick; calls registry.evict_departed()
//!  ├── db_flush_task           — 60s tick; flushes delay history to DB
//!  ├── prune_task              — 24h tick; deletes old timetable/history rows
//!  ├── poll_consumer_task      — optional; wires GBR REST polling (requires GBR_API_KEY)
//!  ├── weather_task            — optional; 10min Open-Meteo wind poll → VolatilityStore
//!  ├── notification_task       — optional; ntfy push on Critical promotions
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
use railpredict::networking::{CircuitBreaker, Coalescer, LiveGbrClient, RateLimiter};
use railpredict::prediction::PredictionEngine;
use railpredict::state_machine::PollManager;
use railpredict::types::{TrainId, TrainStatus};

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
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("Failed to install rustls ring crypto provider"))?;

    dotenvy::dotenv().ok();

    let config = Config::from_env().map_err(|e| anyhow::anyhow!("{e}"))?;

    init_tracing(&config);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "RailPredict starting");

    // --- CLI subcommands ---
    let cli = Cli::parse();
    if let Some(command) = cli.command {
        let db_pool = db::connect(&config.database_url, config.db_max_connections).await?;
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
                    anyhow::bail!("CIF ingest is not yet implemented");
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
    let db_pool = db::connect(&config.database_url, config.db_max_connections).await?;

    // Load Tier B history from DB into in-memory store.
    let history_store = db::history::load_history(&db_pool).await?;
    let history_store = Arc::new(history_store);
    let prediction_engine = PredictionEngine::with_store(Arc::clone(&history_store));

    let registry = Arc::new(TrainRegistry::new());
    let volatility_store: railpredict::weather::VolatilityStore = Arc::new(dashmap::DashMap::new());

    // --- Proactive warm-up: pre-register today's future departures (Tier A) ---
    // Ensures trains are in the registry before Darwin mentions them, so state
    // transitions and predictions can start immediately on first message arrival.
    match db::maintenance::load_todays_calls(&db_pool).await {
        Ok(calls) => {
            let today = chrono::Utc::now().date_naive();
            let statuses: Vec<(TrainId, TrainStatus)> = calls
                .into_iter()
                .filter_map(|call| {
                    let time = call.scheduled_departure?;
                    let dt = chrono::NaiveDateTime::new(today, time).and_utc();
                    // UIDs from the timetable may be shorter/longer than 6 chars on a fresh
                    // install. Skip those that fail validation rather than panicking.
                    let id = TrainId::uid(&call.uid).ok()?;
                    let mut status = TrainStatus::new(id.clone(), dt, dt);
                    status.origin_crs = Some(call.location_crs);
                    status.uid = Some(call.uid);
                    Some((id, status))
                })
                .collect();
            let count = statuses.len();
            registry.warm(statuses);
            tracing::info!(trains = count, "Pre-warmed registry from today's timetable");
        }
        Err(e) => tracing::warn!(error = %e, "Registry warm-up from timetable skipped"),
    }

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

    // --- DB prune background task (24h tick) ---
    let prune_db = db_pool.clone();
    let prune_token = token.clone();
    let prune_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(86400)); // 24h
        loop {
            tokio::select! {
                _ = prune_token.cancelled() => {
                    tracing::info!("Prune task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    match db::maintenance::prune_old_rows(&prune_db).await {
                        Ok((tc, dh)) => tracing::debug!(
                            timetable_rows = tc,
                            history_rows = dh,
                            "DB prune complete"
                        ),
                        Err(e) => tracing::error!(error = %e, "DB prune failed"),
                    }
                }
            }
        }
    });

    // --- Weather polling task (optional — only when WEATHER_ANCHORS is set) ---
    if !config.weather_anchors.is_empty() {
        let store = Arc::clone(&volatility_store);
        let anchors: Vec<railpredict::weather::WeatherAnchor> = config.weather_anchors.iter()
            .map(|(id, lat, lon)| railpredict::weather::WeatherAnchor {
                route_id: id.clone(),
                lat: *lat,
                lon: *lon,
            })
            .collect();
        let http_client = reqwest::Client::new();
        tokio::spawn(railpredict::weather::run_weather_task(store, anchors, http_client));
        tracing::info!(anchors = config.weather_anchors.len(), "Weather polling task started");
    }

    // --- Poll consumer task (Tier C — only when GBR_API_KEY is set) ---
    let gbr_key = std::env::var("GBR_API_KEY").unwrap_or_default();
    let poll_task: Option<tokio::task::JoinHandle<()>> = if gbr_key.is_empty() {
        tracing::warn!("GBR_API_KEY not set — poll consumer disabled, Tier C inactive");
        None
    } else {
        let gbr_client = match LiveGbrClient::from_env() {
            Ok(c) => Arc::new(c) as Arc<dyn railpredict::networking::gbr_client::GbrClient>,
            Err(e) => {
                tracing::error!(error = %e, "Failed to build GBR client — poll consumer disabled");
                return Err(e);
            }
        };
        let coalescer = Arc::new(Coalescer::new(gbr_client));
        let rate_limiter = Arc::new(RateLimiter::default_gbr());
        let cb = Arc::new(CircuitBreaker::default_gbr());

        let mut poll_rx = sc_tx.subscribe();
        let poll_registry = Arc::clone(&registry);
        let poll_token = token.clone();

        Some(tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = poll_token.cancelled() => {
                        tracing::info!("Poll consumer shutting down");
                        break;
                    }
                    result = poll_rx.recv() => {
                        let event = match result {
                            Ok(e) => e,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!(skipped = n, "Poll consumer lagged — skipping events");
                                continue;
                            }
                            Err(_) => break,
                        };

                        // Only process same-state events (poll interval fired, not a real promotion).
                        // Real state promotions have old_state != new_state.
                        if event.old_state != event.new_state {
                            continue;
                        }
                        // Only Active/Critical trains need live GBR polling.
                        use railpredict::state_machine::train_state::TrainState;
                        if !matches!(event.new_state, TrainState::Active | TrainState::Critical) {
                            continue;
                        }

                        // Check circuit breaker before consuming a rate-limit token.
                        if !cb.is_request_allowed().await {
                            metrics::counter!("circuit_breaker_blocked_total").increment(1);
                            continue;
                        }

                        // Acquire a rate-limit token (sleeps if bucket is empty).
                        rate_limiter.acquire().await;

                        let train_id = event.train_id.clone();
                        let coalescer = Arc::clone(&coalescer);
                        let cb_clone = Arc::clone(&cb);
                        let reg_clone = Arc::clone(&poll_registry);
                        let volatility_store_poll = Arc::clone(&volatility_store);

                        tokio::spawn(async move {
                            match coalescer.get(train_id.clone()).await {
                                Ok(status) => {
                                    cb_clone.record_success().await;
                                    reg_clone
                                        .update(&train_id, |existing| {
                                            // Only apply if GBR data is at least as fresh as stored data.
                                            let gbr_ts =
                                                status.actual_estimated_departure.last_updated;
                                            if gbr_ts
                                                >= existing
                                                    .actual_estimated_departure
                                                    .last_updated
                                            {
                                                existing.actual_estimated_departure =
                                                    status.actual_estimated_departure.clone();
                                                existing.reported_delay_mins =
                                                    status.reported_delay_mins.clone();
                                            }
                                            if gbr_ts >= existing.actual_platform.last_updated {
                                                existing.actual_platform =
                                                    status.actual_platform.clone();
                                            }
                                            if gbr_ts >= existing.is_cancelled.last_updated {
                                                existing.is_cancelled =
                                                    status.is_cancelled.clone();
                                            }
                                            existing.last_update_source =
                                                status.last_update_source;
                                        })
                                        .await;
                                    tracing::debug!(
                                        train_id = %train_id,
                                        "GBR poll applied to registry"
                                    );

                                    // Weather-driven volatility: check if any anchor covers this train's origin
                                    if let Some(origin) = status.origin_crs.as_deref()
                                        && let Some(wind_entry) = volatility_store_poll.get(origin) {
                                            let wind_mph = *wind_entry;
                                            if wind_mph > 50.0 {
                                                reg_clone.update(&train_id, |existing| {
                                                    existing.volatility.wind_speed_mph = Some(wind_mph);
                                                    existing.volatility.incident_flagged = true;
                                                }).await;
                                                tracing::info!(
                                                    train_id = %train_id,
                                                    wind_mph,
                                                    "Wind-driven volatility promotion"
                                                );
                                            }
                                    }
                                }
                                Err(e) => {
                                    use railpredict::networking::coalescer::CoalescerError;
                                    match &e {
                                        CoalescerError::GbrError(msg) if msg.contains("503") => {
                                            cb_clone.record_failure().await;
                                            tracing::warn!(
                                                "GBR returned 503 — circuit breaker incremented"
                                            );
                                        }
                                        CoalescerError::GbrError(msg) if msg.contains("429") => {
                                            tracing::warn!("GBR rate limited — backing off");
                                        }
                                        CoalescerError::InFlightDropped => {
                                            tracing::debug!(
                                                train_id = %train_id,
                                                "In-flight GBR request dropped"
                                            );
                                        }
                                        _ => {
                                            tracing::warn!(error = %e, "GBR poll error");
                                        }
                                    }
                                }
                            }
                        });
                    }
                }
            }
        }))
    };

    // --- Push notification task (optional — only when enabled + NTFY_URL set) ---
    if config.notifications_enabled
        && let Some(ntfy_url) = config.ntfy_url.clone() {
            let mut notif_rx = sc_tx.subscribe();
            let http_client = reqwest::Client::new();
            tokio::spawn(async move {
                loop {
                    match notif_rx.recv().await {
                        Ok(event)
                            if event.new_state
                                == railpredict::state_machine::train_state::TrainState::Critical
                                && event.old_state
                                    != railpredict::state_machine::train_state::TrainState::Critical =>
                        {
                            let title = format!("Train {} now Critical", event.train_id);
                            let body = "State promotion detected — check live updates".to_string();
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
            tracing::info!("Push notification task started");
    }

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
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move { api_token.cancelled().await })
        .await
        .ok();
        tracing::info!("HTTP API server stopped");
    });

    // Wait for SIGTERM or Ctrl-C.
    shutdown_signal().await;
    tracing::info!("Shutdown signal received — initiating graceful shutdown");
    token.cancel();

    let _ = tokio::join!(pm_task, pipeline_task, eviction_task, flush_task, prune_task, api_task);
    if let Some(pt) = poll_task {
        let _ = pt.await;
    }

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

#[allow(clippy::too_many_arguments)]
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

    if let Ok(addr) = config.api_bind_addr.parse::<std::net::SocketAddr>()
        && let Ok(listener) = tokio::net::TcpListener::bind(addr).await
    {
        tracing::info!(addr = %addr, "HTTP API listening (ingestion disabled)");
        let api_token = token.clone();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async move { api_token.cancelled().await })
                .await
                .ok();
        });
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
