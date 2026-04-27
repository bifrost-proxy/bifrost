//! Keep-awake `Mode` and its string (de)serialization.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// Keep-awake working mode.
///
/// State-machine transitions are enforced by [`crate::KeepAwakeManager`];
/// this enum itself is intentionally dumb (just a tagged value).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Never acquire an assertion.
    Off,

    /// Acquire on the first incoming remote call; keep it held until the user
    /// releases it or the process exits. On the next process start the
    /// manager returns to the "inactive, waiting for a call" state.
    #[default]
    Auto,

    /// Acquire at startup, keep it held for the lifetime of the process
    /// (unless explicitly released via the `off` one-shot override).
    ForceOn,
}

impl Mode {
    /// Stable snake_case representation used on the wire and in CLI output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::ForceOn => "force_on",
        }
    }

    /// Canonical list used for CLI `value_parser` and docs.
    pub fn all() -> [Mode; 3] {
        [Mode::Off, Mode::Auto, Mode::ForceOn]
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid keepawake mode '{0}', expected one of: off, auto, force_on")]
pub struct ParseModeError(pub String);

impl FromStr for Mode {
    type Err = ParseModeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Accept both snake_case and dash-case for user friendliness.
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "off" => Ok(Mode::Off),
            "auto" => Ok(Mode::Auto),
            "force_on" | "forceon" | "on" => Ok(Mode::ForceOn),
            _ => Err(ParseModeError(s.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_parse_roundtrip() {
        for m in Mode::all() {
            let s = m.to_string();
            assert_eq!(Mode::from_str(&s).unwrap(), m, "roundtrip {s}");
        }
    }

    #[test]
    fn parse_accepts_aliases() {
        assert_eq!(Mode::from_str("Force-On").unwrap(), Mode::ForceOn);
        assert_eq!(Mode::from_str("FORCE_ON").unwrap(), Mode::ForceOn);
        assert_eq!(Mode::from_str("on").unwrap(), Mode::ForceOn);
        assert_eq!(Mode::from_str(" auto ").unwrap(), Mode::Auto);
    }

    #[test]
    fn parse_rejects_unknown() {
        let err = Mode::from_str("bogus").unwrap_err();
        assert!(err.to_string().contains("bogus"));
    }

    #[test]
    fn serde_uses_snake_case() {
        let json = serde_json::to_string(&Mode::ForceOn).unwrap();
        assert_eq!(json, "\"force_on\"");
        let parsed: Mode = serde_json::from_str("\"auto\"").unwrap();
        assert_eq!(parsed, Mode::Auto);
    }

    #[test]
    fn default_is_auto() {
        assert_eq!(Mode::default(), Mode::Auto);
    }
}
