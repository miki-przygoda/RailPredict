//! Darwin Push Port XML parser.
//!
//! ## Parser choice
//! Uses `quick-xml`'s event-based reader rather than `serde-xml-rs` for two reasons:
//! 1. Darwin XML uses multiple namespace prefixes (ns3:, ns5:, etc.) that vary between
//!    message versions. `quick-xml`'s `local_name()` strips prefixes reliably.
//! 2. The event API is zero-copy and streaming — no intermediate DOM allocation.
//!
//! ## Messages handled
//! - `TS` (Train Status): delay, platform, estimated departure, cancellation flag.
//! - `deactivated`: train has been cancelled or has departed — triggers Terminal state.
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
use quick_xml::{events::Event, Reader};
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
    /// Parsed content of a Darwin `Association` message (category `NP` only).
    Association {
        /// RID of the incoming (previous working) service.
        prev_rid: String,
        /// RID of the outgoing (next part) service being formed from the same stock.
        next_rid: String,
    },
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
}

/// Parsed content of a Darwin `deactivated` message.
#[derive(Debug, Clone)]
pub struct DeactivatedUpdate {
    pub rid: TrainId,
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

    // Parser state: we track whether we're inside a TS block.
    let mut current_ts: Option<TsUpdate> = None;

    // Parser state for Association messages (category NP only).
    let mut current_assoc_category: Option<String> = None;
    let mut current_assoc_main_rid: Option<String>  = None;
    let mut current_assoc_next_rid: Option<String>  = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local = e.local_name();

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
                        current_ts = Some(TsUpdate {
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
                        });
                    }
                    b"Location" if current_ts.is_some() => {
                        if let Some(ref mut ts) = current_ts {
                            let tpl = attr_opt(e, b"tpl");
                            if ts.scheduled_departure.is_none() {
                                // Use `ptd` (public timetable departure) from the first location.
                                if let Some(ptd) = attr_opt(e, b"ptd") {
                                    ts.scheduled_departure =
                                        parse_hhmm_on_date(&ptd, ts.ssd).ok();
                                }
                                if let Some(wtd) = attr_opt(e, b"wtd") {
                                    ts.working_departure =
                                        parse_hhmm_on_date(&wtd, ts.ssd).ok();
                                }
                                ts.platform = attr_opt(e, b"plat");
                                ts.station_crs = tpl.clone();
                            }
                            // Always update destination_crs to the most recently seen Location
                            // `tpl`. After the loop this will be the last (destination) stop.
                            if tpl.is_some() {
                                ts.destination_crs = tpl;
                            }
                        }
                    }
                    b"dep" if current_ts.is_some() => {
                        if let Some(ref mut ts) = current_ts {
                            if let Some(et) = attr_opt(e, b"et") {
                                ts.estimated_departure = parse_hhmm_on_date(&et, ts.ssd).ok();
                            }
                            if let Some(at) = attr_opt(e, b"at") {
                                ts.actual_departure = parse_hhmm_on_date(&at, ts.ssd).ok();
                            }
                            ts.is_delayed = attr_bool(e, b"delayed");
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

            Ok(Event::End(ref e)) => {
                match e.local_name().as_ref() {
                    b"TS" => {
                        if let Some(ts) = current_ts.take() {
                            updates.push(ParsedUpdate::TrainStatus(ts));
                        }
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

fn extract_pport_ts(
    e: &quick_xml::events::BytesStart,
) -> Result<DateTime<Utc>, ParseError> {
    let ts_str = attr_str(e, b"ts", "Pport", "ts")?;
    DateTime::parse_from_rfc3339(&ts_str)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| ParseError::InvalidTimestamp(ts_str))
}

fn attr_str(
    e: &quick_xml::events::BytesStart,
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

fn attr_opt(e: &quick_xml::events::BytesStart, name: &[u8]) -> Option<String> {
    e.attributes()
        .filter_map(|a| a.ok())
        .find(|a| a.key.local_name().as_ref() == name)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn attr_bool(e: &quick_xml::events::BytesStart, name: &[u8]) -> bool {
    attr_opt(e, name).is_some_and(|v| v == "true")
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
}
