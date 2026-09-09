//! The status mapping: a total function from an explicit enumeration of evidence onto the four
//! provenance statuses.
//!
//! Everything in this module is a fieldless `Copy` enum and one `match` with no wildcard arm, so a
//! reviewer can read every case that exists and the compiler refuses a case that is missing. The
//! evidence-carrying types live in [`crate::result`] and reduce to these classes; nothing else in
//! the crate is allowed to decide a status.
//!
//! # Precedence, and why binding outranks trust
//!
//! Recovery, then signature, then binding, then trust. If an unresolved trust anchor outranked a
//! broken binding, `changed` would be reachable only for anchored signers, and the honest common
//! case is an unanchored one. Four statuses would collapse to three for nearly every user.
//!
//! # What a soft binding may and may not do
//!
//! A `single`-class Watermark detection and a bare fingerprint hit are both proof level `inferred`
//! and neither verifies: one CRC-passing block is as thin as a perceptual guess, and a fingerprint
//! selects a manifest rather than binding one.
//!
//! Two classes verify without an exact hard binding. A `strong` Watermark detection above the
//! coverage threshold, with a measured false-positive rate, substitutes for a hard binding that was
//! never declared. [`BindingClass::HardMismatchSoftAffirmed`] answers a different question about a
//! hard binding that WAS declared and failed: not "are these the same bytes", which is settled and
//! settled as no, but "is this the same work", which the failed digest cannot reach. It needs BOTH
//! a CRC-valid payload naming this record and the record's own signed reference constellation, so a
//! fingerprint still never verifies on its own, and it reports `inferred` at a `match` under 1.0 so
//! an inferred verification can never present itself as exact.

use audio_provenance_core::{ProofLevel, VerificationStatus};

/// Why the ladder stopped without a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Exhaustion {
    /// Every rung ran to completion and none produced a candidate.
    AllRungsRan,
    /// At least one rung could not run: a registry outage, a decode budget, an aborted search. The
    /// verdict is still `not_found`, but paired with `incomplete`, which is what keeps a network
    /// fault from reading as "unregistered".
    RungCouldNotRun,
}

/// A candidate that never reached binding evaluation because its own integrity failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectionReason {
    NoncanonicalManifest,
    LocatorMismatch,
    AmbiguousBinding,
    InvariantsFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignatureOutcome {
    Valid,
    Invalid,
    Absent,
    UnsupportedAlgorithm,
}

/// What the presented audio proved about the manifest's bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindingClass {
    /// The signed content digest recomputes over these exact file bytes.
    HardExactContent,
    /// The file bytes moved but the signed decoded-audio digest still recomputes: a container-only
    /// edit. Kept distinct from [`Self::HardExactContent`] so the fact is never lost.
    HardExactDecodedAudioOnly,
    /// A hard binding is present and disagrees, and nothing corroborates the audio as the same
    /// work. This is the real tampering case: an edit, a splice, or a mark lifted onto other audio.
    HardMismatch,
    /// A hard binding is present and disagrees, but a CRC-valid Watermark payload naming THIS record
    /// was recovered from the audio AND the record's own signed reference constellation affirms it
    /// is the same work. The bytes moved; the recording did not. Verifies at proof level
    /// `inferred` and never at `match` 1.0, so it can never be read as an exact-bytes result.
    HardMismatchSoftAffirmed,
    /// `strong` Watermark, coverage at or above threshold, with a measured false-positive rate.
    SoftMarkStrongAtOrAboveThreshold,
    SoftMarkStrongBelowThreshold,
    /// A single CRC-passing block. Proof level `inferred`, so it cannot verify on its own.
    SoftMarkInferredOnly,
    /// A soft binding that would otherwise verify, with no null-test row to price it. The remedy is
    /// running the bench, not re-recording the audio, so it gets its own reason code.
    SoftMarkFalsePositiveRateUnknown,
    /// Perceptual similarity only. Always `inferred`; no hard binding was recoverable.
    SoftFingerprintInferred,
    /// No hard binding declared and no soft evidence recovered.
    NoBindingEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrustOutcome {
    /// The signer resolved through a configured anchor to a name.
    Anchored,
    /// No anchor covers this signer. A valid self-generated signature lands here.
    Unanchored,
    /// An anchor covers this signer and rejected it.
    Failed,
}

/// The complete evidence the status mapping consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusInputs {
    NoCandidate(Exhaustion),
    CandidateRejected(RejectionReason),
    Candidate {
        signature: SignatureOutcome,
        binding: BindingClass,
        trust: TrustOutcome,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusReason {
    NoManifestRecovered,
    RecoveryIncomplete,
    NoncanonicalManifest,
    LocatorMismatch,
    AmbiguousBinding,
    ManifestInvariantsFailed,
    SignatureInvalid,
    SignatureAbsent,
    SignatureAlgorithmUnsupported,
    HardBindingMatch,
    HardBindingDecodedAudioOnly,
    HardBindingMismatch,
    SoftBindingAffirmedSameWork,
    SoftBindingAccepted,
    SoftBindingCoverageLow,
    SoftBindingSingleBlockInferred,
    SoftBindingFalsePositiveRateUnknown,
    FingerprintAssociationInferred,
    NoBindingEvidence,
    TrustAnchorUnresolved,
    TrustAnchorRejected,
}

impl StatusReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoManifestRecovered => "no_manifest_recovered",
            Self::RecoveryIncomplete => "recovery_incomplete",
            Self::NoncanonicalManifest => "noncanonical_manifest",
            Self::LocatorMismatch => "locator_mismatch",
            Self::AmbiguousBinding => "ambiguous_binding",
            Self::ManifestInvariantsFailed => "manifest_invariants_failed",
            Self::SignatureInvalid => "signature_invalid",
            Self::SignatureAbsent => "signature_absent",
            Self::SignatureAlgorithmUnsupported => "signature_algorithm_unsupported",
            Self::HardBindingMatch => "hard_binding_match",
            Self::HardBindingDecodedAudioOnly => "hard_binding_decoded_audio_only",
            Self::HardBindingMismatch => "hard_binding_mismatch",
            Self::SoftBindingAffirmedSameWork => "soft_binding_affirmed_same_work",
            Self::SoftBindingAccepted => "soft_binding_accepted",
            Self::SoftBindingCoverageLow => "soft_binding_coverage_low",
            Self::SoftBindingSingleBlockInferred => "soft_binding_single_block_inferred",
            Self::SoftBindingFalsePositiveRateUnknown => "soft_binding_false_positive_rate_unknown",
            Self::FingerprintAssociationInferred => "fingerprint_association_inferred",
            Self::NoBindingEvidence => "no_binding_evidence",
            Self::TrustAnchorUnresolved => "trust_anchor_unresolved",
            Self::TrustAnchorRejected => "trust_anchor_rejected",
        }
    }
}

/// The single output of the mapping.
///
/// `identity_proof_level` is the one place the disclosure decision lives. `identity` is populated
/// from it and only from it, which is what makes SDK_SPEC's "non-null IFF the proof level is
/// `externally_verified`" hold by construction rather than by review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Verdict {
    status: VerificationStatus,
    reason: StatusReason,
    identity_proof_level: ProofLevel,
    incomplete: bool,
}

impl Verdict {
    pub const fn status(&self) -> VerificationStatus {
        self.status
    }

    pub const fn reason(&self) -> StatusReason {
        self.reason
    }

    pub const fn identity_proof_level(&self) -> ProofLevel {
        self.identity_proof_level
    }

    /// True exactly when a name may be reported.
    pub const fn discloses_identity(&self) -> bool {
        matches!(self.identity_proof_level, ProofLevel::ExternallyVerified)
    }

    pub const fn incomplete(&self) -> bool {
        self.incomplete
    }

    const fn new(status: VerificationStatus, reason: StatusReason, level: ProofLevel) -> Self {
        Self {
            status,
            reason,
            identity_proof_level: level,
            incomplete: false,
        }
    }
}

/// THE status mapping. Total, exhaustive, wildcard-free.
pub const fn derive_status(inputs: StatusInputs) -> Verdict {
    use BindingClass as B;
    use ProofLevel::UnknownUnobserved as Unobserved;
    use StatusReason as R;
    use TrustOutcome as T;
    use VerificationStatus::{Changed, NotFound, Untrusted, Verified};

    match inputs {
        StatusInputs::NoCandidate(Exhaustion::AllRungsRan) => {
            Verdict::new(NotFound, R::NoManifestRecovered, Unobserved)
        }
        // IMPORTANT: an outage is `not_found` PLUS `incomplete`, never `not_found` alone. The pair
        // is minted here so the two can never drift apart.
        StatusInputs::NoCandidate(Exhaustion::RungCouldNotRun) => Verdict {
            status: NotFound,
            reason: R::RecoveryIncomplete,
            identity_proof_level: Unobserved,
            incomplete: true,
        },

        StatusInputs::CandidateRejected(RejectionReason::NoncanonicalManifest) => {
            Verdict::new(Untrusted, R::NoncanonicalManifest, Unobserved)
        }
        StatusInputs::CandidateRejected(RejectionReason::LocatorMismatch) => {
            Verdict::new(Untrusted, R::LocatorMismatch, Unobserved)
        }
        StatusInputs::CandidateRejected(RejectionReason::AmbiguousBinding) => {
            Verdict::new(Untrusted, R::AmbiguousBinding, Unobserved)
        }
        StatusInputs::CandidateRejected(RejectionReason::InvariantsFailed) => {
            Verdict::new(Untrusted, R::ManifestInvariantsFailed, Unobserved)
        }

        StatusInputs::Candidate {
            signature: SignatureOutcome::Invalid,
            ..
        } => Verdict::new(Untrusted, R::SignatureInvalid, Unobserved),
        StatusInputs::Candidate {
            signature: SignatureOutcome::Absent,
            ..
        } => Verdict::new(Untrusted, R::SignatureAbsent, Unobserved),
        StatusInputs::Candidate {
            signature: SignatureOutcome::UnsupportedAlgorithm,
            ..
        } => Verdict::new(Untrusted, R::SignatureAlgorithmUnsupported, Unobserved),

        StatusInputs::Candidate {
            signature: SignatureOutcome::Valid,
            binding,
            trust,
        } => match (binding, trust) {
            (B::HardExactContent, T::Anchored) => Verdict::new(
                Verified,
                R::HardBindingMatch,
                ProofLevel::ExternallyVerified,
            ),
            (B::HardExactContent, T::Unanchored) => {
                Verdict::new(Untrusted, R::TrustAnchorUnresolved, Unobserved)
            }
            (B::HardExactContent, T::Failed) => {
                Verdict::new(Untrusted, R::TrustAnchorRejected, Unobserved)
            }

            (B::HardExactDecodedAudioOnly, T::Anchored) => Verdict::new(
                Verified,
                R::HardBindingDecodedAudioOnly,
                ProofLevel::ExternallyVerified,
            ),
            (B::HardExactDecodedAudioOnly, T::Unanchored) => {
                Verdict::new(Untrusted, R::TrustAnchorUnresolved, Unobserved)
            }
            (B::HardExactDecodedAudioOnly, T::Failed) => {
                Verdict::new(Untrusted, R::TrustAnchorRejected, Unobserved)
            }

            // `changed` is the one non-verified status that may still name a signer: a known studio
            // whose audio was altered is exactly what the row is for.
            (B::HardMismatch, T::Anchored) => Verdict::new(
                Changed,
                R::HardBindingMismatch,
                ProofLevel::ExternallyVerified,
            ),
            (B::HardMismatch, T::Unanchored | T::Failed) => {
                Verdict::new(Changed, R::HardBindingMismatch, Unobserved)
            }

            // Route 3(c). `inferred` here is the BINDING's proof level, carried on the evaluation;
            // the level in this verdict is about identity, which an anchor established.
            (B::HardMismatchSoftAffirmed, T::Anchored) => Verdict::new(
                Verified,
                R::SoftBindingAffirmedSameWork,
                ProofLevel::ExternallyVerified,
            ),
            (B::HardMismatchSoftAffirmed, T::Unanchored) => {
                Verdict::new(Untrusted, R::TrustAnchorUnresolved, Unobserved)
            }
            (B::HardMismatchSoftAffirmed, T::Failed) => {
                Verdict::new(Untrusted, R::TrustAnchorRejected, Unobserved)
            }

            (B::SoftMarkStrongAtOrAboveThreshold, T::Anchored) => Verdict::new(
                Verified,
                R::SoftBindingAccepted,
                ProofLevel::ExternallyVerified,
            ),
            (B::SoftMarkStrongAtOrAboveThreshold, T::Unanchored) => {
                Verdict::new(Untrusted, R::TrustAnchorUnresolved, Unobserved)
            }
            (B::SoftMarkStrongAtOrAboveThreshold, T::Failed) => {
                Verdict::new(Untrusted, R::TrustAnchorRejected, Unobserved)
            }

            (B::SoftMarkStrongBelowThreshold, T::Anchored | T::Unanchored | T::Failed) => {
                Verdict::new(Untrusted, R::SoftBindingCoverageLow, Unobserved)
            }
            (B::SoftMarkInferredOnly, T::Anchored | T::Unanchored | T::Failed) => {
                Verdict::new(Untrusted, R::SoftBindingSingleBlockInferred, Unobserved)
            }
            (B::SoftMarkFalsePositiveRateUnknown, T::Anchored | T::Unanchored | T::Failed) => {
                Verdict::new(
                    Untrusted,
                    R::SoftBindingFalsePositiveRateUnknown,
                    Unobserved,
                )
            }
            (B::SoftFingerprintInferred, T::Anchored | T::Unanchored | T::Failed) => {
                Verdict::new(Untrusted, R::FingerprintAssociationInferred, Unobserved)
            }

            (B::NoBindingEvidence, T::Anchored) => Verdict::new(
                Changed,
                R::NoBindingEvidence,
                ProofLevel::ExternallyVerified,
            ),
            (B::NoBindingEvidence, T::Unanchored | T::Failed) => {
                Verdict::new(Changed, R::NoBindingEvidence, Unobserved)
            }
        },
    }
}

/// Every variant of every axis, so tests and the CLI can walk the mapping rather than sample it.
pub const EXHAUSTION: [Exhaustion; 2] = [Exhaustion::AllRungsRan, Exhaustion::RungCouldNotRun];

pub const REJECTIONS: [RejectionReason; 4] = [
    RejectionReason::NoncanonicalManifest,
    RejectionReason::LocatorMismatch,
    RejectionReason::AmbiguousBinding,
    RejectionReason::InvariantsFailed,
];

pub const SIGNATURE_OUTCOMES: [SignatureOutcome; 4] = [
    SignatureOutcome::Valid,
    SignatureOutcome::Invalid,
    SignatureOutcome::Absent,
    SignatureOutcome::UnsupportedAlgorithm,
];

pub const BINDING_CLASSES: [BindingClass; 10] = [
    BindingClass::HardExactContent,
    BindingClass::HardExactDecodedAudioOnly,
    BindingClass::HardMismatch,
    BindingClass::HardMismatchSoftAffirmed,
    BindingClass::SoftMarkStrongAtOrAboveThreshold,
    BindingClass::SoftMarkStrongBelowThreshold,
    BindingClass::SoftMarkInferredOnly,
    BindingClass::SoftMarkFalsePositiveRateUnknown,
    BindingClass::SoftFingerprintInferred,
    BindingClass::NoBindingEvidence,
];

pub const TRUST_OUTCOMES: [TrustOutcome; 3] = [
    TrustOutcome::Anchored,
    TrustOutcome::Unanchored,
    TrustOutcome::Failed,
];

/// Every input the mapping accepts, in a stable order.
pub fn all_status_inputs() -> Vec<StatusInputs> {
    let mut inputs =
        Vec::with_capacity(EXHAUSTION.len() + REJECTIONS.len() + 4 * BINDING_CLASSES.len() * 3);
    inputs.extend(EXHAUSTION.iter().copied().map(StatusInputs::NoCandidate));
    inputs.extend(
        REJECTIONS
            .iter()
            .copied()
            .map(StatusInputs::CandidateRejected),
    );
    for signature in SIGNATURE_OUTCOMES {
        for binding in BINDING_CLASSES {
            for trust in TRUST_OUTCOMES {
                inputs.push(StatusInputs::Candidate {
                    signature,
                    binding,
                    trust,
                });
            }
        }
    }
    inputs
}
