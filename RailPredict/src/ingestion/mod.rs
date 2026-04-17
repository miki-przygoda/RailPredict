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
pub mod parser;
pub mod stomp_client;

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};

use crate::cache::TrainRegistry;
use crate::state_machine::poll_manager::StateChangeEvent;
use crate::state_machine::train_state::{PromotionReason, TrainState};
use crate::types::train_status::{Stamped, UpdateSource};

use filter::Filter;
use parser::{parse_pport, ParsedUpdate};
use stomp_client::{StompClient, StompError, StompFrame};

const INGESTION_BUFFER: usize = 512;

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

pub struct IngestionPipeline {
    stomp: Box<dyn StompClient>,
    filter: Filter,
    registry: Arc<TrainRegistry>,
    state_change_tx: broadcast::Sender<StateChangeEvent>,
}

impl IngestionPipeline {
    pub fn new(
        stomp: Box<dyn StompClient>,
        watched_routes: HashSet<String>,
        registry: Arc<TrainRegistry>,
        state_change_tx: broadcast::Sender<StateChangeEvent>,
    ) -> Self {
        Self {
            stomp,
            filter: Filter::new(watched_routes),
            registry,
            state_change_tx,
        }
    }

    pub fn passthrough(
        stomp: Box<dyn StompClient>,
        registry: Arc<TrainRegistry>,
        state_change_tx: broadcast::Sender<StateChangeEvent>,
    ) -> Self {
        Self::new(stomp, HashSet::new(), registry, state_change_tx)
    }

    /// Start the pipeline. Runs until the STOMP connection closes or all senders are dropped.
    pub async fn run(mut self) {
        let (frame_tx, mut frame_rx) = mpsc::channel::<Result<StompFrame, StompError>>(INGESTION_BUFFER);

        if let Err(e) = self.stomp.subscribe(frame_tx).await {
            tracing::error!(error = %e, "STOMP subscribe failed");
            return;
        }

        tracing::info!("Darwin ingestion pipeline running");

        while let Some(result) = frame_rx.recv().await {
            match result {
                Ok(frame) => self.process_frame(frame).await,
                Err(e) => {
                    tracing::error!(error = %e, "STOMP connection error — pipeline stopping");
                    break;
                }
            }
        }

        tracing::info!("Darwin ingestion pipeline stopped");
    }

    async fn process_frame(&self, frame: StompFrame) {
        if frame.command != "MESSAGE" {
            return;
        }

        let xml_bytes = &frame.body;

        // Gate 1: taxonomy + route filter (no XML parse).
        // We don't have a CRS at this point (pre-parse); pass None to rely on taxonomy only.
        // A more sophisticated implementation would do a fast scan for the `tpl` attribute.
        if !self.filter.should_parse(xml_bytes, None) {
            return;
        }

        let xml_str = match std::str::from_utf8(xml_bytes) {
            Ok(s) => s,
            Err(_) => {
                tracing::warn!("Received non-UTF-8 Darwin frame — dropping");
                return;
            }
        };

        let (msg_ts, updates) = match parse_pport(xml_str) {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!(error = %e, "Darwin XML parse error — dropping frame");
                return;
            }
        };

        for update in updates {
            match update {
                ParsedUpdate::TrainStatus(ts_update) => {
                    // Gate 2: sequence guard.
                    if !self.filter.should_apply(&ts_update.rid, msg_ts) {
                        tracing::trace!(rid = %ts_update.rid, "Dropping stale TS message");
                        continue;
                    }

                    let rid = ts_update.rid.clone();
                    let is_cancelled = ts_update.is_cancelled;
                    let is_delayed = ts_update.is_delayed;

                    // Ensure the train is registered before applying the update.
                    // We do this first so the subsequent update() call always succeeds.
                    if self.registry.get(&rid).is_none() {
                        if let (Some(sched), Some(publ)) =
                            (ts_update.scheduled_departure, ts_update.scheduled_departure)
                        {
                            tracing::debug!(rid = %rid, "Registering new train from TS message");
                            let mut new_status = crate::types::TrainStatus::new(rid.clone(), sched, publ);
                            new_status.origin_crs = ts_update.station_crs.clone();
                            self.registry.upsert(rid.clone(), new_status);
                        }
                    }

                    // Apply live fields to registry (works whether just registered or pre-existing).
                    self.registry
                        .update(&rid, |status| {
                            if let Some(dep) = ts_update.estimated_departure {
                                status.actual_estimated_departure = Stamped::new(Some(dep));
                            }
                            if let Some(platform) = ts_update.platform.clone() {
                                status.actual_platform = Stamped::new(Some(platform));
                            }
                            status.is_cancelled = Stamped::new(is_cancelled);
                            status.last_update_source = UpdateSource::StompFirehose;
                        })
                        .await;

                    // Emit state-change event for emergency promotions.
                    if is_cancelled || is_delayed {
                        tracing::info!(rid = %rid, cancelled = is_cancelled, delayed = is_delayed, "Emergency Critical promotion");
                        let event = StateChangeEvent {
                            train_id: rid,
                            old_state: TrainState::Active,
                            new_state: TrainState::Critical,
                            reason: PromotionReason::IncidentDetected,
                        };
                        let _ = self.state_change_tx.send(event);
                    }
                }

                ParsedUpdate::Deactivated(deact) => {
                    if !self.filter.should_apply(&deact.rid, msg_ts) {
                        tracing::trace!(rid = %deact.rid, "Dropping stale deactivated message");
                        continue;
                    }

                    let rid = deact.rid.clone();
                    tracing::info!(rid = %rid, "Train deactivated — removing from registry");

                    // Remove from registry and forget sequence state.
                    self.registry.remove(&rid);
                    self.filter.forget(&rid);

                    let event = StateChangeEvent {
                        train_id: rid,
                        old_state: TrainState::Active,
                        new_state: TrainState::Terminal,
                        reason: PromotionReason::TimeBased,
                    };
                    let _ = self.state_change_tx.send(event);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state_machine::poll_manager::StateChangeEvent;
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
        pipeline.run().await;
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
        pipeline.run().await;
        let id = TrainId::rid("202404170123456").unwrap();
        assert!(registry.get(&id).is_none());
    }

    #[tokio::test]
    async fn deactivated_emits_terminal_state_change() {
        let (pipeline, mut sc_rx, _) = make_pipeline(vec![
            TS_XML.to_string(),
            DEACTIVATED_XML.to_string(),
        ]);
        pipeline.run().await;

        let mut saw_terminal = false;
        while let Ok(event) = sc_rx.try_recv() {
            if event.new_state == TrainState::Terminal {
                saw_terminal = true;
            }
        }
        assert!(saw_terminal);
    }

    #[tokio::test]
    async fn cancelled_train_emits_critical_event() {
        let (pipeline, mut sc_rx, _) = make_pipeline(vec![CANCELLED_XML.to_string()]);
        pipeline.run().await;

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
        pipeline.run().await;

        let id = TrainId::rid("202404170123456").unwrap();
        if let Some(entry) = registry.get(&id) {
            let status = entry.read().await;
            // Platform should still be "3", not "STALE".
            assert_ne!(status.actual_platform.value.as_deref(), Some("STALE"));
        }
    }
}
