use serde::{Deserialize, Serialize};

use crate::error::VocabularyError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofLevel {
    DirectlyObserved,
    Inferred,
    UserDeclared,
    ExternallyVerified,
    UnknownUnobserved,
}

impl ProofLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DirectlyObserved => "directly_observed",
            Self::Inferred => "inferred",
            Self::UserDeclared => "user_declared",
            Self::ExternallyVerified => "externally_verified",
            Self::UnknownUnobserved => "unknown_unobserved",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Verified,
    Changed,
    Untrusted,
    NotFound,
}

impl VerificationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Changed => "changed",
            Self::Untrusted => "untrusted",
            Self::NotFound => "not_found",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    CompleteObservedPath,
    PartialObservedPath,
    UnknownCoverage,
}

impl CoverageStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CompleteObservedPath => "complete_observed_path",
            Self::PartialObservedPath => "partial_observed_path",
            Self::UnknownCoverage => "unknown_coverage",
        }
    }

    const fn required_proof_level(self) -> ProofLevel {
        match self {
            Self::CompleteObservedPath | Self::PartialObservedPath => ProofLevel::Inferred,
            Self::UnknownCoverage => ProofLevel::UnknownUnobserved,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssociationStatus {
    InferredMatch,
    NotEstablished,
    Unavailable,
}

impl AssociationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InferredMatch => "inferred_match",
            Self::NotEstablished => "not_established",
            Self::Unavailable => "unavailable",
        }
    }

    /// IMPORTANT: `daemon/schema.py` refuses any other pairing. An established association is
    /// evidence of similarity, never of observation, so it can never rise above `inferred`.
    const fn required_proof_level(self) -> ProofLevel {
        match self {
            Self::InferredMatch => ProofLevel::Inferred,
            Self::NotEstablished | Self::Unavailable => ProofLevel::UnknownUnobserved,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(into = "AssociationClaimWire")]
pub struct AssociationClaim {
    status: AssociationStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct AssociationClaimWire {
    status: AssociationStatus,
    #[serde(rename = "apw:proof_level")]
    proof_level: ProofLevel,
}

impl From<AssociationClaim> for AssociationClaimWire {
    fn from(claim: AssociationClaim) -> Self {
        Self {
            status: claim.status,
            proof_level: claim.proof_level(),
        }
    }
}

impl TryFrom<AssociationClaimWire> for AssociationClaim {
    type Error = VocabularyError;

    fn try_from(wire: AssociationClaimWire) -> Result<Self, Self::Error> {
        let claim = Self::new(wire.status);
        if wire.proof_level != claim.proof_level() {
            return Err(match wire.status {
                AssociationStatus::InferredMatch => VocabularyError::AssociationOverstated {
                    found: wire.proof_level.as_str(),
                },
                AssociationStatus::NotEstablished | AssociationStatus::Unavailable => {
                    VocabularyError::AssociationUnderstated {
                        found: wire.proof_level.as_str(),
                    }
                }
            });
        }
        Ok(claim)
    }
}

impl<'de> Deserialize<'de> for AssociationClaim {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = AssociationClaimWire::deserialize(deserializer)?;
        Self::try_from(wire).map_err(serde::de::Error::custom)
    }
}

impl AssociationClaim {
    pub const fn new(status: AssociationStatus) -> Self {
        Self { status }
    }

    pub const fn established() -> Self {
        Self::new(AssociationStatus::InferredMatch)
    }

    pub const fn not_established() -> Self {
        Self::new(AssociationStatus::NotEstablished)
    }

    pub const fn unavailable() -> Self {
        Self::new(AssociationStatus::Unavailable)
    }

    pub const fn status(&self) -> AssociationStatus {
        self.status
    }

    pub const fn proof_level(&self) -> ProofLevel {
        self.status.required_proof_level()
    }
}

/// The counters `daemon/schema.py` reads when it decides whether a session may claim
/// `complete_observed_path`. `midi_events_dropped` and `stream_evictions` default to zero because
/// the Python reads them with `.get(key, 0)` while requiring the rest to be present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationCounters {
    pub windows_hashed: u64,
    pub buffer_hash_events_received: u64,
    pub fifo_samples_dropped: u64,
    pub fifo_windows_dropped: u64,
    pub udp_sends_failed: u64,
    pub sequence_gaps: u64,
    pub hash_chain_breaks: u64,
    pub events_prepared: u64,
    pub events_received: u64,
    pub daemon_acknowledgements_sent: u64,
    pub daemon_acknowledgements_failed: u64,
    pub packets_received: u64,
    /// `None` identifies a pre-lifecycle-telemetry manifest. Such a
    /// manifest cannot exclude host bypass and therefore cannot prove complete
    /// observation coverage.
    #[serde(default)]
    pub bypassed_buffers: Option<u64>,
    #[serde(default)]
    pub plugin_telemetry_regressions: u64,
    #[serde(default)]
    pub midi_events_dropped: u64,
    #[serde(default)]
    pub stream_evictions: u64,
}

impl ObservationCounters {
    /// IMPORTANT: a conjunction of exact equalities and zero-valued loss counters, never a
    /// threshold or a heuristic. Any relaxation here silently promotes a partial session.
    pub const fn proves_complete_observed_path(&self) -> bool {
        self.windows_hashed == self.buffer_hash_events_received
            && self.events_prepared == self.events_received
            && self.daemon_acknowledgements_sent == self.packets_received
            && self.fifo_samples_dropped == 0
            && self.fifo_windows_dropped == 0
            && self.midi_events_dropped == 0
            && match self.bypassed_buffers {
                Some(value) => value == 0,
                None => false,
            }
            && self.udp_sends_failed == 0
            && self.sequence_gaps == 0
            && self.hash_chain_breaks == 0
            && self.stream_evictions == 0
            && self.daemon_acknowledgements_failed == 0
            && self.plugin_telemetry_regressions == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(into = "ObservationCoverageWire")]
pub struct ObservationCoverage {
    counters: Option<ObservationCounters>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct ObservationCoverageWire {
    status: CoverageStatus,
    #[serde(rename = "apw:proof_level")]
    proof_level: ProofLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    counters: Option<ObservationCounters>,
}

impl From<ObservationCoverage> for ObservationCoverageWire {
    fn from(coverage: ObservationCoverage) -> Self {
        Self {
            status: coverage.status(),
            proof_level: coverage.proof_level(),
            counters: coverage.counters,
        }
    }
}

impl TryFrom<ObservationCoverageWire> for ObservationCoverage {
    type Error = VocabularyError;

    fn try_from(wire: ObservationCoverageWire) -> Result<Self, Self::Error> {
        let coverage = match wire.counters {
            Some(counters) => Self::from_counters(counters),
            None => {
                if wire.status == CoverageStatus::CompleteObservedPath {
                    return Err(VocabularyError::CoverageCountersMissing);
                }
                Self { counters: None }
            }
        };
        let derived = coverage.status();
        if wire.status != derived {
            return Err(VocabularyError::CoverageOverstated {
                declared: wire.status.as_str(),
                derived: derived.as_str(),
            });
        }
        let expected = derived.required_proof_level();
        if wire.proof_level != expected {
            return Err(VocabularyError::CoverageProofLevelMismatch {
                status: derived.as_str(),
                expected: expected.as_str(),
                found: wire.proof_level.as_str(),
            });
        }
        Ok(coverage)
    }
}

impl<'de> Deserialize<'de> for ObservationCoverage {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ObservationCoverageWire::deserialize(deserializer)?;
        Self::try_from(wire).map_err(serde::de::Error::custom)
    }
}

impl ObservationCoverage {
    pub const fn from_counters(counters: ObservationCounters) -> Self {
        Self {
            counters: Some(counters),
        }
    }

    pub const fn unknown() -> Self {
        Self { counters: None }
    }

    pub const fn counters(&self) -> Option<&ObservationCounters> {
        self.counters.as_ref()
    }

    pub const fn status(&self) -> CoverageStatus {
        match self.counters {
            Some(counters) => {
                if counters.proves_complete_observed_path() {
                    CoverageStatus::CompleteObservedPath
                } else {
                    CoverageStatus::PartialObservedPath
                }
            }
            None => CoverageStatus::UnknownCoverage,
        }
    }

    pub const fn proof_level(&self) -> ProofLevel {
        self.status().required_proof_level()
    }
}
