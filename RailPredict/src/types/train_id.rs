//! `TrainId` — the canonical identifier enum for a UK rail service.
//!
//! Darwin messages use different identifier types per message family:
//!   - Schedule / TS messages   → RID (15-char, e.g. `202404170123456`)
//!   - Activation messages      → UID (6-char alphanumeric, e.g. `C12345`) + RID
//!   - RTT / display boards     → Headcode (4-char, e.g. `1A23`)
//!
//! All internal lookups go through `TrainId`. Never pass raw strings across module boundaries.

use thiserror::Error;

/// A UK rail service identifier. Three formats exist in Darwin; all are supported.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TrainId {
    /// Darwin Reference ID — 15-character string with date prefix (e.g. `202404170123456`).
    /// Primary key in schedule and train-status (TS) messages.
    Rid(String),

    /// RTTI UID — 6-character alphanumeric (e.g. `C12345`).
    /// Used in activation and association messages.
    Uid(String),

    /// Headcode / reporting number — 4-character format: digit + letter + 2 digits (e.g. `1A23`).
    /// Used on display boards and in some RTT feeds; least precise identifier.
    Headcode(String),
}

#[derive(Debug, Error)]
pub enum TrainIdError {
    #[error("RID must be exactly 15 characters, got {0}")]
    InvalidRidLength(usize),

    #[error("UID must be exactly 6 characters, got {0}")]
    InvalidUidLength(usize),

    #[error("Headcode must be exactly 4 characters, got {0}")]
    InvalidHeadcodeLength(usize),

    #[error("Headcode must start with a letter followed by 3 digits, got '{0}'")]
    InvalidHeadcodeFormat(String),
}

impl TrainId {
    pub fn rid(s: impl Into<String>) -> Result<Self, TrainIdError> {
        let s = s.into();
        if s.len() != 15 {
            return Err(TrainIdError::InvalidRidLength(s.len()));
        }
        Ok(Self::Rid(s))
    }

    pub fn uid(s: impl Into<String>) -> Result<Self, TrainIdError> {
        let s = s.into();
        if s.len() != 6 {
            return Err(TrainIdError::InvalidUidLength(s.len()));
        }
        Ok(Self::Uid(s))
    }

    pub fn headcode(s: impl Into<String>) -> Result<Self, TrainIdError> {
        let s = s.into();
        if s.len() != 4 {
            return Err(TrainIdError::InvalidHeadcodeLength(s.len()));
        }
        // Format: digit + letter + digit + digit (e.g. "1A23")
        let bytes = s.as_bytes();
        if !bytes[0].is_ascii_digit()
            || !bytes[1].is_ascii_alphabetic()
            || !bytes[2].is_ascii_digit()
            || !bytes[3].is_ascii_digit()
        {
            return Err(TrainIdError::InvalidHeadcodeFormat(s));
        }
        Ok(Self::Headcode(s))
    }

    /// Returns the raw identifier string regardless of variant.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Rid(s) | Self::Uid(s) | Self::Headcode(s) => s,
        }
    }
}

impl std::fmt::Display for TrainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rid(s) => write!(f, "RID:{s}"),
            Self::Uid(s) => write!(f, "UID:{s}"),
            Self::Headcode(s) => write!(f, "HC:{s}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rid_valid() {
        let id = TrainId::rid("202404170123456").unwrap();
        assert_eq!(id.as_str(), "202404170123456");
    }

    #[test]
    fn rid_wrong_length_rejected() {
        assert!(TrainId::rid("short").is_err());
    }

    #[test]
    fn uid_valid() {
        let id = TrainId::uid("C12345").unwrap();
        assert_eq!(id.as_str(), "C12345");
    }

    #[test]
    fn uid_wrong_length_rejected() {
        assert!(TrainId::uid("TOOLONG7").is_err());
    }

    #[test]
    fn headcode_valid() {
        let id = TrainId::headcode("1A23").unwrap();
        assert_eq!(id.as_str(), "1A23");
    }

    #[test]
    fn headcode_wrong_format_rejected() {
        // Two leading letters — invalid (must be digit-letter-digit-digit)
        assert!(TrainId::headcode("AA23").is_err());
    }

    #[test]
    fn headcode_wrong_length_rejected() {
        assert!(TrainId::headcode("1A2").is_err());
    }

    #[test]
    fn display_format() {
        assert_eq!(TrainId::rid("202404170123456").unwrap().to_string(), "RID:202404170123456");
        assert_eq!(TrainId::headcode("1A23").unwrap().to_string(), "HC:1A23");
    }

    #[test]
    fn uid_display_format() {
        assert_eq!(TrainId::uid("C12345").unwrap().to_string(), "UID:C12345");
    }

    #[test]
    fn headcode_all_digits_rejected() {
        // "1234" — second char is a digit, not a letter
        assert!(TrainId::headcode("1234").is_err());
    }

    #[test]
    fn headcode_letter_first_rejected() {
        // "A123" — first char must be a digit
        assert!(TrainId::headcode("A123").is_err());
    }

    #[test]
    fn headcode_letter_in_third_position_rejected() {
        // "1AB3" — third char must be a digit
        assert!(TrainId::headcode("1AB3").is_err());
    }

    #[test]
    fn same_variant_same_string_are_equal() {
        let a = TrainId::rid("202404170123456").unwrap();
        let b = TrainId::rid("202404170123456").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn different_variants_same_raw_string_are_not_equal() {
        // Uid and Headcode have different lengths so we need strings that fit each type.
        let uid = TrainId::uid("C12345").unwrap();
        let rid = TrainId::rid("202404170123456").unwrap();
        assert_ne!(uid, rid);
    }

    #[test]
    fn as_str_returns_inner_value_for_all_variants() {
        assert_eq!(TrainId::rid("202404170123456").unwrap().as_str(), "202404170123456");
        assert_eq!(TrainId::uid("C12345").unwrap().as_str(), "C12345");
        assert_eq!(TrainId::headcode("1A23").unwrap().as_str(), "1A23");
    }
}
