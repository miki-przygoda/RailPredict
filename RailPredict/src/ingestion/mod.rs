//! Darwin Push Port ingestion pipeline.
//!
//! ## Data flow
//! ```text
//! STOMP broker
//!      │  raw XML frames
//!      ▼
//! [ingestion_rx channel]   ← bounded, INGESTION_BUFFER = 512
//!      │
//!      ▼
//! Filter::should_parse()   ← taxonomy check + route filter (no XML parse yet)
//!      │  KEEP / CONDITIONAL
//!      ▼
//! Filter::should_apply()   ← sequence guard (timestamp > last seen for this RID)
//!      │  newer
//!      ▼
//! parser::parse_pport()    ← quick-xml event parse → ParsedUpdate
//!      │
//!      ├── TrainStatus → TrainRegistry::update()
//!      │
//!      └── Deactivated → TrainRegistry::remove()
//!                      + state_change_tx (mpsc) → state machine notification
//! ```
//!
//! ## Emergency promotion hook
//! Disruption signals (cancellations, major delays from `is_cancelled` or `is_delayed`
//! on a TS message) are broadcast on `state_change_tx` — the same channel defined in
//! Epic 2 — so the state machine can force a Critical promotion. This is NOT a direct
//! function call into `state_machine/`; the channel decouples the modules.
//!
//! ## Buffer size rationale
//! `INGESTION_BUFFER = 512`: Darwin at peak delivers ~200–400 messages/second.
//! 512 slots give ~1–2 seconds of backpressure before the STOMP reader blocks.
//! Blocking the reader is preferable to unbounded growth — a slow consumer is a
//! signal to investigate the filter/parser throughput, not to allocate more memory.

pub mod filter;
pub mod gtfs;
pub mod operators;
pub mod parser;
pub mod rds;
pub mod reason;
pub mod stomp_client;

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Datelike, Timelike, Utc};
use dashmap::DashSet;
use tokio::sync::{broadcast, mpsc};

use crate::cache::TrainRegistry;
use crate::db::Db;
use crate::prediction::PredictionEngine;
use crate::state_machine::{StateChangeEvent, TrainState};
use crate::types::train_status::{Stamped, UpdateSource};

use filter::Filter;
use parser::{parse_pport, ParsedUpdate};
use stomp_client::{StompClient, StompError, StompFrame};

fn decompress_if_gzip(data: &[u8]) -> anyhow::Result<std::borrow::Cow<'_, [u8]>> {
    if data.starts_with(&[0x1f, 0x8b]) {
        use std::io::Read;
        let mut decoder = flate2::read::GzDecoder::new(data);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out)?;
        Ok(std::borrow::Cow::Owned(out))
    } else {
        Ok(std::borrow::Cow::Borrowed(data))
    }
}

const INGESTION_BUFFER: usize = 512;

// ---------------------------------------------------------------------------
// Pipeline shared context — cloned cheaply for reconnect attempts
// ---------------------------------------------------------------------------

/// Non-stomp parts of the pipeline, kept behind `Arc` so they can be shared
/// across reconnect attempts without cloning the filter sequence state.
///
/// `Filter` internally uses `DashMap` which handles concurrent access without
/// a Mutex, so we use `Arc<Filter>` directly.
#[derive(Clone)]
pub struct PipelineContext {
    pub filter: Arc<Filter>,
    pub registry: Arc<TrainRegistry>,
    pub state_change_tx: broadcast::Sender<StateChangeEvent>,
    pub prediction_engine: PredictionEngine,
    /// DB pool for the per-RID `prediction_outcomes` ledger. `None` in tests and
    /// in the `passthrough` constructor — when absent, prediction persistence
    /// is silently skipped.
    pub db: Option<Db>,
    /// RIDs we've already written a `prediction_outcomes` row for. Survives
    /// STOMP reconnects (lives on the shared context). On train deactivation
    /// the entry is removed so memory stays bounded.
    pub persisted_predictions: Arc<DashSet<String>>,
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

pub struct IngestionPipeline {
    stomp: Box<dyn StompClient>,
    ctx: PipelineContext,
}

impl IngestionPipeline {
    pub fn new(
        stomp: Box<dyn StompClient>,
        watched_routes: HashSet<String>,
        registry: Arc<TrainRegistry>,
        state_change_tx: broadcast::Sender<StateChangeEvent>,
        prediction_engine: PredictionEngine,
        db: Option<Db>,
    ) -> Self {
        let ctx = PipelineContext {
            filter: Arc::new(Filter::new(watched_routes)),
            registry,
            state_change_tx,
            prediction_engine,
            db,
            persisted_predictions: Arc::new(DashSet::new()),
        };
        Self { stomp, ctx }
    }

    pub fn passthrough(
        stomp: Box<dyn StompClient>,
        registry: Arc<TrainRegistry>,
        state_change_tx: broadcast::Sender<StateChangeEvent>,
    ) -> Self {
        Self::new(stomp, HashSet::new(), registry, state_change_tx, PredictionEngine::new(), None)
    }

    /// Clone the shared pipeline context (filter state, registry, broadcast channel).
    /// The caller uses this to rebuild the pipeline with a fresh STOMP client after
    /// a reconnect — all sequence guard state and registry entries are preserved.
    pub fn context(&self) -> PipelineContext {
        self.ctx.clone()
    }

    /// Build a pipeline from a pre-existing context and a new STOMP client.
    /// Used by the reconnect retry loop in `main.rs`.
    pub fn from_context(ctx: PipelineContext, stomp: Box<dyn StompClient>) -> Self {
        Self { stomp, ctx }
    }

    /// Start the pipeline. Runs until the STOMP connection closes or all senders are dropped.
    ///
    /// Returns `Ok(())` on a clean shutdown (channel closed without error).
    /// Returns `Err(e)` if the STOMP connection fails — the caller can retry.
    pub async fn run(mut self) -> anyhow::Result<()> {
        let (frame_tx, mut frame_rx) = mpsc::channel::<Result<StompFrame, StompError>>(INGESTION_BUFFER);

        self.stomp.subscribe(frame_tx).await.map_err(|e| anyhow::anyhow!("{e}"))?;

        tracing::info!("Darwin ingestion pipeline running");

        while let Some(result) = frame_rx.recv().await {
            match result {
                Ok(frame) => self.process_frame(frame).await,
                Err(e) => {
                    tracing::error!(error = %e, "STOMP connection error — pipeline stopping");
                    return Err(anyhow::anyhow!("{e}"));
                }
            }
        }

        tracing::info!("Darwin ingestion pipeline stopped");
        Ok(())
    }

    async fn process_frame(&self, frame: StompFrame) {
        if frame.command != "MESSAGE" {
            return;
        }

        // Phase 2: count every Darwin message received from the broker.
        metrics::counter!("darwin_messages_received_total").increment(1);

        // Decompress gzip body if present (Darwin Push Port sends gzip-compressed XML).
        let decompressed = match decompress_if_gzip(&frame.body) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "Gzip decompression failed — dropping frame");
                metrics::counter!("darwin_messages_dropped_total", "reason" => "decompress_error").increment(1);
                return;
            }
        };
        let xml_bytes: &[u8] = &decompressed;

        // Gate 1: taxonomy + route filter (no XML parse).
        // We don't have a CRS at this point (pre-parse); pass None to rely on taxonomy only.
        // A more sophisticated implementation would do a fast scan for the `tpl` attribute.
        if !self.ctx.filter.should_parse(xml_bytes, None) {
            // Phase 2: count messages dropped at the taxonomy/route filter stage.
            metrics::counter!("darwin_messages_dropped_total", "reason" => "taxonomy").increment(1);
            return;
        }

        let xml_str = match std::str::from_utf8(xml_bytes) {
            Ok(s) => s,
            Err(_) => {
                tracing::warn!("Received non-UTF-8 Darwin frame — dropping");
                // Phase 2: count messages dropped due to encoding errors.
                metrics::counter!("darwin_messages_dropped_total", "reason" => "encoding").increment(1);
                return;
            }
        };

        let (msg_ts, updates) = match parse_pport(xml_str) {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!(error = %e, "Darwin XML parse error — dropping frame");
                // Phase 2: count messages dropped due to XML parse errors.
                metrics::counter!("darwin_messages_dropped_total", "reason" => "parse_error").increment(1);
                return;
            }
        };

        // Phase 4: attach trace_id to the current span for log correlation.
        // Allows correlating a Grafana spike in darwin_messages_dropped_total back to
        // a specific frame in the structured log stream.
        let span = tracing::Span::current();
        let trace_id = format!("{:?}", span.id());
        span.record("trace_id", trace_id.as_str());

        for update in updates {
            match update {
                ParsedUpdate::TrainStatus(mut ts_update) => {
                    // Gate 2: sequence guard.
                    if !self.ctx.filter.should_apply(&ts_update.rid, msg_ts) {
                        tracing::trace!(rid = %ts_update.rid, "Dropping stale TS message");
                        // Phase 2: count messages dropped by the sequence guard.
                        metrics::counter!("darwin_messages_dropped_total", "reason" => "stale").increment(1);
                        continue;
                    }

                    // Source version: Darwin message timestamp in milliseconds (~1.75 × 10¹²).
                    // Always larger than GBR poll counters (1, 2, 3…), so Darwin wins on conflict.
                    let darwin_version = msg_ts.timestamp_millis() as u64;

                    let rid = ts_update.rid.clone();
                    let is_cancelled = ts_update.is_cancelled;
                    let is_delayed = ts_update.is_delayed;

                    // Ensure the train is registered before applying the update.
                    // We do this first so the subsequent update() call always succeeds.
                    if self.ctx.registry.get(&rid).is_none()
                        && let (Some(sched), Some(publ)) =
                            (ts_update.scheduled_departure, ts_update.scheduled_departure)
                    {
                        tracing::debug!(rid = %rid, "Registering new train from TS message");
                        let mut new_status = crate::types::TrainStatus::new(rid.clone(), sched, publ);
                        new_status.origin_crs = ts_update.station_crs.clone();
                        new_status.destination_crs = ts_update.destination_crs.clone();
                        new_status.uid = ts_update.uid.clone();
                        self.ctx.registry.upsert(rid.clone(), new_status);
                    }

                    // Capture values needed inside the closure before borrowing self.
                    let working_dep = ts_update.working_departure;
                    let platform = ts_update.platform.clone();
                    let ts_uid = ts_update.uid.clone();
                    let ts_destination_crs = ts_update.destination_crs.clone();
                    // Full-Journey Capture: move the per-call list + reasons out of the parsed
                    // update so they can be folded into the accumulated journey inside the lock.
                    let ts_calls = std::mem::take(&mut ts_update.calls);
                    let ts_late_reason = ts_update.late_reason.take();
                    let ts_cancel_reason = ts_update.cancel_reason.take();
                    let engine = self.ctx.prediction_engine.clone();
                    let db_for_persist = self.ctx.db.clone();
                    let persisted_set = Arc::clone(&self.ctx.persisted_predictions);

                    // Look up predecessor delay BEFORE the registry.update() closure to avoid
                    // holding nested locks (update holds a write lock; delay_for_rid needs a read
                    // lock on a different entry — still a potential deadlock via DashMap shards).
                    let predecessor_delay = self.ctx.registry
                        .predecessor_rid(rid.as_str())
                        .and_then(|prev| self.ctx.registry.delay_for_rid(&prev));

                    // Apply live fields to registry (works whether just registered or pre-existing).
                    self.ctx.registry
                        .update(&rid, |status| {
                            if let Some(p) = platform {
                                status.actual_platform.apply_if_newer(
                                    Stamped::with_version(Some(p), darwin_version),
                                );
                            }
                            // Populate uid on first sighting.
                            if status.uid.is_none() {
                                status.uid = ts_uid;
                            }
                            // Populate working_departure on first sighting.
                            if status.working_departure.is_none() {
                                status.working_departure = working_dep;
                            }
                            // Update destination_crs whenever the parser emits one —
                            // the last Location in the TS message is always the destination.
                            if ts_destination_crs.is_some() {
                                status.destination_crs = ts_destination_crs;
                            }
                            status.is_cancelled.apply_if_newer(
                                Stamped::with_version(Some(is_cancelled), darwin_version),
                            );
                            status.last_update_source = UpdateSource::StompFirehose;

                            // Full-Journey Capture: fold every parsed call into the accumulated
                            // journey and record the latest reason codes (additive — the scalar
                            // fields the live predictor uses are untouched above).
                            for c in &ts_calls {
                                status.apply_call(call_obs_from(c));
                            }
                            if let Some(r) = &ts_late_reason {
                                status.late_reason_code = Some(r.code);
                                if r.tiploc.is_some() {
                                    status.reason_tiploc = r.tiploc.clone();
                                }
                            }
                            if let Some(r) = &ts_cancel_reason {
                                status.cancel_reason_code = Some(r.code);
                                if r.tiploc.is_some() {
                                    status.reason_tiploc = r.tiploc.clone();
                                }
                            }

                            // Origin departure + delay from the accumulated origin call, so the
                            // scheduled and estimated/actual times come from the SAME stop. The old
                            // scalar paired the origin's schedule with a later stop's time (Darwin
                            // TS messages are partial), measuring journey progress, not delay.
                            let origin_dep = status
                                .journey
                                .get(&0)
                                .and_then(|o| o.act_dep.or(o.est_dep).map(|obs| (obs, o.sched_dep)));
                            if let Some((obs, sched)) = origin_dep {
                                status.actual_estimated_departure.apply_if_newer(
                                    Stamped::with_version(Some(obs), darwin_version),
                                );
                                if let Some(sched) = sched {
                                    status.reported_delay_mins.apply_if_newer(
                                        Stamped::with_version(
                                            Some(wrapped_delay_mins(sched, obs)),
                                            darwin_version,
                                        ),
                                    );
                                }
                            }

                            // Propagate fleet turnround (NP association) delay into volatility
                            // context so prediction/engine.rs can use it as a feature.
                            if predecessor_delay.is_some() {
                                status.volatility.predecessor_train_delay_mins = predecessor_delay;
                            }

                            // Feed confirmed delay into historical store, then refresh prediction.
                            engine.record_outcome(status);
                            engine.predict_and_update(status);

                            // Persist the first prediction we ever produce for this RID into
                            // the prediction_outcomes ledger. The DashSet ensures we attempt
                            // the insert exactly once per RID; the SQL still has ON CONFLICT
                            // DO NOTHING as a belt-and-braces guard against process restarts.
                            if status.predicted_delay_mins.value.is_some()
                                && let Some(db) = db_for_persist.as_ref()
                                && persisted_set.insert(status.id.as_str().to_string())
                            {
                                let db_clone = db.clone();
                                let status_snapshot = status.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = crate::db::predictions::insert_first_prediction(
                                        &db_clone, &status_snapshot,
                                    ).await {
                                        tracing::warn!(
                                            error = %e,
                                            rid = %status_snapshot.id,
                                            "Failed to persist first prediction"
                                        );
                                    }
                                    // Snapshot every initial ML prediction for replay training.
                                    if let (Some(uid), Some(pred)) = (
                                        status_snapshot.uid.as_deref(),
                                        status_snapshot.predicted_delay_mins.value,
                                    ) {
                                        let features = status_snapshot.volatility.prediction_features.as_ref();
                                        if let Err(e) = crate::db::predictions::insert_snapshot(
                                            &db_clone,
                                            status_snapshot.id.as_str(),
                                            uid,
                                            pred,
                                            features,
                                        ).await {
                                            tracing::warn!(
                                                error = %e,
                                                rid = %status_snapshot.id,
                                                "Failed to insert prediction snapshot"
                                            );
                                        }
                                    }
                                });
                            }
                        })
                        .await;

                    // Phase 2: count messages successfully applied to the registry.
                    metrics::counter!("darwin_messages_applied_total").increment(1);

                    // TODO: uncomment when tier routing wired (Improvements 2.1-2.3)
                    // metrics::counter!("request_served_tier_total", "tier" => "C").increment(1);

                    // Emit state-change event for emergency promotions.
                    if is_cancelled || is_delayed {
                        tracing::info!(rid = %rid, cancelled = is_cancelled, delayed = is_delayed, "Emergency Critical promotion");
                        let event = StateChangeEvent {
                            train_id: rid,
                            old_state: TrainState::Active,
                            new_state: TrainState::Critical,
                        };
                        let _ = self.ctx.state_change_tx.send(event);
                    }
                }

                ParsedUpdate::Schedule(sched) => {
                    // Not gated by the sequence guard: schedule sets toc + the planned plan and
                    // is idempotent (apply_call merges, toc is set-once), so a reconnect replay
                    // must not consume the guard slot a TS message needs.
                    let rid = sched.rid.clone();
                    // Register the train from its plan if unseen, so toc + the planned calling
                    // pattern are captured even before the first TS message arrives.
                    if self.ctx.registry.get(&rid).is_none()
                        && let Some(first_dep) = sched.calls.iter().find_map(|c| c.sched_dep)
                    {
                        let mut new_status =
                            crate::types::TrainStatus::new(rid.clone(), first_dep, first_dep);
                        new_status.uid = sched.uid.clone();
                        new_status.origin_crs = sched.calls.first().map(|c| c.tpl.clone());
                        new_status.destination_crs = sched.calls.last().map(|c| c.tpl.clone());
                        self.ctx.registry.upsert(rid.clone(), new_status);
                    }
                    self.ctx
                        .registry
                        .update(&rid, move |status| {
                            if status.toc.is_none() {
                                status.toc = sched.toc.clone();
                            }
                            if status.train_category.is_none() {
                                status.train_category = sched.train_category.clone();
                            }
                            for c in &sched.calls {
                                status.apply_call(call_obs_from(c));
                            }
                            if let Some(r) = &sched.late_reason {
                                status.late_reason_code = Some(r.code);
                                if r.tiploc.is_some() {
                                    status.reason_tiploc = r.tiploc.clone();
                                }
                            }
                            if let Some(r) = &sched.cancel_reason {
                                status.cancel_reason_code = Some(r.code);
                                if r.tiploc.is_some() {
                                    status.reason_tiploc = r.tiploc.clone();
                                }
                            }
                        })
                        .await;
                }

                ParsedUpdate::Association { prev_rid, next_rid } => {
                    self.ctx.registry.record_association(&prev_rid, &next_rid);
                    tracing::debug!(
                        prev_rid = %prev_rid,
                        next_rid = %next_rid,
                        "Recorded NP turnround association"
                    );
                }

                ParsedUpdate::Deactivated(deact) => {
                    if !self.ctx.filter.should_apply(&deact.rid, msg_ts) {
                        tracing::trace!(rid = %deact.rid, "Dropping stale deactivated message");
                        continue;
                    }

                    let rid = deact.rid.clone();
                    tracing::info!(rid = %rid, "Train deactivated — removing from registry");

                    // Capture the final delay (to close out the prediction_outcomes row)
                    // and, if the service was cancelled, the cancellation pattern — both
                    // before removing the entry.
                    let (final_delay, cancellation, journey) = if let Some(arc) =
                        self.ctx.registry.get(&rid)
                    {
                        let s = arc.read().await;
                        let cancellation = if s.is_cancelled.value == Some(true) {
                            match (s.uid.clone(), s.origin_crs.clone()) {
                                (Some(uid), Some(origin)) => {
                                    Some((uid, origin, s.scheduled_departure.value))
                                }
                                _ => None,
                            }
                        } else {
                            None
                        };
                        // Full-Journey Capture: snapshot the accumulated journey before eviction.
                        let journey = build_journey_record(&s);
                        (s.reported_delay_mins.value, cancellation, journey)
                    } else {
                        (None, None, None)
                    };

                    // Remove from registry and forget sequence state.
                    self.ctx.registry.remove(&rid);
                    self.ctx.filter.forget(&rid);
                    self.ctx.persisted_predictions.remove(rid.as_str());

                    // Finalise the prediction_outcomes row (best-effort, fire-and-forget).
                    if let (Some(db), Some(final_d)) = (self.ctx.db.as_ref(), final_delay) {
                        let db_clone = db.clone();
                        let rid_str = rid.as_str().to_string();
                        tokio::spawn(async move {
                            if let Err(e) = crate::db::predictions::finalise_outcome(
                                &db_clone, &rid_str, final_d,
                            ).await {
                                tracing::warn!(
                                    error = %e,
                                    rid = %rid_str,
                                    "Failed to finalise prediction outcome"
                                );
                            }
                        });
                    }

                    // Persist the cancellation (best-effort) if this service was cancelled.
                    if let (Some((uid, origin, sched)), Some(db)) =
                        (cancellation, self.ctx.db.as_ref())
                    {
                        let db_clone = db.clone();
                        let rid_str = rid.as_str().to_string();
                        tokio::spawn(async move {
                            if let Err(e) = crate::db::cancellations::record_cancellation(
                                &db_clone, &uid, &origin, sched,
                            )
                            .await
                            {
                                tracing::warn!(error = %e, rid = %rid_str, "Failed to record cancellation");
                            }
                        });
                    }

                    // Persist the full journey (best-effort) — the analytics "fat record".
                    if let (Some((record, calls)), Some(db)) = (journey, self.ctx.db.as_ref()) {
                        let db_clone = db.clone();
                        let rid_str = rid.as_str().to_string();
                        tokio::spawn(async move {
                            if let Err(e) =
                                crate::db::journeys::insert_journey(&db_clone, &record, &calls).await
                            {
                                tracing::warn!(error = %e, rid = %rid_str, "Failed to persist journey");
                            }
                        });
                    }

                    let event = StateChangeEvent {
                        train_id: rid,
                        old_state: TrainState::Active,
                        new_state: TrainState::Terminal,
                    };
                    let _ = self.ctx.state_change_tx.send(event);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Full-Journey Capture helpers
// ---------------------------------------------------------------------------

/// Convert a parsed Darwin call into the registry's accumulated observation shape.
fn call_obs_from(c: &parser::CallUpdate) -> crate::types::CallObservation {
    crate::types::CallObservation {
        tpl: c.tpl.clone(),
        seq: c.seq,
        sched_arr: c.sched_arr,
        sched_dep: c.sched_dep,
        est_arr: c.est_arr,
        act_arr: c.act_arr,
        est_dep: c.est_dep,
        act_dep: c.act_dep,
        platform: c.platform.clone(),
        plat_confirmed: c.plat_confirmed,
        is_cancelled: c.is_cancelled,
        activity: c.activity.clone(),
    }
}

/// Minutes between a scheduled and an observed (estimated/actual) time, correcting for midnight
/// rollover. Darwin times are `HH:MM` anchored on the schedule-start date, so a stop just after
/// midnight (00:05) against a 23:55 schedule naively reads −1430 min. A delay/early beyond ±12h
/// is always such an artifact, so wrap it back into range.
fn wrapped_delay_mins(scheduled: DateTime<Utc>, observed: DateTime<Utc>) -> i32 {
    let mut d = (observed - scheduled).num_minutes();
    if d < -720 {
        d += 1440;
    } else if d > 720 {
        d -= 1440;
    }
    d as i32
}

/// Delay in whole minutes between a scheduled and an actual/estimated time, if both are known.
fn delay_between(sched: Option<DateTime<Utc>>, actual: Option<DateTime<Utc>>) -> Option<i32> {
    match (sched, actual) {
        (Some(s), Some(a)) => Some(wrapped_delay_mins(s, a)),
        _ => None,
    }
}

/// Assemble a finalised journey (header + per-stop rows) from an accumulated `TrainStatus`.
/// Returns `None` if the minimum identity (uid + origin TIPLOC) is missing. Computes the cheap
/// rollups (arrival delay, max/min/recovered, n_calls) the dashboards and regression use.
fn build_journey_record(
    status: &crate::types::TrainStatus,
) -> Option<(
    crate::db::journeys::JourneyRecord,
    Vec<crate::db::journeys::JourneyCallRecord>,
)> {
    use crate::db::journeys::{JourneyCallRecord, JourneyRecord};

    let uid = status.uid.clone()?;
    let origin_tpl = status.origin_crs.clone()?;
    let sched_dep = status.scheduled_departure.value;
    let weekday = sched_dep.weekday().num_days_from_monday() as i16;
    let departure_hour = sched_dep.hour() as i16;

    let mut calls: Vec<JourneyCallRecord> = Vec::with_capacity(status.journey.len());
    let mut max_delay: Option<i32> = None;
    let mut min_delay: Option<i32> = None;
    let mut partial_cancel = false;

    // BTreeMap iterates in seq order: calls[0] = origin, calls.last() = destination.
    for obs in status.journey.values() {
        let arr_delay = delay_between(obs.sched_arr, obs.act_arr.or(obs.est_arr));
        let dep_delay = delay_between(obs.sched_dep, obs.act_dep.or(obs.est_dep));
        let dwell_secs = match (obs.act_arr, obs.act_dep) {
            (Some(a), Some(d)) => Some((d - a).num_seconds() as i32),
            _ => None,
        };
        if let Some(d) = dep_delay.or(arr_delay) {
            max_delay = Some(max_delay.map_or(d, |m| m.max(d)));
            min_delay = Some(min_delay.map_or(d, |m| m.min(d)));
        }
        if obs.is_cancelled {
            partial_cancel = true;
        }
        calls.push(JourneyCallRecord {
            seq: obs.seq as i16,
            tpl: obs.tpl.clone(),
            sched_arr: obs.sched_arr,
            actual_arr: obs.act_arr,
            arr_delay_mins: arr_delay,
            sched_dep: obs.sched_dep,
            actual_dep: obs.act_dep,
            dep_delay_mins: dep_delay,
            platform: obs.platform.clone(),
            plat_confirmed: obs.plat_confirmed,
            is_cancelled: obs.is_cancelled,
            dwell_secs,
        });
    }

    let was_cancelled = status.is_cancelled.value == Some(true);
    if was_cancelled {
        // A whole-service cancel supersedes any per-stop cancel flags.
        partial_cancel = false;
    }

    // Identify origin/destination by TIPLOC rather than position: when a TS message registers
    // a train before its schedule arrives, the first-seen stop (seq 0) may be a mid-journey
    // location, not the origin. Fall back to positional if the name isn't in the journey.
    let origin_obs = status
        .journey
        .values()
        .find(|c| c.tpl == origin_tpl)
        .or_else(|| status.journey.get(&0));
    let origin_platform = origin_obs
        .and_then(|c| c.platform.clone())
        .or_else(|| status.actual_platform.value.clone());
    let platform_confirmed = origin_obs.and_then(|c| c.plat_confirmed);
    let actual_departure = origin_obs.and_then(|c| c.act_dep);

    let arrival_delay_mins = status
        .destination_crs
        .as_ref()
        .and_then(|dest| calls.iter().find(|c| &c.tpl == dest))
        .or_else(|| calls.last())
        .and_then(|c| c.arr_delay_mins.or(c.dep_delay_mins));
    let recovered_mins = match (max_delay, arrival_delay_mins) {
        (Some(mx), Some(ad)) => Some(mx - ad),
        _ => None,
    };

    let reason_code = status.late_reason_code.or(status.cancel_reason_code);
    let reason_class = reason::reason_class_code(reason_code);

    let record = JourneyRecord {
        rid: status.id.as_str().to_string(),
        uid,
        ssd: sched_dep.date_naive(),
        weekday,
        departure_hour,
        toc: status.toc.clone(),
        train_category: status.train_category.clone(),
        origin_tpl,
        destination_tpl: status.destination_crs.clone(),
        scheduled_departure: sched_dep,
        actual_departure,
        origin_delay_mins: status.reported_delay_mins.value,
        arrival_delay_mins,
        late_reason_code: status.late_reason_code.map(|c| c as i16),
        cancel_reason_code: status.cancel_reason_code.map(|c| c as i16),
        reason_tiploc: status.reason_tiploc.clone(),
        reason_class,
        was_cancelled,
        partial_cancel,
        n_calls: status.journey.len() as i16,
        max_delay_mins: max_delay,
        min_delay_mins: min_delay,
        recovered_mins,
        origin_platform,
        platform_confirmed,
        wind_mph: status.volatility.wind_speed_mph,
    };
    Some((record, calls))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_machine::StateChangeEvent;
    use crate::types::TrainId;
    use stomp_client::MockStompClient;

    fn make_pipeline(
        payloads: Vec<String>,
    ) -> (IngestionPipeline, broadcast::Receiver<StateChangeEvent>, Arc<TrainRegistry>) {
        let registry = Arc::new(TrainRegistry::new());
        let (sc_tx, sc_rx) = broadcast::channel(64);
        let stomp = Box::new(MockStompClient::with_xml_payloads(payloads));
        let pipeline = IngestionPipeline::passthrough(stomp, Arc::clone(&registry), sc_tx);
        (pipeline, sc_rx, registry)
    }

    const TS_XML: &str = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:00:00Z" version="16.0">
  <uR updateOrigin="Darwin">
    <TS rid="202404170123456" ssd="2024-04-17" uid="C12345">
      <Location tpl="LEEDS" ptd="12:00" plat="3">
        <dep et="12:05" delayed="false"/>
      </Location>
    </TS>
  </uR>
</Pport>"#;

    const DEACTIVATED_XML: &str = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T13:00:00Z" version="16.0">
  <uR>
    <deactivated rid="202404170123456"/>
  </uR>
</Pport>"#;

    const CANCELLED_XML: &str = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:01:00Z" version="16.0">
  <uR>
    <TS rid="202404170123456" ssd="2024-04-17" uid="C12345" can="true">
      <Location tpl="LEEDS" ptd="12:00"/>
    </TS>
  </uR>
</Pport>"#;

    #[tokio::test]
    async fn ts_message_registers_train() {
        let (pipeline, _, registry) = make_pipeline(vec![TS_XML.to_string()]);
        let _ = pipeline.run().await;
        let id = TrainId::rid("202404170123456").unwrap();
        assert!(registry.get(&id).is_some());
    }

    #[tokio::test]
    async fn deactivated_removes_train_from_registry() {
        // Register first, then deactivate.
        let (pipeline, _, registry) = make_pipeline(vec![
            TS_XML.to_string(),
            DEACTIVATED_XML.to_string(),
        ]);
        let _ = pipeline.run().await;
        let id = TrainId::rid("202404170123456").unwrap();
        assert!(registry.get(&id).is_none());
    }

    #[tokio::test]
    async fn deactivated_emits_terminal_state_change() {
        let (pipeline, mut sc_rx, _) = make_pipeline(vec![
            TS_XML.to_string(),
            DEACTIVATED_XML.to_string(),
        ]);
        let _ = pipeline.run().await;

        let mut saw_terminal = false;
        while let Ok(event) = sc_rx.try_recv() {
            if event.new_state == TrainState::Terminal {
                saw_terminal = true;
            }
        }
        assert!(saw_terminal);
    }

    #[test]
    fn build_journey_record_computes_rollups() {
        use crate::types::{CallObservation, Stamped, TrainId, TrainStatus};
        let base = Utc::now();
        let mut s = TrainStatus::new(TrainId::rid("202404170123456").unwrap(), base, base);
        s.uid = Some("C12345".into());
        s.origin_crs = Some("LEEDS".into());
        s.destination_crs = Some("SHEFFLD".into());
        s.reported_delay_mins = Stamped::new(Some(2));

        // Origin departs +2.
        let mut c0 = CallObservation::new("LEEDS".into(), 0);
        c0.sched_dep = Some(base);
        c0.act_dep = Some(base + chrono::Duration::minutes(2));
        s.apply_call(c0);
        // Peaks at +12 mid-journey.
        let mut c1 = CallObservation::new("WAKEFLD".into(), 0);
        c1.sched_arr = Some(base + chrono::Duration::minutes(18));
        c1.act_arr = Some(base + chrono::Duration::minutes(30));
        s.apply_call(c1);
        // Recovers to +6 by the destination.
        let mut c2 = CallObservation::new("SHEFFLD".into(), 0);
        c2.sched_arr = Some(base + chrono::Duration::minutes(50));
        c2.act_arr = Some(base + chrono::Duration::minutes(56));
        s.apply_call(c2);

        let (rec, calls) = build_journey_record(&s).expect("record built");
        assert_eq!(calls.len(), 3);
        assert_eq!(rec.n_calls, 3);
        assert_eq!(rec.origin_tpl, "LEEDS");
        assert_eq!(rec.destination_tpl.as_deref(), Some("SHEFFLD"));
        assert_eq!(rec.arrival_delay_mins, Some(6));
        assert_eq!(rec.max_delay_mins, Some(12));
        assert_eq!(rec.recovered_mins, Some(6)); // 12 peak − 6 at arrival
        assert!(!rec.was_cancelled);
    }

    #[test]
    fn wrapped_delay_corrects_midnight_rollover() {
        let base = Utc::now();
        assert_eq!(wrapped_delay_mins(base, base + chrono::Duration::minutes(8)), 8);
        // Observed reads 1430 min *before* scheduled = a just-after-midnight rollover → +10.
        assert_eq!(wrapped_delay_mins(base, base - chrono::Duration::minutes(1430)), 10);
        // The symmetric case (scheduled just after midnight) wraps the other way.
        assert_eq!(wrapped_delay_mins(base, base + chrono::Duration::minutes(1435)), -5);
    }

    #[test]
    fn build_journey_record_finds_origin_by_tiploc_not_seq() {
        use crate::types::{CallObservation, TrainId, TrainStatus};
        let base = Utc::now();
        let mut s = TrainStatus::new(TrainId::rid("202404170123456").unwrap(), base, base);
        s.uid = Some("C12345".into());
        s.origin_crs = Some("LEEDS".into());
        s.destination_crs = Some("SHEFFLD".into());
        // A TS arrived before the schedule, leading with a MID stop → seq 0 = WAKEFLD.
        let mut mid = CallObservation::new("WAKEFLD".into(), 0);
        mid.act_arr = Some(base + chrono::Duration::minutes(30));
        s.apply_call(mid);
        let mut origin = CallObservation::new("LEEDS".into(), 0);
        origin.act_dep = Some(base + chrono::Duration::minutes(2));
        origin.platform = Some("3".into());
        s.apply_call(origin);
        let mut dest = CallObservation::new("SHEFFLD".into(), 0);
        dest.sched_arr = Some(base + chrono::Duration::minutes(50));
        dest.act_arr = Some(base + chrono::Duration::minutes(56));
        s.apply_call(dest);

        let (rec, _) = build_journey_record(&s).expect("record built");
        // Origin platform + actual departure must come from LEEDS (seq 1), not WAKEFLD (seq 0).
        assert_eq!(rec.origin_platform.as_deref(), Some("3"));
        assert!(rec.actual_departure.is_some());
        assert_eq!(rec.arrival_delay_mins, Some(6)); // by name → SHEFFLD
    }

    #[test]
    fn build_journey_record_requires_identity() {
        use crate::types::{TrainId, TrainStatus};
        let base = Utc::now();
        // uid + origin_crs never set → not enough to build a journey row.
        let s = TrainStatus::new(TrainId::rid("202404170123456").unwrap(), base, base);
        assert!(build_journey_record(&s).is_none());
    }

    #[tokio::test]
    async fn cancelled_train_emits_critical_event() {
        let (pipeline, mut sc_rx, _) = make_pipeline(vec![CANCELLED_XML.to_string()]);
        let _ = pipeline.run().await;

        let mut saw_critical = false;
        while let Ok(event) = sc_rx.try_recv() {
            if event.new_state == TrainState::Critical {
                saw_critical = true;
            }
        }
        assert!(saw_critical);
    }

    #[tokio::test]
    async fn stale_message_not_applied() {
        // Send TS at T=12:00, then another at T=11:59 (older).
        let stale_xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T11:59:00Z" version="16.0">
  <uR>
    <TS rid="202404170123456" ssd="2024-04-17" uid="C12345">
      <Location tpl="LEEDS" ptd="12:00" plat="STALE">
        <dep et="12:10"/>
      </Location>
    </TS>
  </uR>
</Pport>"#;

        let (pipeline, _, registry) = make_pipeline(vec![
            TS_XML.to_string(),   // T=12:00 — platform "3"
            stale_xml.to_string(), // T=11:59 — platform "STALE" — should be dropped
        ]);
        let _ = pipeline.run().await;

        let id = TrainId::rid("202404170123456").unwrap();
        if let Some(entry) = registry.get(&id) {
            let status = entry.read().await;
            // Platform should still be "3", not "STALE".
            assert_ne!(status.actual_platform.value.as_deref(), Some("STALE"));
        }
    }
}
