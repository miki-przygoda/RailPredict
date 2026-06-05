//! Darwin Push Port XML parser.
//!
//! ## Parser choice
//! Uses `quick-xml`'s event-based reader rather than `serde-xml-rs` for two reasons:
//! 1. Darwin XML uses multiple namespace prefixes (ns3:, ns5:, etc.) that vary between
//!    message versions. `quick-xml`'s `local_name()` strips prefixes reliably.
//! 2. The event API is zero-copy and streaming — no intermediate DOM allocation.
//!
//! ## Messages handled
//! - `TS` (Train Status): delay, platform, estimated departure, cancellation flag, plus the
//!   full per-`<Location>` calling pattern (arr/dep est+actual, platform confidence, per-stop
//!   cancel) and late/cancel reason codes — see Full-Journey Capture (`docs/...journey...`).
//! - `schedule` (SC): operator (`toc`), train category, and the canonical ordered calling
//!   pattern that partial TS actuals hang onto.
//! - `deactivated`: train has been cancelled or has departed — triggers Terminal state.
//! - `Association` (NP): turnround predecessor → successor link.
//!
//! ## Envelope
//! All Darwin messages are wrapped in a `<Pport ts="..." version="...">` element.
//! The `ts` attribute is the authoritative message timestamp used by the sequence guard.
//!
//! ## Darwin XML structure (simplified)
//! ```xml
//! <Pport ts="2024-04-17T12:00:00" version="16.0">
//!   <uR updateOrigin="Darwin">
//!     <TS rid="202404170123456" ssd="2024-04-17" uid="C12345">
//!       <Location tpl="LEEDS" wtd="12:00" ptd="12:00">
//!         <dep et="12:05" at="12:07" delayed="true"/>
//!       </Location>
//!     </TS>
//!   </uR>
//! </Pport>
//! ```

use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use quick_xml::{
    events::{BytesStart, Event},
    Reader,
};
use thiserror::Error;

use crate::types::TrainId;

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// A parsed update ready to be applied to the registry.
#[derive(Debug, Clone)]
pub enum ParsedUpdate {
    TrainStatus(TsUpdate),
    Deactivated(DeactivatedUpdate),
    /// Parsed content of a Darwin `schedule` (SC) message: operator + planned plan.
    Schedule(ScheduleUpdate),
    /// Parsed content of a Darwin `Association` message (category `NP` only).
    Association {
        /// RID of the incoming (previous working) service.
        prev_rid: String,
        /// RID of the outgoing (next part) service being formed from the same stock.
        next_rid: String,
    },
}

/// A Darwin late/cancel reason: the numeric code plus the TIPLOC it was attributed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasonRef {
    pub code: i32,
    pub tiploc: Option<String>,
}

/// One calling point parsed from a Darwin message.
///
/// Produced both by `TS` (live est/actual times, platform, per-stop cancel) and by `schedule`
/// (planned times + activity only). Accumulated into `TrainStatus::journey` by `seq`, merging
/// newest non-null fields across partial messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallUpdate {
    pub tpl: String,
    pub seq: u16,
    pub sched_arr: Option<DateTime<Utc>>,
    pub sched_dep: Option<DateTime<Utc>>,
    pub est_arr: Option<DateTime<Utc>>,
    pub act_arr: Option<DateTime<Utc>>,
    pub est_dep: Option<DateTime<Utc>>,
    pub act_dep: Option<DateTime<Utc>>,
    pub platform: Option<String>,
    pub plat_confirmed: Option<bool>,
    pub is_cancelled: bool,
    /// Planned activity codes (e.g. "T" stop, "R" request, "U" set-down) — from `schedule`.
    pub activity: Option<String>,
}

impl CallUpdate {
    fn new(tpl: String, seq: u16) -> Self {
        Self {
            tpl,
            seq,
            sched_arr: None,
            sched_dep: None,
            est_arr: None,
            act_arr: None,
            est_dep: None,
            act_dep: None,
            platform: None,
            plat_confirmed: None,
            is_cancelled: false,
            activity: None,
        }
    }
}

/// Parsed content of a Darwin `TS` message.
#[derive(Debug, Clone)]
pub struct TsUpdate {
    pub rid: TrainId,
    /// RTTI UID (e.g. "C12345") — stable service identity, used as prediction key.
    pub uid: Option<String>,
    /// Scheduled service date — used to anchor NaiveTime departure times.
    pub ssd: NaiveDate,
    /// Scheduled public departure (from `ptd` attribute on the first Location).
    pub scheduled_departure: Option<DateTime<Utc>>,
    /// Estimated departure from Darwin (`et` attribute on `dep` element).
    pub estimated_departure: Option<DateTime<Utc>>,
    /// Actual recorded departure (`at` attribute), if the train has already left.
    pub actual_departure: Option<DateTime<Utc>>,
    /// Current platform assignment (`plat` attribute on Location).
    pub platform: Option<String>,
    /// Whether this service is cancelled (`can` attribute on TS element).
    pub is_cancelled: bool,
    /// Whether departure is flagged as delayed (`delayed` attribute on dep element).
    pub is_delayed: bool,
    /// CRS code of the first Location element (`tpl` attribute), used as origin station.
    pub station_crs: Option<String>,
    /// CRS code of the last Location element (`tpl` attribute), used as destination station.
    /// Equals `station_crs` when the TS message contains only a single Location element.
    pub destination_crs: Option<String>,
    /// Working timetable departure (`wtd` attribute on the first Location).
    /// Internal schedule with engineering margins — typically ≤ `scheduled_departure`.
    pub working_departure: Option<DateTime<Utc>>,
    /// Every `<Location>` in this message, in document order. Additive to the scalar fields
    /// above — the live predictor still uses the scalars; this feeds Full-Journey Capture.
    pub calls: Vec<CallUpdate>,
    /// Late-running reason (`<LateReason>` element or `delayReason` attribute), if present.
    pub late_reason: Option<ReasonRef>,
    /// Cancellation reason (`<CancelReason>` element or `cancelReason` attribute), if present.
    pub cancel_reason: Option<ReasonRef>,
}

/// Parsed content of a Darwin `schedule` (SC) message.
#[derive(Debug, Clone)]
pub struct ScheduleUpdate {
    pub rid: TrainId,
    pub uid: Option<String>,
    pub ssd: NaiveDate,
    /// Operating company (TOC) code — the operator-league signal.
    pub toc: Option<String>,
    /// Train category (express / stopper / freight …).
    pub train_category: Option<String>,
    /// Ordered planned calling pattern (`OR`/`IP`/`DT` and operational variants).
    pub calls: Vec<CallUpdate>,
    pub late_reason: Option<ReasonRef>,
    pub cancel_reason: Option<ReasonRef>,
}

/// Parsed content of a Darwin `deactivated` message.
#[derive(Debug, Clone)]
pub struct DeactivatedUpdate {
    pub rid: TrainId,
}

// Late vs cancel — which reason field a parsed `<…Reason>` element targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReasonKind {
    Late,
    Cancel,
}

// Darwin puts some values (platform number, reason code) in element *text*. This marks what
// the next `Text` event should be captured as. Reset on every element open.
#[derive(Debug, Clone, Copy)]
enum PendingText {
    Plat,
    ReasonCode,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("XML parse error: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("Missing required attribute '{attr}' on element '{element}'")]
    MissingAttribute { element: &'static str, attr: &'static str },

    #[error("Invalid RID '{0}': {1}")]
    InvalidRid(String, crate::types::train_id::TrainIdError),

    #[error("Invalid timestamp '{0}'")]
    InvalidTimestamp(String),
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a full Darwin Pport XML payload.
/// Returns the envelope timestamp and all parsed updates contained in the message.
pub fn parse_pport(xml: &str) -> Result<(DateTime<Utc>, Vec<ParsedUpdate>), ParseError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut pport_ts: Option<DateTime<Utc>> = None;
    let mut updates: Vec<ParsedUpdate> = Vec::new();

    // TS parse state.
    let mut current_ts: Option<TsUpdate> = None;
    let mut current_call: Option<CallUpdate> = None;
    let mut call_seq: u16 = 0;

    // schedule parse state.
    let mut current_schedule: Option<ScheduleUpdate> = None;
    let mut sched_seq: u16 = 0;

    // Text-capture target for the next `Text` event; reset on each element open.
    let mut pending_text: Option<PendingText> = None;
    // A reason element awaiting its text code: (kind, code?, tiploc).
    let mut pending_reason: Option<(ReasonKind, Option<i32>, Option<String>)> = None;

    // Association parse state (category NP only).
    let mut current_assoc_category: Option<String> = None;
    let mut current_assoc_main_rid: Option<String> = None;
    let mut current_assoc_next_rid: Option<String> = None;

    loop {
        let event = reader.read_event();
        match event {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let is_empty = matches!(event, Ok(Event::Empty(_)));
                let local = e.local_name();

                // Any element open ends a pending text capture (plat number / reason code must
                // immediately follow their element with no intervening element).
                pending_text = None;

                match local.as_ref() {
                    b"Pport" => {
                        pport_ts = Some(extract_pport_ts(e)?);
                    }
                    b"TS" => {
                        let rid_str = attr_str(e, b"rid", "TS", "rid")?;
                        let ssd_str = attr_str(e, b"ssd", "TS", "ssd")?;
                        let is_cancelled = attr_bool(e, b"can");
                        let uid = attr_opt(e, b"uid");
                        let rid = TrainId::rid(&rid_str)
                            .map_err(|err| ParseError::InvalidRid(rid_str.clone(), err))?;
                        let ssd = parse_ssd(&ssd_str)?;
                        let (late_reason, cancel_reason) = attr_reasons(e);
                        let ts = TsUpdate {
                            rid,
                            uid,
                            ssd,
                            scheduled_departure: None,
                            estimated_departure: None,
                            actual_departure: None,
                            platform: None,
                            is_cancelled,
                            is_delayed: false,
                            station_crs: None,
                            destination_crs: None,
                            working_departure: None,
                            calls: Vec::new(),
                            late_reason,
                            cancel_reason,
                        };
                        current_call = None;
                        call_seq = 0;
                        if is_empty {
                            // Self-closing `<TS/>` (rare) — no locations to gather.
                            updates.push(ParsedUpdate::TrainStatus(ts));
                            current_ts = None;
                        } else {
                            current_ts = Some(ts);
                        }
                    }
                    b"Location" if current_ts.is_some() => {
                        // Flush the previous location's call before starting a new one.
                        if let (Some(ts), Some(call)) = (current_ts.as_mut(), current_call.take()) {
                            ts.calls.push(call);
                        }
                        if let Some(ref mut ts) = current_ts {
                            let tpl = attr_opt(e, b"tpl");
                            // --- existing scalar logic (first location only) — UNCHANGED ---
                            if ts.scheduled_departure.is_none() {
                                if let Some(ptd) = attr_opt(e, b"ptd") {
                                    ts.scheduled_departure = parse_hhmm_on_date(&ptd, ts.ssd).ok();
                                }
                                if let Some(wtd) = attr_opt(e, b"wtd") {
                                    ts.working_departure = parse_hhmm_on_date(&wtd, ts.ssd).ok();
                                }
                                ts.platform = attr_opt(e, b"plat");
                                ts.station_crs = tpl.clone();
                            }
                            // Always update destination_crs to the most recently seen Location.
                            if tpl.is_some() {
                                ts.destination_crs = tpl.clone();
                            }
                            // --- new: build the per-call record (additive) ---
                            let mut call = CallUpdate::new(tpl.unwrap_or_default(), call_seq);
                            call_seq += 1;
                            call.sched_arr = parse_time_attr(e, b"pta", ts.ssd)
                                .or_else(|| parse_time_attr(e, b"wta", ts.ssd));
                            call.sched_dep = parse_time_attr(e, b"ptd", ts.ssd)
                                .or_else(|| parse_time_attr(e, b"wtd", ts.ssd));
                            call.platform = attr_opt(e, b"plat");
                            call.is_cancelled = attr_bool(e, b"can");
                            current_call = Some(call);
                        }
                    }
                    b"arr" if current_ts.is_some() => {
                        let ssd = current_ts.as_ref().map(|t| t.ssd);
                        if let (Some(call), Some(ssd)) = (current_call.as_mut(), ssd) {
                            if let Some(et) = parse_time_attr(e, b"et", ssd) {
                                call.est_arr = Some(et);
                            }
                            if let Some(at) = parse_time_attr(e, b"at", ssd) {
                                call.act_arr = Some(at);
                            }
                        }
                    }
                    b"dep" if current_ts.is_some() => {
                        let ssd = current_ts.as_ref().map(|t| t.ssd);
                        // The scalar reported-delay pair must come from the SAME stop:
                        // `scheduled_departure` is the first Location's `ptd`, so the scalar
                        // estimated/actual departure must be the first Location's `<dep>` too —
                        // the ORIGIN. Using the *last* `<dep>` in the message compared the last
                        // stop's time against the origin's schedule, inflating delay by the journey
                        // duration (Darwin TS messages are partial). Per-call data below still
                        // records every stop's own delay.
                        let is_origin = current_call.as_ref().map(|c| c.seq) == Some(0);
                        if is_origin && let Some(ref mut ts) = current_ts {
                            if let Some(et) = attr_opt(e, b"et") {
                                ts.estimated_departure = parse_hhmm_on_date(&et, ts.ssd).ok();
                            }
                            if let Some(at) = attr_opt(e, b"at") {
                                ts.actual_departure = parse_hhmm_on_date(&at, ts.ssd).ok();
                            }
                            ts.is_delayed = attr_bool(e, b"delayed");
                        }
                        // --- per-call departure forecast/actual (every stop) ---
                        if let (Some(call), Some(ssd)) = (current_call.as_mut(), ssd) {
                            if let Some(et) = parse_time_attr(e, b"et", ssd) {
                                call.est_dep = Some(et);
                            }
                            if let Some(at) = parse_time_attr(e, b"at", ssd) {
                                call.act_dep = Some(at);
                            }
                        }
                    }
                    b"plat" if current_call.is_some() => {
                        // Real Darwin carries the platform NUMBER as element text and a `conf`
                        // attribute for confirmation. (This codebase also accepts a `plat`
                        // attribute on Location, captured above.)
                        if let Some(call) = current_call.as_mut() {
                            call.plat_confirmed = Some(attr_bool(e, b"conf"));
                        }
                        pending_text = Some(PendingText::Plat);
                    }
                    b"LateReason" | b"CancelReason" => {
                        let kind = if local.as_ref() == b"LateReason" {
                            ReasonKind::Late
                        } else {
                            ReasonKind::Cancel
                        };
                        let code = attr_opt(e, b"code").and_then(|v| v.parse().ok());
                        let tiploc = attr_opt(e, b"tiploc");
                        if code.is_none() {
                            pending_text = Some(PendingText::ReasonCode);
                        }
                        pending_reason = Some((kind, code, tiploc));
                        if is_empty {
                            // Self-closing `<CancelReason code="100"/>` — attach immediately.
                            if let Some((kind, Some(code), tiploc)) = pending_reason.take() {
                                attach_reason(
                                    &mut current_ts,
                                    &mut current_schedule,
                                    kind,
                                    ReasonRef { code, tiploc },
                                );
                            }
                        }
                    }
                    b"schedule" => {
                        let rid_str = attr_str(e, b"rid", "schedule", "rid")?;
                        let rid = TrainId::rid(&rid_str)
                            .map_err(|err| ParseError::InvalidRid(rid_str.clone(), err))?;
                        let ssd = match attr_opt(e, b"ssd").map(|s| parse_ssd(&s)).transpose()? {
                            Some(d) => d,
                            None => match pport_ts {
                                Some(t) => t.date_naive(),
                                None => continue,
                            },
                        };
                        let (late_reason, cancel_reason) = attr_reasons(e);
                        let sched = ScheduleUpdate {
                            rid,
                            uid: attr_opt(e, b"uid"),
                            ssd,
                            toc: attr_opt(e, b"toc"),
                            train_category: attr_opt(e, b"trainCat"),
                            calls: Vec::new(),
                            late_reason,
                            cancel_reason,
                        };
                        sched_seq = 0;
                        if is_empty {
                            updates.push(ParsedUpdate::Schedule(sched));
                            current_schedule = None;
                        } else {
                            current_schedule = Some(sched);
                        }
                    }
                    // schedule calling-pattern elements: origin / intermediate / destination
                    // (+ operational variants that don't set down passengers).
                    b"OR" | b"IP" | b"DT" | b"OPOR" | b"OPIP" | b"OPDT"
                        if current_schedule.is_some() =>
                    {
                        if let Some(ref mut sched) = current_schedule
                            && let Some(tpl) = attr_opt(e, b"tpl")
                        {
                            let mut call = CallUpdate::new(tpl, sched_seq);
                            sched_seq += 1;
                            call.sched_arr = parse_time_attr(e, b"pta", sched.ssd)
                                .or_else(|| parse_time_attr(e, b"wta", sched.ssd));
                            call.sched_dep = parse_time_attr(e, b"ptd", sched.ssd)
                                .or_else(|| parse_time_attr(e, b"wtd", sched.ssd));
                            call.activity = attr_opt(e, b"act");
                            sched.calls.push(call);
                        }
                    }
                    b"deactivated" => {
                        let rid_str = attr_str(e, b"rid", "deactivated", "rid")?;
                        let rid = TrainId::rid(&rid_str)
                            .map_err(|err| ParseError::InvalidRid(rid_str.clone(), err))?;
                        updates.push(ParsedUpdate::Deactivated(DeactivatedUpdate { rid }));
                    }
                    b"Association" => {
                        // Only track NP (Next Part / turnround) associations.
                        current_assoc_category = attr_opt(e, b"category");
                        current_assoc_main_rid = None;
                        current_assoc_next_rid = None;
                    }
                    b"main" if current_assoc_category.as_deref() == Some("NP") => {
                        current_assoc_main_rid = attr_opt(e, b"rid");
                    }
                    b"assoc" if current_assoc_category.as_deref() == Some("NP") => {
                        current_assoc_next_rid = attr_opt(e, b"rid");
                    }
                    _ => {}
                }
            }

            Ok(Event::Text(ref t)) => match pending_text.take() {
                Some(PendingText::Plat) => {
                    if let Ok(s) = t.unescape() {
                        let p = s.trim().to_string();
                        if !p.is_empty() {
                            let is_first = current_call.as_ref().map(|c| c.seq) == Some(0);
                            if let Some(call) = current_call.as_mut() {
                                call.platform = Some(p.clone());
                            }
                            // Mirror existing scalar behaviour: platform comes from the origin.
                            if is_first && let Some(ts) = current_ts.as_mut() {
                                ts.platform = Some(p);
                            }
                        }
                    }
                }
                Some(PendingText::ReasonCode) => {
                    if let Ok(s) = t.unescape()
                        && let Ok(code) = s.trim().parse::<i32>()
                        && let Some(pr) = pending_reason.as_mut()
                    {
                        pr.1 = Some(code);
                    }
                }
                None => {}
            },

            Ok(Event::End(ref e)) => {
                match e.local_name().as_ref() {
                    b"TS" => {
                        if let Some(mut ts) = current_ts.take() {
                            if let Some(call) = current_call.take() {
                                ts.calls.push(call);
                            }
                            updates.push(ParsedUpdate::TrainStatus(ts));
                        }
                    }
                    b"schedule" => {
                        if let Some(sched) = current_schedule.take() {
                            updates.push(ParsedUpdate::Schedule(sched));
                        }
                    }
                    b"LateReason" | b"CancelReason" => {
                        if let Some((kind, Some(code), tiploc)) = pending_reason.take() {
                            attach_reason(
                                &mut current_ts,
                                &mut current_schedule,
                                kind,
                                ReasonRef { code, tiploc },
                            );
                        }
                        pending_text = None;
                    }
                    b"Association" => {
                        // Emit only if this was an NP association and both RIDs were captured.
                        if current_assoc_category.as_deref() == Some("NP")
                            && let (Some(prev_rid), Some(next_rid)) = (
                                current_assoc_main_rid.take(),
                                current_assoc_next_rid.take(),
                            )
                        {
                            updates.push(ParsedUpdate::Association { prev_rid, next_rid });
                        }
                        current_assoc_category = None;
                        current_assoc_main_rid = None;
                        current_assoc_next_rid = None;
                    }
                    _ => {}
                }
            }

            Ok(Event::Eof) => break,
            Err(e) => return Err(ParseError::Xml(e)),
            _ => {}
        }
    }

    let ts = pport_ts.ok_or(ParseError::MissingAttribute {
        element: "Pport",
        attr: "ts",
    })?;

    Ok((ts, updates))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_pport_ts(e: &BytesStart) -> Result<DateTime<Utc>, ParseError> {
    let ts_str = attr_str(e, b"ts", "Pport", "ts")?;
    DateTime::parse_from_rfc3339(&ts_str)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| ParseError::InvalidTimestamp(ts_str))
}

fn attr_str(
    e: &BytesStart,
    name: &[u8],
    element: &'static str,
    attr: &'static str,
) -> Result<String, ParseError> {
    e.attributes()
        .filter_map(|a| a.ok())
        .find(|a| a.key.local_name().as_ref() == name)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
        .ok_or(ParseError::MissingAttribute { element, attr })
}

fn attr_opt(e: &BytesStart, name: &[u8]) -> Option<String> {
    e.attributes()
        .filter_map(|a| a.ok())
        .find(|a| a.key.local_name().as_ref() == name)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn attr_bool(e: &BytesStart, name: &[u8]) -> bool {
    attr_opt(e, name).is_some_and(|v| v == "true")
}

/// Parse an `HH:MM`(`:SS`) time attribute into a `DateTime<Utc>` anchored on `ssd`.
fn parse_time_attr(e: &BytesStart, name: &[u8], ssd: NaiveDate) -> Option<DateTime<Utc>> {
    attr_opt(e, name).and_then(|v| parse_hhmm_on_date(&v, ssd).ok())
}

/// Extract attribute-form reasons (`delayReason` / `cancelReason`) from an element.
fn attr_reasons(e: &BytesStart) -> (Option<ReasonRef>, Option<ReasonRef>) {
    let late = attr_opt(e, b"delayReason")
        .and_then(|v| v.parse().ok())
        .map(|code| ReasonRef {
            code,
            tiploc: attr_opt(e, b"delayReasonTiploc"),
        });
    let cancel = attr_opt(e, b"cancelReason")
        .and_then(|v| v.parse().ok())
        .map(|code| ReasonRef {
            code,
            tiploc: attr_opt(e, b"cancelReasonTiploc"),
        });
    (late, cancel)
}

/// Attach a parsed element-form reason to whichever container is currently open (TS first,
/// then schedule). No-op if neither is open.
fn attach_reason(
    ts: &mut Option<TsUpdate>,
    sched: &mut Option<ScheduleUpdate>,
    kind: ReasonKind,
    reason: ReasonRef,
) {
    let (late, cancel) = if let Some(t) = ts.as_mut() {
        (&mut t.late_reason, &mut t.cancel_reason)
    } else if let Some(s) = sched.as_mut() {
        (&mut s.late_reason, &mut s.cancel_reason)
    } else {
        return;
    };
    match kind {
        ReasonKind::Late => *late = Some(reason),
        ReasonKind::Cancel => *cancel = Some(reason),
    }
}

fn parse_ssd(ssd: &str) -> Result<NaiveDate, ParseError> {
    NaiveDate::parse_from_str(ssd, "%Y-%m-%d")
        .map_err(|_| ParseError::InvalidTimestamp(ssd.to_string()))
}

/// Parse a `HH:MM` time string into a `DateTime<Utc>` anchored on `date`.
fn parse_hhmm_on_date(hhmm: &str, date: NaiveDate) -> Result<DateTime<Utc>, ParseError> {
    let t = NaiveTime::parse_from_str(hhmm, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(hhmm, "%H:%M:%S"))
        .map_err(|_| ParseError::InvalidTimestamp(hhmm.to_string()))?;
    Utc.from_local_datetime(&date.and_time(t))
        .single()
        .ok_or_else(|| ParseError::InvalidTimestamp(hhmm.to_string()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TS: &str = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:00:00Z" version="16.0">
  <uR updateOrigin="Darwin">
    <TS rid="202404170123456" ssd="2024-04-17" uid="C12345">
      <Location tpl="LEEDS" ptd="12:00" plat="3">
        <dep et="12:05" delayed="true"/>
      </Location>
    </TS>
  </uR>
</Pport>"#;

    const SAMPLE_CANCELLED: &str = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:00:00Z" version="16.0">
  <uR>
    <TS rid="202404170123456" ssd="2024-04-17" uid="C12345" can="true">
      <Location tpl="LEEDS" ptd="12:00"/>
    </TS>
  </uR>
</Pport>"#;

    const SAMPLE_DEACTIVATED: &str = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T13:00:00Z" version="16.0">
  <uR>
    <deactivated rid="202404170123456"/>
  </uR>
</Pport>"#;

    #[test]
    fn parses_ts_message() {
        let (ts, updates) = parse_pport(SAMPLE_TS).unwrap();
        assert_eq!(updates.len(), 1);
        assert!(matches!(updates[0], ParsedUpdate::TrainStatus(_)));
        assert_eq!(ts.to_rfc3339(), "2024-04-17T12:00:00+00:00");
    }

    #[test]
    fn ts_extracts_rid_and_platform() {
        let (_, updates) = parse_pport(SAMPLE_TS).unwrap();
        if let ParsedUpdate::TrainStatus(ts) = &updates[0] {
            assert_eq!(ts.rid.as_str(), "202404170123456");
            assert_eq!(ts.platform.as_deref(), Some("3"));
            assert!(ts.is_delayed);
        } else {
            panic!("expected TrainStatus");
        }
    }

    #[test]
    fn ts_extracts_estimated_departure() {
        let (_, updates) = parse_pport(SAMPLE_TS).unwrap();
        if let ParsedUpdate::TrainStatus(ts) = &updates[0] {
            assert!(ts.estimated_departure.is_some());
        }
    }

    #[test]
    fn cancelled_flag_parsed() {
        let (_, updates) = parse_pport(SAMPLE_CANCELLED).unwrap();
        if let ParsedUpdate::TrainStatus(ts) = &updates[0] {
            assert!(ts.is_cancelled);
        } else {
            panic!("expected TrainStatus");
        }
    }

    #[test]
    fn parses_deactivated_message() {
        let (_, updates) = parse_pport(SAMPLE_DEACTIVATED).unwrap();
        assert_eq!(updates.len(), 1);
        assert!(matches!(updates[0], ParsedUpdate::Deactivated(_)));
    }

    #[test]
    fn missing_pport_ts_returns_error() {
        let xml = r#"<Pport version="16.0"><uR></uR></Pport>"#;
        assert!(parse_pport(xml).is_err());
    }

    #[test]
    fn multiple_ts_updates_in_one_pport() {
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:00:00Z" version="16.0">
  <uR>
    <TS rid="202404170000001" ssd="2024-04-17" uid="A00001">
      <Location tpl="LDS" ptd="12:00"/>
    </TS>
    <TS rid="202404170000002" ssd="2024-04-17" uid="A00002">
      <Location tpl="MAN" ptd="13:00"/>
    </TS>
  </uR>
</Pport>"#;
        let (_, updates) = parse_pport(xml).unwrap();
        assert_eq!(updates.len(), 2);
        assert!(matches!(updates[0], ParsedUpdate::TrainStatus(_)));
        assert!(matches!(updates[1], ParsedUpdate::TrainStatus(_)));
        if let ParsedUpdate::TrainStatus(ts) = &updates[0] {
            assert_eq!(ts.rid.as_str(), "202404170000001");
        }
        if let ParsedUpdate::TrainStatus(ts) = &updates[1] {
            assert_eq!(ts.rid.as_str(), "202404170000002");
        }
    }

    #[test]
    fn ts_with_actual_departure_parsed() {
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:10:00Z" version="16.0">
  <uR>
    <TS rid="202404170123456" ssd="2024-04-17" uid="C12345">
      <Location tpl="LEEDS" ptd="12:00" plat="3">
        <dep at="12:07"/>
      </Location>
    </TS>
  </uR>
</Pport>"#;
        let (_, updates) = parse_pport(xml).unwrap();
        if let ParsedUpdate::TrainStatus(ts) = &updates[0] {
            assert!(ts.actual_departure.is_some(), "actual_departure should be parsed from 'at'");
            assert!(ts.estimated_departure.is_none());
        } else {
            panic!("expected TrainStatus");
        }
    }

    #[test]
    fn scalar_departure_comes_from_origin_not_last_stop() {
        // Origin departs ~on time (+2); a later stop's forecast is 90 min after the origin's
        // schedule. The scalar estimated_departure must be the ORIGIN's dep (so reported delay
        // reads ≈ +2), NOT the last stop's (which would read ≈ +90 = the journey length).
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T10:00:00Z" version="16.0">
  <uR>
    <TS rid="202404170123456" ssd="2024-04-17" uid="C12345">
      <Location tpl="LEEDS" ptd="10:00"><dep et="10:02" delayed="true"/></Location>
      <Location tpl="WAKEFLD" pta="10:30" ptd="10:32"><dep et="11:30"/></Location>
    </TS>
  </uR>
</Pport>"#;
        let (_, updates) = parse_pport(xml).unwrap();
        let ParsedUpdate::TrainStatus(ts) = &updates[0] else {
            panic!("expected TrainStatus");
        };
        assert_eq!(
            ts.estimated_departure.map(|d| d.to_rfc3339()),
            Some("2024-04-17T10:02:00+00:00".to_string()),
            "scalar estimated_departure must be the origin's dep, not the last stop's"
        );
        // The per-call vector still captures the later stop's forecast.
        assert_eq!(ts.calls.len(), 2);
        assert!(ts.calls[1].est_dep.is_some());
    }

    #[test]
    fn ts_missing_rid_returns_error() {
        // `rid` is required on a TS element — omitting it should cause a parse error.
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:00:00Z" version="16.0">
  <uR>
    <TS ssd="2024-04-17" uid="C12345">
      <Location tpl="LDS" ptd="12:00"/>
    </TS>
  </uR>
</Pport>"#;
        assert!(parse_pport(xml).is_err());
    }

    #[test]
    fn ts_missing_ssd_returns_error() {
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:00:00Z" version="16.0">
  <uR>
    <TS rid="202404170123456" uid="C12345">
      <Location tpl="LDS" ptd="12:00"/>
    </TS>
  </uR>
</Pport>"#;
        assert!(parse_pport(xml).is_err());
    }

    #[test]
    fn parses_np_association_message() {
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:00:00Z" version="16.0">
  <uR>
    <Association tiploc="LEEDS" category="NP">
      <main rid="202404170000001" wta="12:00" wtd="12:05"/>
      <assoc rid="202404170000002" wta="12:30" wtd="12:35"/>
    </Association>
  </uR>
</Pport>"#;
        let (_, updates) = parse_pport(xml).unwrap();
        assert_eq!(updates.len(), 1);
        if let ParsedUpdate::Association { prev_rid, next_rid } = &updates[0] {
            assert_eq!(prev_rid, "202404170000001");
            assert_eq!(next_rid, "202404170000002");
        } else {
            panic!("expected Association update");
        }
    }

    #[test]
    fn non_np_association_is_ignored() {
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:00:00Z" version="16.0">
  <uR>
    <Association tiploc="LEEDS" category="JJ">
      <main rid="202404170000001" wta="12:00" wtd="12:05"/>
      <assoc rid="202404170000002" wta="12:30" wtd="12:35"/>
    </Association>
  </uR>
</Pport>"#;
        let (_, updates) = parse_pport(xml).unwrap();
        assert_eq!(updates.len(), 0, "JJ (join) associations should be ignored");
    }

    #[test]
    fn ts_and_deactivated_in_same_pport() {
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:00:00Z" version="16.0">
  <uR>
    <TS rid="202404170000001" ssd="2024-04-17" uid="A00001">
      <Location tpl="LDS" ptd="12:00"/>
    </TS>
    <deactivated rid="202404170000002"/>
  </uR>
</Pport>"#;
        let (_, updates) = parse_pport(xml).unwrap();
        assert_eq!(updates.len(), 2);
        assert!(matches!(updates[0], ParsedUpdate::TrainStatus(_)));
        assert!(matches!(updates[1], ParsedUpdate::Deactivated(_)));
    }

    // --- Full-Journey Capture: new coverage ---

    #[test]
    fn ts_captures_every_location_as_a_call() {
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:00:00Z" version="16.0">
  <uR>
    <TS rid="202404170123456" ssd="2024-04-17" uid="C12345">
      <Location tpl="LEEDS" ptd="12:00" plat="3"><dep et="12:05" delayed="true"/></Location>
      <Location tpl="WAKEFLD" pta="12:18" ptd="12:20"><arr et="12:23"/><dep et="12:25"/></Location>
      <Location tpl="SHEFFLD" pta="12:50"><arr at="12:58"/></Location>
    </TS>
  </uR>
</Pport>"#;
        let (_, updates) = parse_pport(xml).unwrap();
        let ParsedUpdate::TrainStatus(ts) = &updates[0] else {
            panic!("expected TrainStatus");
        };
        assert_eq!(ts.calls.len(), 3, "all three locations captured");
        assert_eq!(ts.calls[0].tpl, "LEEDS");
        assert_eq!(ts.calls[0].seq, 0);
        assert!(ts.calls[0].est_dep.is_some());
        assert_eq!(ts.calls[1].tpl, "WAKEFLD");
        assert!(ts.calls[1].est_arr.is_some() && ts.calls[1].est_dep.is_some());
        // Destination keeps its scalar role.
        assert_eq!(ts.destination_crs.as_deref(), Some("SHEFFLD"));
        assert!(ts.calls[2].act_arr.is_some(), "actual arrival captured at destination");
        // Scalars still behave as before.
        assert_eq!(ts.station_crs.as_deref(), Some("LEEDS"));
        assert_eq!(ts.platform.as_deref(), Some("3"));
    }

    #[test]
    fn ts_extracts_element_form_late_reason() {
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:00:00Z" version="16.0">
  <uR>
    <TS rid="202404170123456" ssd="2024-04-17" uid="C12345">
      <LateReason tiploc="LEEDS">168</LateReason>
      <Location tpl="LEEDS" ptd="12:00"><dep et="12:09" delayed="true"/></Location>
    </TS>
  </uR>
</Pport>"#;
        let (_, updates) = parse_pport(xml).unwrap();
        let ParsedUpdate::TrainStatus(ts) = &updates[0] else {
            panic!("expected TrainStatus");
        };
        let r = ts.late_reason.as_ref().expect("late reason present");
        assert_eq!(r.code, 168);
        assert_eq!(r.tiploc.as_deref(), Some("LEEDS"));
    }

    #[test]
    fn ts_extracts_self_closing_cancel_reason() {
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:00:00Z" version="16.0">
  <uR>
    <TS rid="202404170123456" ssd="2024-04-17" uid="C12345" can="true">
      <CancelReason code="100" tiploc="LEEDS"/>
      <Location tpl="LEEDS" ptd="12:00"/>
    </TS>
  </uR>
</Pport>"#;
        let (_, updates) = parse_pport(xml).unwrap();
        let ParsedUpdate::TrainStatus(ts) = &updates[0] else {
            panic!("expected TrainStatus");
        };
        let r = ts.cancel_reason.as_ref().expect("cancel reason present");
        assert_eq!(r.code, 100);
        assert!(ts.is_cancelled);
    }

    #[test]
    fn ts_per_stop_cancel_flag() {
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T12:00:00Z" version="16.0">
  <uR>
    <TS rid="202404170123456" ssd="2024-04-17" uid="C12345">
      <Location tpl="LEEDS" ptd="12:00"><dep et="12:00"/></Location>
      <Location tpl="WAKEFLD" pta="12:18" can="true"/>
    </TS>
  </uR>
</Pport>"#;
        let (_, updates) = parse_pport(xml).unwrap();
        let ParsedUpdate::TrainStatus(ts) = &updates[0] else {
            panic!("expected TrainStatus");
        };
        assert!(!ts.calls[0].is_cancelled);
        assert!(ts.calls[1].is_cancelled, "per-stop can flag captured");
    }

    #[test]
    fn parses_schedule_with_toc_and_calls() {
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T08:00:00Z" version="16.0">
  <uR>
    <schedule rid="202404170123456" uid="C12345" ssd="2024-04-17" toc="GW" trainCat="OO">
      <OR tpl="PADTON" ptd="08:00"/>
      <IP tpl="READING" pta="08:25" ptd="08:27"/>
      <DT tpl="BRISTM" pta="09:40"/>
    </schedule>
  </uR>
</Pport>"#;
        let (_, updates) = parse_pport(xml).unwrap();
        assert_eq!(updates.len(), 1);
        let ParsedUpdate::Schedule(s) = &updates[0] else {
            panic!("expected Schedule");
        };
        assert_eq!(s.toc.as_deref(), Some("GW"));
        assert_eq!(s.train_category.as_deref(), Some("OO"));
        assert_eq!(s.calls.len(), 3);
        assert_eq!(s.calls[0].tpl, "PADTON");
        assert_eq!(s.calls[0].seq, 0);
        assert_eq!(s.calls[2].tpl, "BRISTM");
        assert_eq!(s.calls[2].seq, 2);
        assert!(s.calls[1].sched_arr.is_some() && s.calls[1].sched_dep.is_some());
    }

    #[test]
    fn schedule_attribute_form_cancel_reason() {
        let xml = r#"<?xml version="1.0"?>
<Pport ts="2024-04-17T08:00:00Z" version="16.0">
  <uR>
    <schedule rid="202404170123456" uid="C12345" ssd="2024-04-17" cancelReason="100"/>
  </uR>
</Pport>"#;
        let (_, updates) = parse_pport(xml).unwrap();
        let ParsedUpdate::Schedule(s) = &updates[0] else {
            panic!("expected Schedule");
        };
        assert_eq!(s.cancel_reason.as_ref().unwrap().code, 100);
    }
}
