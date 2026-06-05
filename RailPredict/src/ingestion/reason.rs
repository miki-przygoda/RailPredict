//! Darwin late/cancel reason-code classification.
//!
//! Darwin `TS` / `schedule` messages can carry a numeric reason code for why a service
//! is late or cancelled. The meaningful axis for prediction is **Structural vs Exogenous**:
//!
//! - **Structural** causes (congestion, regulation, awaiting a platform, connections) are
//!   recurring/operational — the model *can* learn them from history.
//! - **Exogenous** causes (trespass, broken rail, a person hit, severe weather) are one-off
//!   external shocks the model could never have predicted.
//!
//! Splitting accuracy by this class turns a flat "within ±5 min %" into an honest, decomposed
//! number (earned vs coincidental). See `docs/reason-code-feature.md`.
//!
//! ## Reference status
//! The code → (text, class) map below is an **illustrative subset**. Replace it with the
//! official Darwin LateRunningReasons / CancellationReasons reference when wired in; keep the
//! Structural/Exogenous overlay as a small curated layer on top.

/// Whether a delay/cancel cause is recurring (learnable) or a one-off external shock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonClass {
    /// Recurring/operational — the model can learn it from history.
    Structural,
    /// One-off incident / external shock — not in the historical pattern.
    Exogenous,
}

impl ReasonClass {
    /// Stable numeric encoding for persistence (`journeys.reason_class`):
    /// `1` = structural, `2` = exogenous. (`0` is reserved for "unknown / no reason".)
    pub fn code(self) -> i16 {
        match self {
            ReasonClass::Structural => 1,
            ReasonClass::Exogenous => 2,
        }
    }
}

/// Human-readable text + class for a Darwin reason code.
pub struct ReasonInfo {
    pub text: &'static str,
    pub class: ReasonClass,
}

/// Look up the illustrative text + class for a reason code. `None` for codes not in the
/// curated subset (treated as unknown — class `0` for persistence).
pub fn reason_info(code: i32) -> Option<ReasonInfo> {
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

/// Persistence encoding for a (possibly absent) reason code:
/// `0` = unknown / no reason, `1` = structural, `2` = exogenous.
pub fn reason_class_code(code: Option<i32>) -> i16 {
    code.and_then(reason_info).map(|r| r.class.code()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_code_classified() {
        assert_eq!(reason_info(168).unwrap().class, ReasonClass::Structural);
        assert_eq!(reason_class_code(Some(168)), 1);
    }

    #[test]
    fn exogenous_code_classified() {
        assert_eq!(reason_info(131).unwrap().class, ReasonClass::Exogenous);
        assert_eq!(reason_class_code(Some(131)), 2);
    }

    #[test]
    fn unknown_code_is_zero() {
        assert!(reason_info(99999).is_none());
        assert_eq!(reason_class_code(Some(99999)), 0);
        assert_eq!(reason_class_code(None), 0);
    }
}
