use crate::error::{CoreError, Result};

/// IMPORTANT: every claim-bearing object in a manifest carries exactly one of
/// these. There is no "absent" variant; omission is `UnknownUnobserved`.
///
/// No `Ord` derive: the only ordering is [`ProofLevel::rank`], so its "not a
/// general trust ranking" caveat cannot be bypassed by an unattached `max()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofLevel {
    UnknownUnobserved,
    UserDeclared,
    Inferred,
    DirectlyObserved,
    ExternallyVerified,
}

pub const PROOF_LEVEL_KEY: &str = "apw:proof_level";

impl ProofLevel {
    pub const ALL: [ProofLevel; 5] = [
        ProofLevel::UnknownUnobserved,
        ProofLevel::UserDeclared,
        ProofLevel::Inferred,
        ProofLevel::DirectlyObserved,
        ProofLevel::ExternallyVerified,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ProofLevel::UnknownUnobserved => "unknown_unobserved",
            ProofLevel::UserDeclared => "user_declared",
            ProofLevel::Inferred => "inferred",
            ProofLevel::DirectlyObserved => "directly_observed",
            ProofLevel::ExternallyVerified => "externally_verified",
        }
    }

    pub fn parse(value: &str) -> Result<ProofLevel> {
        ProofLevel::ALL
            .into_iter()
            .find(|level| level.as_str() == value)
            .ok_or_else(|| CoreError::InvalidProofLevel {
                value: value.to_owned(),
            })
    }

    /// Evidence-strength order for network cap comparisons only; NOT a trust ranking.
    pub fn rank(self) -> u8 {
        match self {
            ProofLevel::UnknownUnobserved => 0,
            ProofLevel::UserDeclared => 1,
            ProofLevel::Inferred => 2,
            ProofLevel::DirectlyObserved => 3,
            ProofLevel::ExternallyVerified => 4,
        }
    }

    pub fn capped_at(self, cap: ProofLevel) -> ProofLevel {
        if self.rank() > cap.rank() {
            cap
        } else {
            self
        }
    }
}

impl core::str::FromStr for ProofLevel {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<ProofLevel> {
        ProofLevel::parse(value)
    }
}

impl core::fmt::Display for ProofLevel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}
