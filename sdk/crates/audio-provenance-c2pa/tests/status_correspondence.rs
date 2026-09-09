//! The mapping table in `validation.rs`, checked by the compiler rather than by reading.
//!
//! Both routes to a [`VerificationStatus`] are total functions over an explicit enumeration. This
//! walks every C2PA evidence combination that exists, names the `StatusInputs` the ladder would see
//! for the same facts, and requires the two to land on the same status. A row that drifts fails
//! here instead of quietly making the documented correspondence a lie.

#![allow(clippy::unwrap_used, clippy::panic)]

use audio_provenance_c2pa::{
    C2paEvidence, C2paValidationState, ClaimSignatureEvidence, CredentialEvidence,
    HardBindingEvidence, classify,
};
use audio_provenance_core::VerificationStatus;
use apw_trace::{
    BindingClass, Exhaustion, RejectionReason, SignatureOutcome, StatusInputs, TrustOutcome,
    derive_status,
};

/// The `StatusInputs` the recovery ladder would be handed for the same C2PA facts.
fn ladder_equivalent(evidence: C2paEvidence) -> StatusInputs {
    if !evidence.store_present {
        return StatusInputs::NoCandidate(Exhaustion::AllRungsRan);
    }
    // A claim whose hashed URIs do not recompute is the C2PA analogue of a manifest whose bytes are
    // not the bytes that were signed over: rejected before any binding is evaluated.
    if !evidence.assertion_store_intact {
        return StatusInputs::CandidateRejected(RejectionReason::NoncanonicalManifest);
    }
    // C2PA 2.4, "Validate the correct assertions for the type of manifest": a standard manifest
    // MUST carry exactly one hard binding, and a validator that finds none rejects the manifest
    // with `claim.hardBindings.missing`. That is a statement about the claim, not about the audio,
    // so it is a rejected candidate and NOT `BindingClass::NoBindingEvidence`, which the ladder
    // reads as `changed` because a Audio Provenance record may legitimately declare no hard binding.
    if evidence.hard_binding == HardBindingEvidence::Absent {
        return StatusInputs::CandidateRejected(RejectionReason::InvariantsFailed);
    }
    StatusInputs::Candidate {
        signature: match evidence.signature {
            ClaimSignatureEvidence::Validated => SignatureOutcome::Valid,
            ClaimSignatureEvidence::Invalid => SignatureOutcome::Invalid,
            // Nobody looked, so nothing was proved. The ladder's nearest honest input is the same
            // one it uses for a manifest carrying no signature at all.
            ClaimSignatureEvidence::Absent | ClaimSignatureEvidence::NotEvaluated => {
                SignatureOutcome::Absent
            }
        },
        binding: match evidence.hard_binding {
            HardBindingEvidence::Match => BindingClass::HardExactContent,
            HardBindingEvidence::Mismatch => BindingClass::HardMismatch,
            // Unreachable: the early return above rejects an absent hard binding. The arm exists so
            // adding a fourth `HardBindingEvidence` variant fails to compile here.
            HardBindingEvidence::Absent => BindingClass::NoBindingEvidence,
        },
        trust: match evidence.credential {
            CredentialEvidence::Trusted => TrustOutcome::Anchored,
            CredentialEvidence::Untrusted => TrustOutcome::Unanchored,
            CredentialEvidence::Rejected => TrustOutcome::Failed,
        },
    }
}

fn every_evidence() -> Vec<C2paEvidence> {
    let mut out = Vec::new();
    for store_present in [true, false] {
        for assertion_store_intact in [true, false] {
            for signature in [
                ClaimSignatureEvidence::Validated,
                ClaimSignatureEvidence::Invalid,
                ClaimSignatureEvidence::Absent,
                ClaimSignatureEvidence::NotEvaluated,
            ] {
                for hard_binding in [
                    HardBindingEvidence::Match,
                    HardBindingEvidence::Mismatch,
                    HardBindingEvidence::Absent,
                ] {
                    for credential in [
                        CredentialEvidence::Trusted,
                        CredentialEvidence::Untrusted,
                        CredentialEvidence::Rejected,
                    ] {
                        out.push(C2paEvidence {
                            store_present,
                            assertion_store_intact,
                            signature,
                            hard_binding,
                            credential,
                        });
                    }
                }
            }
        }
    }
    out
}

#[test]
fn every_c2pa_evidence_combination_lands_where_the_recovery_ladder_lands() {
    let cases = every_evidence();
    assert_eq!(cases.len(), 2 * 2 * 4 * 3 * 3);

    for evidence in cases {
        let ours = classify(evidence).to_status();
        let ladder = derive_status(ladder_equivalent(evidence)).status();
        assert_eq!(ours, ladder, "{evidence:?}");
    }
}

/// The four names are the same four outcomes, and every one of them is reachable. A mapping that
/// can never produce `verified` would satisfy the test above and be useless.
#[test]
fn all_four_states_are_reachable_and_distinct() {
    let reached: Vec<VerificationStatus> = every_evidence()
        .into_iter()
        .map(|e| classify(e).to_status())
        .collect();

    for state in [
        C2paValidationState::Verified,
        C2paValidationState::RegisteredButChanged,
        C2paValidationState::MarkFoundClaimNotTrusted,
        C2paValidationState::NothingFound,
    ] {
        assert!(
            reached.contains(&state.to_status()),
            "unreachable: {}",
            state.as_str()
        );
    }
}
