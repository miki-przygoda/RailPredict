//! STANDALONE PROTOTYPE — Darwin reason-code parsing + "earned vs coincidental"
//! prediction-accuracy analysis. NOT wired into the app; lives in `tests/` only so it
//! compiles + runs via `cargo test` without touching the running build.
//!
//! Run the readable report:
//!   cargo test --test reason_code_prototype -- --nocapture
//!
//! ## The idea
//! Darwin `TS`/`schedule` messages can carry a numeric **reason code** for why a service
//! is late or cancelled (signalling, trespass, weather, congestion, …). We don't parse
//! these today (see `docs/darwin-data-audit.md`). This prototype (1) parses them and (2)
//! uses them to interrogate our *accurate* predictions.
//!
//! A prediction "within ±5 min" looks good — but **why** was it accurate?
//!  - If the delay's cause is **structural/recurring** (congestion, awaiting platform,
//!    regulation), the model can *learn* it from history → the hit is **earned**.
//!  - If the cause is **exogenous/one-off** (trespass, broken rail, a person hit by a
//!    train, severe weather), the model could NOT have known → an in-tolerance hit is
//!    **coincidental** (lucky), and a big miss is **explained** (not the model's fault).
//!
//! Splitting our within-±5 hits into earned vs coincidental tells us whether our accuracy
//! is *robust* (we're modelling real causes) or *fragile* (we're getting lucky on
//! incidents we never modelled). See `docs/reason-code-feature.md` for the feature plan.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

// ---------------------------------------------------------------------------
// Reason reference (ILLUSTRATIVE subset — replace with the official Darwin
// LateRunningReasons / CancellationReasons reference when this is wired in).
// The codes/text below are representative; the structural-vs-exogenous CLASS is
// the meaningful axis for this prototype.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReasonClass {
    /// Recurring/operational — the model can learn it from history.
    Structural,
    /// One-off incident / external shock — not in the historical pattern.
    Exogenous,
}

struct ReasonInfo {
    #[allow(dead_code)] // surfaced in the printed report / future UI, not asserted on
    text: &'static str,
    class: ReasonClass,
}

fn reason_info(code: i32) -> Option<ReasonInfo> {
    use ReasonClass::*;
    let info = |text, class| ReasonInfo { text, class };
    Some(match code {
        // --- Structural / recurring ---
        163 => info("the train running late being regulated", Structural),
        166 => info("awaiting an available platform", Structural),
        168 => info("congestion caused by service disruption ahead", Structural),
        148 => info("waiting for a connecting service", Structural),
        171 => info("a longer-than-usual station stop (boarding)", Structural),
        // --- Exogenous / one-off incidents ---
        100 => info("a broken-down train", Exogenous),
        131 => info("trespassers on the railway", Exogenous),
        134 => info("a person being hit by a train", Exogenous),
        180 => info("a broken rail", Exogenous),
        185 => info("severe weather", Exogenous),
        139 => info("a fault on a level crossing", Exogenous),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Reason-code parser — extracts late/cancel reasons from a Darwin pport fragment.
// Handles the common shapes:
//   <LateReason tiploc="X" near="true">163</LateReason>   (code as text)
//   <CancelReason code="100"/>                            (code as attribute)
//   <schedule ... delayReason="168" cancelReason="100">   (codes as attributes)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReasonKind {
    Late,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedReason {
    kind: ReasonKind,
    code: i32,
    tiploc: Option<String>,
    near: bool,
}

fn attr(e: &BytesStart, name: &[u8]) -> Option<String> {
    e.attributes()
        .filter_map(|a| a.ok())
        .find(|a| a.key.local_name().as_ref() == name)
        .map(|a| String::from_utf8_lossy(&a.value).into_owned())
}

fn reason_kind(e: &BytesStart) -> Option<ReasonKind> {
    match e.local_name().as_ref() {
        b"LateReason" => Some(ReasonKind::Late),
        b"CancelReason" => Some(ReasonKind::Cancel),
        _ => None,
    }
}

/// Emit any attribute-form reasons (`delayReason` / `cancelReason`) on this element.
fn push_attr_reasons(e: &BytesStart, out: &mut Vec<ParsedReason>) {
    if let Some(code) = attr(e, b"delayReason").and_then(|v| v.parse().ok()) {
        out.push(ParsedReason { kind: ReasonKind::Late, code, tiploc: attr(e, b"delayReasonTiploc"), near: false });
    }
    if let Some(code) = attr(e, b"cancelReason").and_then(|v| v.parse().ok()) {
        out.push(ParsedReason { kind: ReasonKind::Cancel, code, tiploc: attr(e, b"cancelReasonTiploc"), near: false });
    }
}

fn parse_reasons(xml: &str) -> Vec<ParsedReason> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut out: Vec<ParsedReason> = Vec::new();
    // element-form reason awaiting its text code: (kind, code?, tiploc, near)
    let mut pending: Option<(ReasonKind, Option<i32>, Option<String>, bool)> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Empty(ref e)) => {
                push_attr_reasons(e, &mut out);
                // Self-closing reason element — code must be in the `code` attribute.
                if let Some(kind) = reason_kind(e)
                    && let Some(code) = attr(e, b"code").and_then(|v| v.parse().ok())
                {
                    out.push(ParsedReason { kind, code, tiploc: attr(e, b"tiploc"), near: attr(e, b"near").as_deref() == Some("true") });
                }
            }
            Ok(Event::Start(ref e)) => {
                push_attr_reasons(e, &mut out);
                if let Some(kind) = reason_kind(e) {
                    let code = attr(e, b"code").and_then(|v| v.parse().ok());
                    pending = Some((kind, code, attr(e, b"tiploc"), attr(e, b"near").as_deref() == Some("true")));
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(p) = pending.as_mut()
                    && p.1.is_none()
                    && let Ok(s) = t.unescape()
                    && let Ok(code) = s.trim().parse::<i32>()
                {
                    p.1 = Some(code);
                }
            }
            Ok(Event::End(ref e)) => {
                if matches!(e.local_name().as_ref(), b"LateReason" | b"CancelReason")
                    && let Some((kind, Some(code), tiploc, near)) = pending.take()
                {
                    out.push(ParsedReason { kind, code, tiploc, near });
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The "earned vs coincidental" classifier.
// ---------------------------------------------------------------------------

const ACCURATE_MINS: i32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Within ±5 and the cause is structural — the model modelled the real cause.
    EarnedAccurate,
    /// Within ±5 but the cause is exogenous — the model couldn't have known; lucky.
    CoincidentalAccurate,
    /// Outside ±5 but the cause is exogenous — an understandable surprise, not a model fault.
    ExplainedMiss,
    /// Outside ±5 and the cause is structural — a genuine miss the model should have caught.
    ModelError,
    /// Within ±5 with no reason recorded (e.g. ran ~on time).
    CleanAccurate,
    /// Outside ±5 with no reason recorded.
    UnexplainedMiss,
}

fn classify(predicted: i32, actual: i32, reason: Option<ReasonClass>) -> (Verdict, &'static str) {
    let within = (actual - predicted).abs() <= ACCURATE_MINS;
    match (within, reason) {
        (true, Some(ReasonClass::Structural)) => {
            (Verdict::EarnedAccurate, "accurate AND the cause is recurring — robust hit")
        }
        (true, Some(ReasonClass::Exogenous)) => {
            (Verdict::CoincidentalAccurate, "accurate but the cause was a one-off incident — lucky")
        }
        (false, Some(ReasonClass::Exogenous)) => {
            (Verdict::ExplainedMiss, "missed, but an unmodellable incident explains it")
        }
        (false, Some(ReasonClass::Structural)) => {
            (Verdict::ModelError, "missed on a recurring cause — the model should improve here")
        }
        (true, None) => (Verdict::CleanAccurate, "accurate, no disruption reason on record"),
        (false, None) => (Verdict::UnexplainedMiss, "missed with no reason on record"),
    }
}

// ---------------------------------------------------------------------------
// Sample Darwin fragments
// ---------------------------------------------------------------------------

const TS_CONGESTION: &str = r#"<?xml version="1.0"?>
<Pport ts="2026-06-05T08:14:00Z" version="16.0">
  <uR>
    <TS rid="202606050000001" ssd="2026-06-05" uid="C12345">
      <LateReason tiploc="WATRLMN" near="true">168</LateReason>
      <Location tpl="WATRLMN" ptd="08:10"><dep et="08:19" delayed="true"/></Location>
    </TS>
  </uR>
</Pport>"#;

const TS_TRESPASS: &str = r#"<?xml version="1.0"?>
<Pport ts="2026-06-05T08:20:00Z" version="16.0">
  <uR>
    <TS rid="202606050000002" ssd="2026-06-05" uid="C22222">
      <LateReason>131</LateReason>
      <Location tpl="VICTRIC" ptd="08:15"><dep et="08:22" delayed="true"/></Location>
    </TS>
  </uR>
</Pport>"#;

const SCHED_CANCEL_ATTR: &str = r#"<?xml version="1.0"?>
<Pport ts="2026-06-05T08:00:00Z" version="16.0">
  <uR>
    <schedule rid="202606050000003" uid="C33333" cancelReason="100"/>
  </uR>
</Pport>"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn parses_late_reason_element_with_text_code() {
    let r = parse_reasons(TS_CONGESTION);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0], ParsedReason { kind: ReasonKind::Late, code: 168, tiploc: Some("WATRLMN".into()), near: true });
    assert_eq!(reason_info(168).unwrap().class, ReasonClass::Structural);
}

#[test]
fn parses_bare_late_reason() {
    let r = parse_reasons(TS_TRESPASS);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].code, 131);
    assert_eq!(reason_info(131).unwrap().class, ReasonClass::Exogenous);
}

#[test]
fn parses_cancel_reason_attribute_form() {
    let r = parse_reasons(SCHED_CANCEL_ATTR);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0], ParsedReason { kind: ReasonKind::Cancel, code: 100, tiploc: None, near: false });
}

#[test]
fn within5_structural_is_earned() {
    // model predicted +8 (it knows this service congests in the peak); congestion gave +9.
    assert_eq!(classify(8, 9, Some(ReasonClass::Structural)).0, Verdict::EarnedAccurate);
}

#[test]
fn within5_exogenous_is_coincidental() {
    // model predicted +6 from the historical pattern; a trespass incident gave +7.
    // Within ±5 — but the model could NOT have known about today's trespass: lucky.
    assert_eq!(classify(6, 7, Some(ReasonClass::Exogenous)).0, Verdict::CoincidentalAccurate);
}

#[test]
fn big_miss_with_exogenous_reason_is_explained_not_model_error() {
    // predicted +5, a broken rail caused +35 — a miss, but unmodellable.
    assert_eq!(classify(5, 35, Some(ReasonClass::Exogenous)).0, Verdict::ExplainedMiss);
}

#[test]
fn big_miss_with_structural_reason_is_a_real_model_error() {
    // predicted +2 but recurring congestion gave +20 — the model should have learned this.
    assert_eq!(classify(2, 20, Some(ReasonClass::Structural)).0, Verdict::ModelError);
}

/// The headline view: split within-±5 "accurate" predictions into earned vs coincidental.
/// Run with `-- --nocapture` to read it.
#[test]
fn report_earned_vs_coincidental() {
    // (predicted, actual, reason_code, label) — a mix of accurate + miss cases.
    let cases: &[(i32, i32, Option<i32>, &str)] = &[
        (8, 9, Some(168), "congestion"),
        (6, 7, Some(131), "trespass"),
        (3, 2, Some(166), "awaiting platform"),
        (10, 11, Some(185), "severe weather"),
        (1, 0, None, "ran on time"),
        (5, 35, Some(180), "broken rail"),
        (2, 20, Some(168), "congestion (missed)"),
    ];

    println!("\n  Reason-aware accuracy (|err| <= {ACCURATE_MINS} = accurate)\n");
    println!("  pred  act  err  reason                  verdict                why");
    println!("  ----  ---  ---  ----------------------  ---------------------  ---");
    let (mut earned, mut coincidental) = (0, 0);
    for &(p, a, code, label) in cases {
        let class = code.and_then(reason_info).map(|r| r.class);
        let (v, why) = classify(p, a, class);
        match v {
            Verdict::EarnedAccurate => earned += 1,
            Verdict::CoincidentalAccurate => coincidental += 1,
            _ => {}
        }
        let verdict = format!("{v:?}");
        println!("  {p:>4}  {a:>3}  {:>3}  {label:<22}  {verdict:<21}  {why}", (a - p).abs());
    }
    println!("\n  Of the accurate (<=5) hits: {earned} earned, {coincidental} coincidental.");
    println!("  -> 'coincidental' = the model got lucky on an incident it never modelled.\n");
    // Sanity: at least one of each surfaced by the sample set.
    assert!(earned >= 1 && coincidental >= 1);
}
