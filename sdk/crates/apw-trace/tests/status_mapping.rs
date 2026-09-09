#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// A test asserts; the workspace bans these in production code, where a panic is a defect rather
// than the reporting mechanism.

//! The status mapping, walked exhaustively.
//!
//! The expectation below is written from TRACE_SPEC's four predicates rather than read off the
//! implementation, so agreement is evidence and not a tautology. Every input the mapping accepts is
//! covered; a new variant on any axis makes this file stop compiling.

use audio_provenance_core::{ProofLevel, VerificationStatus};
use apw_trace::status::{
    BINDING_CLASSES, BindingClass, EXHAUSTION, Exhaustion, REJECTIONS, RejectionReason,
    SIGNATURE_OUTCOMES, SignatureOutcome, StatusInputs, StatusReason, TRUST_OUTCOMES, TrustOutcome,
    all_status_inputs, derive_status,
};

/// Predicate 1: was anything recovered. Predicate 2: does the signature verify. Predicate 3: does
/// the presented audio bind, by route (a) exact hard binding, (b) absent hard binding plus an
/// accepted Watermark soft binding, or (c) failed hard binding plus BOTH a payload naming this
/// record and the record's own signed reference constellation. Predicate 4: does the signer resolve
/// through an anchor. All four pass gives `verified`; the precedence for a failure is 1, then 2,
/// then 3, then 4.
fn expected(inputs: StatusInputs) -> (VerificationStatus, StatusReason, bool) {
    use StatusReason as R;
    use VerificationStatus::{Changed, NotFound, Untrusted, Verified};

    match inputs {
        StatusInputs::NoCandidate(Exhaustion::AllRungsRan) => {
            (NotFound, R::NoManifestRecovered, false)
        }
        StatusInputs::NoCandidate(Exhaustion::RungCouldNotRun) => {
            (NotFound, R::RecoveryIncomplete, true)
        }
        StatusInputs::CandidateRejected(reason) => (
            Untrusted,
            match reason {
                RejectionReason::NoncanonicalManifest => R::NoncanonicalManifest,
                RejectionReason::LocatorMismatch => R::LocatorMismatch,
                RejectionReason::AmbiguousBinding => R::AmbiguousBinding,
                RejectionReason::InvariantsFailed => R::ManifestInvariantsFailed,
            },
            false,
        ),
        StatusInputs::Candidate {
            signature: SignatureOutcome::Invalid,
            ..
        } => (Untrusted, R::SignatureInvalid, false),
        StatusInputs::Candidate {
            signature: SignatureOutcome::Absent,
            ..
        } => (Untrusted, R::SignatureAbsent, false),
        StatusInputs::Candidate {
            signature: SignatureOutcome::UnsupportedAlgorithm,
            ..
        } => (Untrusted, R::SignatureAlgorithmUnsupported, false),
        StatusInputs::Candidate {
            signature: SignatureOutcome::Valid,
            binding,
            trust,
        } => match binding {
            // Predicate 3 holds by route (a), (b) or (c), so predicate 4 decides.
            BindingClass::HardExactContent
            | BindingClass::HardExactDecodedAudioOnly
            | BindingClass::SoftMarkStrongAtOrAboveThreshold
            | BindingClass::HardMismatchSoftAffirmed => match trust {
                TrustOutcome::Anchored => (
                    Verified,
                    match binding {
                        BindingClass::HardExactContent => R::HardBindingMatch,
                        BindingClass::HardExactDecodedAudioOnly => R::HardBindingDecodedAudioOnly,
                        BindingClass::HardMismatchSoftAffirmed => R::SoftBindingAffirmedSameWork,
                        _ => R::SoftBindingAccepted,
                    },
                    false,
                ),
                TrustOutcome::Unanchored => (Untrusted, R::TrustAnchorUnresolved, false),
                TrustOutcome::Failed => (Untrusted, R::TrustAnchorRejected, false),
            },
            // Predicate 3 fails on a binding that moved with nothing corroborating the work:
            // `changed`, and a known signer may still be named.
            BindingClass::HardMismatch => (Changed, R::HardBindingMismatch, false),
            BindingClass::NoBindingEvidence => (Changed, R::NoBindingEvidence, false),
            // Predicate 3 fails because the association itself is too thin to attribute.
            BindingClass::SoftMarkStrongBelowThreshold => {
                (Untrusted, R::SoftBindingCoverageLow, false)
            }
            BindingClass::SoftMarkInferredOnly => {
                (Untrusted, R::SoftBindingSingleBlockInferred, false)
            }
            BindingClass::SoftMarkFalsePositiveRateUnknown => {
                (Untrusted, R::SoftBindingFalsePositiveRateUnknown, false)
            }
            BindingClass::SoftFingerprintInferred => {
                (Untrusted, R::FingerprintAssociationInferred, false)
            }
        },
    }
}

/// The one cell where identity may be disclosed on a non-`verified` status.
fn expected_discloses(inputs: StatusInputs, status: VerificationStatus) -> bool {
    let anchored = matches!(
        inputs,
        StatusInputs::Candidate {
            trust: TrustOutcome::Anchored,
            ..
        }
    );
    anchored
        && matches!(
            status,
            VerificationStatus::Verified | VerificationStatus::Changed
        )
}

#[test]
fn every_input_maps_to_the_specified_status() {
    let inputs = all_status_inputs();
    assert_eq!(
        inputs.len(),
        EXHAUSTION.len()
            + REJECTIONS.len()
            + SIGNATURE_OUTCOMES.len() * BINDING_CLASSES.len() * TRUST_OUTCOMES.len(),
        "the enumeration must cover the whole product of the axes"
    );

    for input in inputs {
        let verdict = derive_status(input);
        let (status, reason, incomplete) = expected(input);
        assert_eq!(verdict.status(), status, "status for {input:?}");
        assert_eq!(verdict.reason(), reason, "reason for {input:?}");
        assert_eq!(verdict.incomplete(), incomplete, "incomplete for {input:?}");
        assert_eq!(
            verdict.discloses_identity(),
            expected_discloses(input, status),
            "identity disclosure for {input:?}"
        );
    }
}

/// SDK_SPEC's central invariant: a name is reported IF AND ONLY IF the proof level is
/// `externally_verified`. Suppressing the string while leaving the level in place would leak a
/// trust claim into a result that carries none.
#[test]
fn identity_is_disclosed_exactly_at_externally_verified() {
    for input in all_status_inputs() {
        let verdict = derive_status(input);
        assert_eq!(
            verdict.discloses_identity(),
            verdict.identity_proof_level() == ProofLevel::ExternallyVerified,
            "{input:?}"
        );
        if matches!(
            verdict.status(),
            VerificationStatus::Untrusted | VerificationStatus::NotFound
        ) {
            assert!(
                !verdict.discloses_identity(),
                "identity must always be null on {:?}: {input:?}",
                verdict.status()
            );
        }
    }
}

/// A fingerprint hit recovers no binding from the audio; it guesses a manifest from perceptual
/// similarity. No combination of signature and trust may take that to `verified`.
#[test]
fn a_fingerprint_basis_never_verifies() {
    for signature in SIGNATURE_OUTCOMES {
        for trust in TRUST_OUTCOMES {
            let verdict = derive_status(StatusInputs::Candidate {
                signature,
                binding: BindingClass::SoftFingerprintInferred,
                trust,
            });
            assert_ne!(
                verdict.status(),
                VerificationStatus::Verified,
                "fingerprint basis verified under {signature:?}/{trust:?}"
            );
            assert!(!verdict.discloses_identity());
        }
    }
}

/// The only four binding classes that may reach `verified`, and only with a valid signature and a
/// resolved anchor.
#[test]
fn verified_requires_all_four_predicates() {
    for input in all_status_inputs() {
        if derive_status(input).status() != VerificationStatus::Verified {
            continue;
        }
        let StatusInputs::Candidate {
            signature,
            binding,
            trust,
        } = input
        else {
            panic!("only a candidate may verify: {input:?}");
        };
        assert_eq!(signature, SignatureOutcome::Valid, "{input:?}");
        assert_eq!(trust, TrustOutcome::Anchored, "{input:?}");
        assert!(
            matches!(
                binding,
                BindingClass::HardExactContent
                    | BindingClass::HardExactDecodedAudioOnly
                    | BindingClass::SoftMarkStrongAtOrAboveThreshold
                    | BindingClass::HardMismatchSoftAffirmed
            ),
            "{binding:?} must not verify"
        );
    }
}

/// An outage is `not_found` PLUS `incomplete`, never `not_found` alone, and never any other status.
#[test]
fn an_outage_is_not_found_plus_incomplete() {
    let verdict = derive_status(StatusInputs::NoCandidate(Exhaustion::RungCouldNotRun));
    assert_eq!(verdict.status(), VerificationStatus::NotFound);
    assert!(verdict.incomplete());

    let clean = derive_status(StatusInputs::NoCandidate(Exhaustion::AllRungsRan));
    assert_eq!(clean.status(), VerificationStatus::NotFound);
    assert!(!clean.incomplete());

    for input in all_status_inputs() {
        let verdict = derive_status(input);
        if verdict.incomplete() {
            assert_eq!(verdict.status(), VerificationStatus::NotFound, "{input:?}");
        }
    }
}
