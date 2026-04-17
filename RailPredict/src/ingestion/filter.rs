//! Region/route filter and sequence guard for the Darwin firehose.
//!
//! ## Filter-first principle
//! The filter is applied to raw XML bytes BEFORE any full parse. A quick string scan
//! for the message-type element is orders of magnitude cheaper than deserialising the
//! full Darwin XML tree. At Darwin's peak rate (~400 msg/s) the CPU saving is material.
//!
//! ## Darwin message type taxonomy
//!
//! | Type            | Local element name   | Decision      | Reason                                      |
//! |-----------------|----------------------|---------------|---------------------------------------------|
//! | Train Status    | `TS`                 | KEEP          | Core live update — delay, platform, cancel  |
//! | Deactivated     | `deactivated`        | KEEP          | Train cancelled/terminated → Terminal state |
//! | Schedule        | `schedule`           | CONDITIONAL   | Needed only if we haven't seen this RID yet |
//! | SF (forecast)   | `SF`                 | CONDITIONAL   | Supplementary; process only for known RIDs  |
//! | Station Message | `OW`                 | DROP          | Passenger announcements; not machine-useful |
//! | Train Alert     | `trainAlert`         | DROP          | Redundant with TS cancellation flag         |
//! | Association     | `association`        | DROP          | Splitting/joining trains; out of scope v1   |
//! | Alarm           | `alarm`              | DROP          | Internal NR system alarm; not relevant      |
//!
//! ## Sequence guard
//! Darwin can deliver messages out of order and replays messages on STOMP reconnect.
//! The guard tracks the last-applied Pport `ts` timestamp per `TrainId`. Any incoming
//! message with a timestamp ≤ the stored value is silently dropped.
//! Semantics: per-TrainId (not global) because a late message for train A must not
//! suppress a timely message for train B.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use dashmap::DashMap;

use crate::types::TrainId;

// ---------------------------------------------------------------------------
// Message taxonomy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageDecision {
    Keep,
    Conditional,
    Drop,
}

/// Classify a raw Darwin XML payload by scanning for known element names.
/// This is a deliberately cheap scan — no full parse.
pub fn classify_message(xml: &[u8]) -> MessageDecision {
    // Scan for the first known element name after "<" (ignoring namespace prefixes).
    // We look for the local part only, so `<ns3:TS ` and `<TS ` both match.
    for (element, decision) in TAXONOMY {
        if contains_element(xml, element) {
            return *decision;
        }
    }
    MessageDecision::Drop
}

/// Ordered taxonomy: KEEP entries checked first to short-circuit.
static TAXONOMY: &[(&[u8], MessageDecision)] = &[
    (b"TS",          MessageDecision::Keep),
    (b"deactivated", MessageDecision::Keep),
    (b"schedule",    MessageDecision::Conditional),
    (b"SF",          MessageDecision::Conditional),
    (b"OW",          MessageDecision::Drop),
    (b"trainAlert",  MessageDecision::Drop),
    (b"association", MessageDecision::Drop),
    (b"alarm",       MessageDecision::Drop),
];

/// Returns `true` if the XML bytes contain `<{name}` or `:{name}` (namespace-qualified).
fn contains_element(xml: &[u8], name: &[u8]) -> bool {
    xml.windows(name.len() + 1).any(|w| {
        (w[0] == b'<' || w[0] == b':') && &w[1..] == name
            || w[0] == b'<' && w[1..].starts_with(name)
    })
}

// ---------------------------------------------------------------------------
// Sequence guard
// ---------------------------------------------------------------------------

/// Tracks the last-applied Pport timestamp per TrainId to prevent stale overwrites.
pub struct SequenceGuard {
    last_seen: DashMap<TrainId, DateTime<Utc>>,
}

impl SequenceGuard {
    pub fn new() -> Self {
        Self { last_seen: DashMap::new() }
    }

    /// Returns `true` if `msg_ts` is strictly newer than the last-seen timestamp for
    /// this train (or if no timestamp has been recorded yet).
    /// Updates the stored timestamp on success.
    pub fn should_apply(&self, id: &TrainId, msg_ts: DateTime<Utc>) -> bool {
        let mut entry = self.last_seen.entry(id.clone()).or_insert(DateTime::<Utc>::MIN_UTC);
        if msg_ts > *entry {
            *entry = msg_ts;
            true
        } else {
            false
        }
    }

    /// Clear the sequence record for a train (called when it is evicted from the registry).
    pub fn forget(&self, id: &TrainId) {
        self.last_seen.remove(id);
    }
}

impl Default for SequenceGuard {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Filter
// ---------------------------------------------------------------------------

/// Combines the message taxonomy check and the sequence guard into one gate.
pub struct Filter {
    /// CRS codes (3-letter station codes) or route identifiers to watch.
    /// Empty set = watch everything (useful in tests or single-region deployments).
    watched_routes: HashSet<String>,
    sequence_guard: SequenceGuard,
}

impl Filter {
    pub fn new(watched_routes: HashSet<String>) -> Self {
        Self { watched_routes, sequence_guard: SequenceGuard::new() }
    }

    /// A filter that passes all messages (no route restriction).
    pub fn passthrough() -> Self {
        Self::new(HashSet::new())
    }

    /// First gate: classify the raw message and apply route filtering.
    /// Returns `false` to drop; `true` to proceed to the sequence check + parse.
    pub fn should_parse(&self, xml: &[u8], relevant_crs: Option<&str>) -> bool {
        match classify_message(xml) {
            MessageDecision::Drop => false,
            MessageDecision::Keep => self.route_allowed(relevant_crs),
            MessageDecision::Conditional => self.route_allowed(relevant_crs),
        }
    }

    /// Second gate: confirm the message timestamp is newer than the last-applied one.
    pub fn should_apply(&self, id: &TrainId, msg_ts: DateTime<Utc>) -> bool {
        self.sequence_guard.should_apply(id, msg_ts)
    }

    pub fn forget(&self, id: &TrainId) {
        self.sequence_guard.forget(id);
    }

    fn route_allowed(&self, crs: Option<&str>) -> bool {
        if self.watched_routes.is_empty() {
            return true;
        }
        crs.is_some_and(|c| self.watched_routes.contains(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn ts_message_classified_as_keep() {
        let xml = b"<Pport><uR><TS rid=\"123\"/></uR></Pport>";
        assert_eq!(classify_message(xml), MessageDecision::Keep);
    }

    #[test]
    fn deactivated_classified_as_keep() {
        let xml = b"<Pport><uR><deactivated rid=\"123\"/></uR></Pport>";
        assert_eq!(classify_message(xml), MessageDecision::Keep);
    }

    #[test]
    fn schedule_classified_as_conditional() {
        let xml = b"<Pport><uR><schedule rid=\"123\"/></uR></Pport>";
        assert_eq!(classify_message(xml), MessageDecision::Conditional);
    }

    #[test]
    fn ow_classified_as_drop() {
        let xml = b"<Pport><uR><OW>some announcement</OW></uR></Pport>";
        assert_eq!(classify_message(xml), MessageDecision::Drop);
    }

    #[test]
    fn unknown_element_classified_as_drop() {
        let xml = b"<Pport><uR><unknownElement/></uR></Pport>";
        assert_eq!(classify_message(xml), MessageDecision::Drop);
    }

    #[test]
    fn namespace_qualified_ts_classified_as_keep() {
        let xml = b"<Pport><uR><ns3:TS rid=\"123\"/></uR></Pport>";
        assert_eq!(classify_message(xml), MessageDecision::Keep);
    }

    #[test]
    fn sequence_guard_allows_newer_timestamp() {
        let guard = SequenceGuard::new();
        let id = TrainId::rid("202404170000001").unwrap();
        let t1 = Utc::now();
        let t2 = t1 + Duration::seconds(1);
        assert!(guard.should_apply(&id, t1));
        assert!(guard.should_apply(&id, t2));
    }

    #[test]
    fn sequence_guard_rejects_older_timestamp() {
        let guard = SequenceGuard::new();
        let id = TrainId::rid("202404170000001").unwrap();
        let t1 = Utc::now();
        let t0 = t1 - Duration::seconds(5);
        guard.should_apply(&id, t1);
        assert!(!guard.should_apply(&id, t0));
    }

    #[test]
    fn sequence_guard_rejects_same_timestamp() {
        let guard = SequenceGuard::new();
        let id = TrainId::rid("202404170000001").unwrap();
        let t = Utc::now();
        guard.should_apply(&id, t);
        assert!(!guard.should_apply(&id, t));
    }

    #[test]
    fn passthrough_filter_allows_all_routes() {
        let filter = Filter::passthrough();
        let xml = b"<Pport><uR><TS rid=\"1\"/></uR></Pport>";
        assert!(filter.should_parse(xml, Some("LDS")));
        assert!(filter.should_parse(xml, None));
    }

    #[test]
    fn route_filter_blocks_unmatched_crs() {
        let filter = Filter::new(["LDS".to_string()].into());
        let xml = b"<Pport><uR><TS rid=\"1\"/></uR></Pport>";
        assert!(filter.should_parse(xml, Some("LDS")));
        assert!(!filter.should_parse(xml, Some("MAN")));
    }
}
