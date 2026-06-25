//! RailPredict entry point.
//!
//! ## Task topology
//!
//! ```text
//! main
//!  ├── db::connect + load_history
//!  ├── registry warm-up        — pre-register today's timetable (Tier A)
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
use tokio::sync::{broadcast, watch};
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

use railpredict::api::{router, AppState};
use railpredict::cache::{StationIndex, TrainRegistry};
use railpredict::cli::{Cli, Commands, IngestSource};
use railpredict::export;
use railpredict::config::{Config, LogFormat};
use railpredict::db;
use railpredict::ingestion::gtfs::{self, IngestPhase, IngestStatus, run_ingest_with_watch};
use railpredict::ingestion::stomp_client::LiveStompClient;
use railpredict::ingestion::IngestionPipeline;
use railpredict::networking::{CircuitBreaker, Coalescer, LiveGbrClient, RateLimiter};
use railpredict::prediction::{OnnxEngine, PredictionEngine};
use railpredict::types::{TrainId, TrainStatus};

/// Prints a startup diagnostics table to stderr before the structured logger initialises,
/// so the report is always readable regardless of LOG_FORMAT (pretty/json).
///
/// Uses `ok` / `--` / `!!` markers instead of Unicode symbols for maximum terminal compat.
fn print_startup_report() {
    const W: usize = 28;
    let sep = "─".repeat(58);

    eprintln!();
    eprintln!("{sep}");
    eprintln!("  RailPredict v{}  —  startup", env!("CARGO_PKG_VERSION"));
    eprintln!("{sep}");
    eprintln!();

    let db = std::env::var("DATABASE_URL").unwrap_or_default();
    let darwin_host = std::env::var("DARWIN_HOST").unwrap_or_default();
    let darwin_user = std::env::var("DARWIN_USERNAME").unwrap_or_default();
    let darwin_pass = std::env::var("DARWIN_PASSWORD").unwrap_or_default();
    let gbr_key = std::env::var("GBR_API_KEY").unwrap_or_default();
    let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let cors = std::env::var("CORS_ALLOWED_ORIGINS").unwrap_or_default();

    // Helpers — macros so the label width constant W is in scope.
    macro_rules! ok   { ($lbl:expr, $note:expr) => { eprintln!("    {:<W$} ok  {}", $lbl, $note) }; }
    macro_rules! opt  { ($lbl:expr, $note:expr) => { eprintln!("    {:<W$} --  {}", $lbl, $note) }; }
    macro_rules! miss { ($lbl:expr, $note:expr) => { eprintln!("    {:<W$} !!  {}", $lbl, $note) }; }

    // --- Core ---
    eprintln!("  core");
    if db.is_empty() {
        miss!("DATABASE_URL", "not set  <- server cannot start without this");
    } else {
        ok!("DATABASE_URL", "configured");
    }
    let bind = std::env::var("API_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000 (default)".to_string());
    ok!("API_BIND_ADDR", bind);
    eprintln!();

    // --- Live ingestion ---
    let darwin_ok = !darwin_host.is_empty() && !darwin_user.is_empty() && !darwin_pass.is_empty();
    eprintln!("  live ingestion  (Darwin Push Port)");
    if darwin_host.is_empty()  { miss!("DARWIN_HOST",     "not set"); }
    else                       { ok!(  "DARWIN_HOST",     &darwin_host); }
    if darwin_user.is_empty()  { miss!("DARWIN_USERNAME", "not set"); }
    else                       { ok!(  "DARWIN_USERNAME", "configured"); }
    if darwin_pass.is_empty()  { miss!("DARWIN_PASSWORD", "not set"); }
    else                       { ok!(  "DARWIN_PASSWORD", "set"); }
    if !darwin_ok {
        eprintln!("    -> live arrival updates disabled");
        eprintln!("       set DARWIN_HOST / DARWIN_USERNAME / DARWIN_PASSWORD to enable");
    }
    eprintln!();

    // --- REST polling ---
    eprintln!("  REST polling  (GBR Retail API)");
    if gbr_key.is_empty() {
        miss!("GBR_API_KEY", "not set  -> fares and seat data disabled");
    } else {
        ok!("GBR_API_KEY", "set");
    }
    eprintln!();

    // --- Security ---
    let cors_required = log_level != "debug";
    eprintln!("  security");
    if cors.is_empty() && cors_required {
        miss!("CORS_ALLOWED_ORIGINS", "not set  <- required in production (LOG_LEVEL != debug)");
    } else if cors.is_empty() {
        opt!("CORS_ALLOWED_ORIGINS", "not set  (permissive — dev mode only)");
    } else {
        let n = cors.split(',').filter(|s| !s.trim().is_empty()).count();
        ok!("CORS_ALLOWED_ORIGINS", format!("{n} origin(s) configured"));
    }
    eprintln!();

    // --- Optional ---
    eprintln!("  optional");
    let routes = std::env::var("WATCHED_ROUTES").unwrap_or_default();
    if routes.is_empty() { opt!("WATCHED_ROUTES",   "not set  (watching all routes)"); }
    else                 { ok!( "WATCHED_ROUTES",   &routes); }
    let ntfy = std::env::var("NTFY_URL").unwrap_or_default();
    if ntfy.is_empty() { opt!("NTFY_URL",           "not set  (push notifications off)"); }
    else               { ok!( "NTFY_URL",           &ntfy); }
    let weather = std::env::var("WEATHER_ANCHORS").unwrap_or_default();
    if weather.is_empty() { opt!("WEATHER_ANCHORS", "not set  (weather polling off)"); }
    else                  { ok!( "WEATHER_ANCHORS", "configured"); }

    eprintln!();
    eprintln!("{sep}");
    eprintln!();
}

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
    print_startup_report();

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
                IngestSource::Rds => {
                    let dir = file.ok_or_else(|| {
                        anyhow::anyhow!("--file <dir> (the imports/ directory) is required for RDS ingest")
                    })?;
                    let summary =
                        railpredict::ingestion::rds::run_ingest_rds(&db_pool, &dir).await?;
                    tracing::info!(
                        stations = summary.stations,
                        operators = summary.operators,
                        "RDS reference ingest complete"
                    );
                }
            },
            Commands::ExportSite { output, days } => {
                export::export_site(&db_pool, &output, days).await?;
            }
            Commands::ExportMap { output, days } => {
                export::map::export_map(&db_pool, &output, days).await?;
            }
            Commands::ExportOs { output, days } => {
                export::os::export_os(&db_pool, &output, days).await?;
            }
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

    // Load ONNX ML models (optional — gracefully falls back to statistical engine).
    let onnx_engine = Arc::new(
        OnnxEngine::load(std::path::Path::new("models")).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "ONNX engine failed to load — using statistical fallback");
            OnnxEngine::default()
        }),
    );
    let prediction_engine = PredictionEngine::with_store_and_onnx(
        Arc::clone(&history_store),
        Arc::clone(&onnx_engine),
    );

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

    // Watch channel for GTFS ingest progress — updated by the ingest UI task.
    // Created here (before the startup auto-ingest below) so the same channel that
    // feeds `AppState.ingest_status` also carries startup-ingest progress, which the
    // /ui/dev/ingest/stream SSE handler subscribes to.
    let (ingest_tx, _ingest_rx) = watch::channel(IngestStatus::default());
    let ingest_tx = Arc::new(ingest_tx);

    // --- Auto-ingest: seed stations on first run if GTFS_URL is configured ---
    // If the stations table is empty AND GTFS_URL is set, kick off a background
    // ingest so the departure board and autocomplete work immediately without
    // any manual step from the operator.
    let station_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM stations")
        .fetch_one(&db_pool)
        .await
        .unwrap_or(0);

    if station_count == 0 {
        if let Ok(gtfs_url) = std::env::var("GTFS_URL") {
            if !gtfs_url.trim().is_empty() {
                tracing::info!("Stations table empty — auto-ingesting from GTFS_URL on startup");
                let auto_db = db_pool.clone();
                // Use the SHARED ingest channel so dev SSE subscribers see startup progress.
                let auto_tx = Arc::clone(&ingest_tx);
                tokio::spawn(async move {
                    auto_tx.send_modify(|s| {
                        s.phase = IngestPhase::Downloading;
                        s.started_at = Some(Utc::now());
                        s.log.push(format!("Auto-ingest: {gtfs_url}"));
                    });
                    match run_ingest_with_watch(&auto_db, &gtfs_url, &auto_tx).await {
                        Ok(_) => tracing::info!("Startup auto-ingest complete"),
                        Err(e) => tracing::warn!(error = %e, "Startup auto-ingest failed"),
                    }
                });
            }
        } else {
            tracing::warn!(
                "Stations table is empty — set GTFS_URL or use /dev to ingest timetable data"
            );
        }
    }

    // Single broadcast channel shared by the IngestionPipeline and SSE handlers
    // (state-change events drive the live UI; emitted from the ingestion pipeline).
    let (sc_tx, _initial_rx) = broadcast::channel(1024);
    drop(_initial_rx);

    let token = CancellationToken::new();

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
                token, registry, sc_tx, &config,
                Arc::clone(&history_store), db_pool, Arc::clone(&prometheus_handle),
                Arc::clone(&ingest_tx),
            )
            .await;
            return Ok(());
        }
    };

    // Clone before move — shares the same Arc<HistoricalStore>, so records from
    // the heartbeat task are visible to the ingestion pipeline and the flush task.
    let heartbeat_engine   = prediction_engine.clone();
    let heartbeat_registry = Arc::clone(&registry);

    let initial_pipeline = IngestionPipeline::new(
        initial_stomp,
        config.watched_routes.clone(),
        Arc::clone(&registry),
        sc_tx.clone(),
        prediction_engine,
        Some(db_pool.clone()),
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

    // --- On-time heartbeat recorder task ---
    // Calls record_outcome on every poll-fired event for Active/Critical trains.
    // Darwin only sends TS messages on delay changes — on-time trains go silent,
    // giving zero training examples for "train is running fine". This task fills
    // that gap: on each poll tick it reads the current status and feeds it into
    // the historical store. The 5-min write throttle prevents flooding.
    let mut heartbeat_rx = sc_tx.subscribe();
    let heartbeat_token  = token.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = heartbeat_token.cancelled() => break,
                result = heartbeat_rx.recv() => {
                    let event = match result {
                        Ok(e) => e,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "Heartbeat recorder lagged");
                            continue;
                        }
                        Err(_) => break,
                    };
                    // Only poll-fired events (same state → same state), not real promotions.
                    if event.old_state != event.new_state {
                        continue;
                    }
                    use railpredict::state_machine::train_state::TrainState;
                    if !matches!(event.new_state, TrainState::Active | TrainState::Critical) {
                        continue;
                    }
                    if let Some(entry) = heartbeat_registry.get(&event.train_id) {
                        let status = entry.read().await;
                        heartbeat_engine.record_outcome(&status);
                    }
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
    let poll_task: Option<tokio::task::JoinHandle<()>> = if !config.gbr_configured {
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

        // Monotonic counter for GBR poll versions. Values stay small (1, 2, 3 …) so Darwin
        // firehose versions (timestamp_millis ~ 1.75 × 10¹²) always take priority.
        let poll_version = Arc::new(std::sync::atomic::AtomicU64::new(0));

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
                        let poll_version_inner = Arc::clone(&poll_version);

                        tokio::spawn(async move {
                            match coalescer.get(train_id.clone()).await {
                                Ok(status) => {
                                    cb_clone.record_success().await;
                                    // Assign a monotonic version to this poll result.
                                    // fetch_add is Relaxed — ordering is enforced by the
                                    // registry write lock acquired inside update().
                                    let ver = poll_version_inner
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                                        + 1;
                                    reg_clone
                                        .update(&train_id, |existing| {
                                            use railpredict::types::Stamped;
                                            existing.actual_estimated_departure.apply_if_newer(
                                                Stamped::with_version(
                                                    status.actual_estimated_departure.value,
                                                    ver,
                                                ),
                                            );
                                            existing.reported_delay_mins.apply_if_newer(
                                                Stamped::with_version(
                                                    status.reported_delay_mins.value,
                                                    ver,
                                                ),
                                            );
                                            existing.actual_platform.apply_if_newer(
                                                Stamped::with_version(
                                                    status.actual_platform.value.clone(),
                                                    ver,
                                                ),
                                            );
                                            existing.is_cancelled.apply_if_newer(
                                                Stamped::with_version(
                                                    status.is_cancelled.value,
                                                    ver,
                                                ),
                                            );
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
                                    use railpredict::networking::gbr_client::GbrErrorKind;
                                    match &e {
                                        CoalescerError::Gbr { kind, detail } => {
                                            if kind.is_breaker_failure() {
                                                cb_clone.record_failure().await;
                                                tracing::warn!(?kind, %detail, "GBR poll failure — circuit breaker incremented");
                                            } else if *kind == GbrErrorKind::RateLimited {
                                                tracing::warn!("GBR rate limited — backing off");
                                            } else {
                                                tracing::warn!(?kind, %detail, "GBR poll error");
                                            }
                                        }
                                        CoalescerError::InFlightDropped => {
                                            tracing::debug!(
                                                train_id = %train_id,
                                                "In-flight GBR request dropped"
                                            );
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

    // --- Station autocomplete index (in-memory, loaded once) ---
    let station_index = build_station_index(&db_pool).await;

    // --- axum HTTP server ---
    let app_state = assemble_app_state(
        Arc::clone(&registry),
        sc_tx,
        db_pool.clone(),
        Arc::clone(&prometheus_handle),
        &config,
        Arc::clone(&ingest_tx),
        Arc::clone(&station_index),
    );
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

    let _ = tokio::join!(pipeline_task, eviction_task, flush_task, prune_task, api_task);
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

/// Build the in-memory station autocomplete index from the DB (once, at startup).
async fn build_station_index(db: &db::Db) -> Arc<StationIndex> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT crs, name FROM stations")
        .fetch_all(db)
        .await
        .unwrap_or_default();
    let count = rows.len();
    let idx = StationIndex::build(rows);
    tracing::info!(stations = count, "Station autocomplete index built");
    Arc::new(idx)
}

/// Assemble the shared `AppState` from its parts. One definition so the normal and
/// ingestion-disabled (`wait_for_shutdown`) startup paths cannot drift.
#[allow(clippy::too_many_arguments)]
fn assemble_app_state(
    registry: Arc<TrainRegistry>,
    state_change_tx: broadcast::Sender<railpredict::state_machine::StateChangeEvent>,
    db: db::Db,
    prometheus: Arc<metrics_exporter_prometheus::PrometheusHandle>,
    config: &Config,
    ingest_status: Arc<watch::Sender<IngestStatus>>,
    station_index: Arc<StationIndex>,
) -> AppState {
    AppState {
        registry,
        state_change_tx,
        db,
        prometheus,
        cors_allowed_origins: config.cors_allowed_origins.clone(),
        http_rate_limit_per_sec: config.http_rate_limit_per_sec,
        ingest_status,
        station_index,
    }
}

#[allow(clippy::too_many_arguments)]
async fn wait_for_shutdown(
    token: CancellationToken,
    registry: Arc<TrainRegistry>,
    sc_tx: broadcast::Sender<railpredict::state_machine::StateChangeEvent>,
    config: &Config,
    history_store: Arc<railpredict::prediction::types::HistoricalStore>,
    db_pool: db::Db,
    prometheus_handle: Arc<metrics_exporter_prometheus::PrometheusHandle>,
    ingest_tx: Arc<watch::Sender<IngestStatus>>,
) {
    let station_index = build_station_index(&db_pool).await;
    let app_state = assemble_app_state(
        registry,
        sc_tx,
        db_pool.clone(),
        prometheus_handle,
        config,
        ingest_tx,
        station_index,
    );
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

    if let Err(e) = db::history::flush_history(&db_pool, &history_store).await {
        tracing::error!(error = %e, "Final DB flush failed");
    }

    tracing::info!("All tasks stopped. Goodbye.");
}
