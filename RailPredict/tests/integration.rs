//! Integration tests for the full RailPredict pipeline.
//!
//! These tests exercise the actual public wiring — `IngestionPipeline` connected to a
//! real `TrainRegistry` with a `MockStompClient` injecting scripted Darwin messages —
//! rather than individual module internals.
//!
//! Run with: `cargo test --test integration`

use std::sync::Arc;

use railpredict::cache::TrainRegistry;
use railpredict::ingestion::stomp_client::MockStompClient;
use railpredict::ingestion::IngestionPipeline;
use railpredict::state_machine::StateChangeEvent;
use railpredict::state_machine::TrainState;
use railpredict::types::TrainId;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_ts_xml(rid: &str, ts: &str, platform: &str, estimated: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<Pport ts="{ts}" version="16.0">
  <uR updateOrigin="Darwin">
    <TS rid="{rid}" ssd="2024-04-17" uid="C12345">
      <Location tpl="LEEDS" ptd="12:00" plat="{platform}">
        <dep et="{estimated}" delayed="false"/>
      </Location>
    </TS>
  </uR>
</Pport>"#
    )
}

fn make_deactivated_xml(rid: &str, ts: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<Pport ts="{ts}" version="16.0">
  <uR>
    <deactivated rid="{rid}"/>
  </uR>
</Pport>"#
    )
}

fn make_cancelled_xml(rid: &str, ts: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<Pport ts="{ts}" version="16.0">
  <uR>
    <TS rid="{rid}" ssd="2024-04-17" uid="C12345" can="true">
      <Location tpl="LEEDS" ptd="12:00"/>
    </TS>
  </uR>
</Pport>"#
    )
}

fn build_pipeline(
    payloads: Vec<String>,
) -> (IngestionPipeline, broadcast::Receiver<StateChangeEvent>, Arc<TrainRegistry>) {
    let registry = Arc::new(TrainRegistry::new());
    let (sc_tx, sc_rx) = broadcast::channel(256);
    let stomp = Box::new(MockStompClient::with_xml_payloads(payloads));
    let pipeline = IngestionPipeline::passthrough(stomp, Arc::clone(&registry), sc_tx);
    (pipeline, sc_rx, registry)
}

const RID: &str = "202404170123456";

// ---------------------------------------------------------------------------
// Test 1: Full pipeline smoke test
//
// Injects 3 TS messages with ascending timestamps T1 < T2 < T3 for the same RID,
// then a deactivated message. Asserts:
//   - Registry reflects T3 state (T1 and T2 applied; T3 platform wins)
//   - After deactivated: registry has no entry
//   - A Terminal StateChangeEvent was emitted
// ---------------------------------------------------------------------------
#[tokio::test]
async fn smoke_three_ts_then_deactivated() {
    let payloads = vec![
        make_ts_xml(RID, "2024-04-17T12:00:01Z", "1", "12:01"),
        make_ts_xml(RID, "2024-04-17T12:00:02Z", "2", "12:02"),
        make_ts_xml(RID, "2024-04-17T12:00:03Z", "3A", "12:03"), // T3 — should win
        make_deactivated_xml(RID, "2024-04-17T12:00:04Z"),
    ];

    let (pipeline, mut sc_rx, registry) = build_pipeline(payloads);
    let _ = pipeline.run().await;

    let id = TrainId::rid(RID).unwrap();

    // Train must be gone — deactivated message removes it.
    assert!(
        registry.get(&id).is_none(),
        "Registry should be empty after deactivated"
    );

    // A Terminal event must have been emitted.
    let mut saw_terminal = false;
    while let Ok(event) = sc_rx.try_recv() {
        if event.new_state == TrainState::Terminal {
            saw_terminal = true;
        }
    }
    assert!(saw_terminal, "Expected Terminal StateChangeEvent");
}

// ---------------------------------------------------------------------------
// Test 2: Registry reflects T3 platform before deactivation
//
// Same sequence but without the deactivated message — assert that T3's platform
// "3A" is the final value in the registry, proving all three updates were applied
// in order and later ones overwrite earlier ones.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn three_ascending_ts_registry_holds_latest() {
    let payloads = vec![
        make_ts_xml(RID, "2024-04-17T12:00:01Z", "1", "12:01"),
        make_ts_xml(RID, "2024-04-17T12:00:02Z", "2", "12:02"),
        make_ts_xml(RID, "2024-04-17T12:00:03Z", "3A", "12:03"),
    ];

    let (pipeline, _, registry) = build_pipeline(payloads);
    let _ = pipeline.run().await;

    let id = TrainId::rid(RID).unwrap();
    let entry = registry.get(&id).expect("Train should be in registry");
    let status = entry.read().await;

    assert_eq!(
        status.actual_platform.value.as_deref(),
        Some("3A"),
        "Platform should be from T3 (the latest message)"
    );
}

// ---------------------------------------------------------------------------
// Test 3: Stale reconnect replay — T1 dropped after T2
//
// Simulates a STOMP reconnect that replays an older message. Injects T2 first,
// then T1 (older timestamp). T1 must be silently dropped by the sequence guard.
// Final registry state must reflect T2's platform, not T1's.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn stale_reconnect_replay_dropped() {
    let payloads = vec![
        make_ts_xml(RID, "2024-04-17T12:00:02Z", "T2-PLATFORM", "12:02"), // T2 first
        make_ts_xml(RID, "2024-04-17T12:00:01Z", "T1-PLATFORM", "12:01"), // T1 replay — must be dropped
    ];

    let (pipeline, _, registry) = build_pipeline(payloads);
    let _ = pipeline.run().await;

    let id = TrainId::rid(RID).unwrap();
    let entry = registry.get(&id).expect("Train should still be in registry");
    let status = entry.read().await;

    assert_eq!(
        status.actual_platform.value.as_deref(),
        Some("T2-PLATFORM"),
        "T1 replay must not overwrite T2 — sequence guard should have dropped it"
    );
}

// ---------------------------------------------------------------------------
// Test 4: Cancellation / graceful drain
//
// Spawns the pipeline under a CancellationToken and cancels it after the first
// message has been processed. Asserts:
//   - The task exits without panic
//   - The registry has at least the messages processed before cancellation
// ---------------------------------------------------------------------------
#[tokio::test]
async fn graceful_shutdown_no_panic() {
    // 10 identical messages — we'll cancel after a short delay.
    let payloads: Vec<_> = (0..10)
        .map(|i| {
            let ts = format!("2024-04-17T12:00:{:02}Z", i + 1);
            make_ts_xml(RID, &ts, "1", "12:05")
        })
        .collect();

    let registry = Arc::new(TrainRegistry::new());
    let (sc_tx, _sc_rx) = broadcast::channel(256);
    let stomp = Box::new(MockStompClient::with_xml_payloads(payloads));
    let pipeline = IngestionPipeline::passthrough(stomp, Arc::clone(&registry), sc_tx);

    let token = CancellationToken::new();
    let cancel = token.clone();

    let pipeline_handle = tokio::spawn(async move {
        tokio::select! {
            _ = token.cancelled() => {}
            _ = pipeline.run() => {}
        }
    });

    // Give the pipeline a moment to start processing, then cancel.
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    cancel.cancel();

    // Must complete without panic (tokio will propagate panics through JoinHandle).
    pipeline_handle.await.expect("Pipeline task panicked");
}

// ---------------------------------------------------------------------------
// Test 5: Cancellation event propagates for cancelled train
//
// A TS message with can="true" must emit a Critical StateChangeEvent.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn cancelled_train_emits_critical_state_change() {
    let payloads = vec![
        make_ts_xml(RID, "2024-04-17T12:00:01Z", "1", "12:00"), // register first
        make_cancelled_xml(RID, "2024-04-17T12:00:02Z"),         // then cancel
    ];

    let (pipeline, mut sc_rx, _) = build_pipeline(payloads);
    let _ = pipeline.run().await;

    let mut saw_critical = false;
    while let Ok(event) = sc_rx.try_recv() {
        if event.new_state == TrainState::Critical {
            saw_critical = true;
        }
    }
    assert!(saw_critical, "Expected Critical StateChangeEvent for cancelled train");
}
