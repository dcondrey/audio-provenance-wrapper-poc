//! Caller errors only.
//!
//! Every provenance condition, a registry outage included, resolves to one of the four statuses.
//! Nothing here is reachable from a file that merely fails to verify: these are unreadable inputs,
//! budgets exceeded, and malformed options.

use audio_provenance_core::CodedError;

#[derive(Debug, thiserror::Error)]
pub enum GenoTraceError {
    #[error("could not read {path}")]
    Unreadable {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("input is {found} bytes, over the {limit}-byte budget")]
    InputTooLarge { found: u64, limit: u64 },
    #[error("input decodes to {found:.3} s, over the {limit:.3} s budget")]
    DurationTooLong { found: f64, limit: f64 },
    #[error("input is not a recognised audio container")]
    UnrecognisedContainer,
    #[error("input could not be decoded: {reason}")]
    Undecodable { reason: String },
    #[error("option {option} is invalid: {reason}")]
    InvalidOption {
        option: &'static str,
        reason: String,
    },
    #[error("null-test report is malformed: {reason}")]
    NullTestMalformed { reason: String },
    #[error("null-test report describes a bench fixture codec, not a shipping algorithm")]
    NullTestFixtureCodec,
    #[error("null-test report records no false-positive trials")]
    NullTestNotRun,
    #[error("fingerprint index is malformed: {reason}")]
    FingerprintIndexMalformed { reason: String },
}

impl CodedError for GenoTraceError {
    fn code(&self) -> &'static str {
        match self {
            Self::Unreadable { .. } => "input_unreadable",
            Self::InputTooLarge { .. } => "input_too_large",
            Self::DurationTooLong { .. } => "input_duration_too_long",
            Self::UnrecognisedContainer => "input_container_unrecognised",
            Self::Undecodable { .. } => "input_undecodable",
            Self::InvalidOption { .. } => "option_invalid",
            Self::NullTestMalformed { .. } => "null_test_malformed",
            Self::NullTestFixtureCodec => "null_test_fixture_codec",
            Self::NullTestNotRun => "null_test_not_run",
            Self::FingerprintIndexMalformed { .. } => "fingerprint_index_malformed",
        }
    }
}
