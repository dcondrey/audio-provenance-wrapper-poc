use crate::error::{CoreError, Result};

/// The four provenance verification outcomes. This vocabulary is closed.
///
/// IMPORTANT: [`VerificationState::NothingFound`] records the absence of
/// provenance data. It is NEVER evidence that an asset is synthetic,
/// machine-generated, or untrustworthy. A caller that renders `NothingFound`
/// as a synthetic-origin claim is misusing it. See
/// [`NOTHING_FOUND_NORMATIVE_NOTE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    Verified,
    RegisteredButChanged,
    MarkFoundClaimNotTrusted,
    NothingFound,
}

pub const NOTHING_FOUND_NORMATIVE_NOTE: &str =
    "A missing mark is not proof of synthetic origin. NOTHING_FOUND records the \
     absence of provenance data, not the presence of a generation signal.";

impl VerificationState {
    pub const ALL: [VerificationState; 4] = [
        VerificationState::Verified,
        VerificationState::RegisteredButChanged,
        VerificationState::MarkFoundClaimNotTrusted,
        VerificationState::NothingFound,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            VerificationState::Verified => "verified",
            VerificationState::RegisteredButChanged => "registered_but_changed",
            VerificationState::MarkFoundClaimNotTrusted => "mark_found_claim_not_trusted",
            VerificationState::NothingFound => "nothing_found",
        }
    }

    pub fn parse(value: &str) -> Result<VerificationState> {
        VerificationState::ALL
            .into_iter()
            .find(|state| state.as_str() == value)
            .ok_or_else(|| CoreError::InvalidEnumValue {
                field: "verification_state",
                value: value.to_owned(),
            })
    }
}

impl core::fmt::Display for VerificationState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The local POC manifest verifier's own outcome vocabulary. Distinct from
/// [`VerificationState`]: it grades one local manifest file, not a registry record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalOutcome {
    Verified,
    Changed,
    Untrusted,
    NotFound,
    Incomplete,
}

impl LocalOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            LocalOutcome::Verified => "verified",
            LocalOutcome::Changed => "changed",
            LocalOutcome::Untrusted => "untrusted",
            LocalOutcome::NotFound => "not_found",
            LocalOutcome::Incomplete => "incomplete",
        }
    }
}

impl core::fmt::Display for LocalOutcome {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}
