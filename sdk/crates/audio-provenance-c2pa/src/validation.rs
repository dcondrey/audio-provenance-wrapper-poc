//! From a C2PA validator's own result codes to a Audio Provenance verification status.
//!
//! # Why this is a two-step mapping and not a rename
//!
//! `audio_provenance_manifest::c2pa_validation_state_to_status` already carries the last step: the POC's
//! four `c2pa_claim.validation.state` names and Audio Provenance's four [`VerificationStatus`] values are
//! the same four outcomes. What was missing is the step before it, deriving which of the four
//! states a real validator's output actually means. c2patool reports facts, not states: it emits
//! `claimSignature.validated`, `assertion.hashedURI.match`, `assertion.dataHash.match` and
//! `signingCredential.untrusted` as separate rows, and it is the combination that picks the state.
//!
//! # Precedence, matching `apw_trace::derive_status`
//!
//! Recovery, then store integrity, then signature, then binding, then trust. Store integrity sits
//! above the binding on purpose: an assertion store that does not recompute against the claim's
//! hashed URIs makes the `c2pa.hash.data` assertion itself untrustworthy, so its verdict cannot be
//! read as evidence about the audio. That is the same reasoning that puts
//! `CandidateRejected(NoncanonicalManifest)` above binding evaluation in the ladder, and it lands
//! on the same status: `untrusted`, never `changed`.
//!
//! Every row of [`classify`] has a counterpart in `apw_trace::derive_status`, reached with the
//! `StatusInputs` on the right:
//!
//! | C2PA evidence                                   | state                          | `StatusInputs`                                              | status      |
//! |-------------------------------------------------|--------------------------------|-------------------------------------------------------------|-------------|
//! | no store                                        | `nothing_found`                | `NoCandidate(AllRungsRan)`                                    | `not_found` |
//! | assertion store not intact                      | `mark_found_claim_not_trusted` | `CandidateRejected(NoncanonicalManifest)`                     | `untrusted` |
//! | signature invalid / absent / not evaluated      | `mark_found_claim_not_trusted` | `Candidate { signature: Invalid \| Absent, .. }`              | `untrusted` |
//! | data hash mismatched                            | `registered_but_changed`       | `Candidate { binding: HardMismatch, .. }`                     | `changed`   |
//! | no `c2pa.hash.data`                             | `mark_found_claim_not_trusted` | `CandidateRejected(InvariantsFailed)`                         | `untrusted` |
//! | data hash matched, credential untrusted/rejected| `mark_found_claim_not_trusted` | `Candidate { binding: HardExactContent, trust: Unanchored }`  | `untrusted` |
//! | data hash matched, credential trusted           | `verified`                     | `Candidate { binding: HardExactContent, trust: Anchored }`    | `verified`  |
//!
//! IMPORTANT: the fifth row is a rejection and not `BindingClass::NoBindingEvidence`, which the
//! ladder reads as `changed`. C2PA 2.4, "Validate the correct assertions for the type of manifest",
//! requires a standard manifest to carry exactly one hard binding and rejects a manifest without
//! one as `claim.hardBindings.missing`. That says the CLAIM is malformed, not that the audio moved,
//! and a Audio Provenance record may legitimately declare no hard binding where a C2PA claim may not.
//!
//! `tests/status_correspondence.rs` calls `derive_status` over all 144 evidence combinations, so
//! this table is checked by the compiler rather than by reading. It is what caught that fifth row
//! being wrong on the first pass.
//!
//! # The honesty boundary is in the type
//!
//! This crate verifies no COSE_Sign1 signature and builds no X.509 chain, so it can never source
//! [`ClaimSignatureEvidence::Validated`] or [`CredentialEvidence::Trusted`] itself. Those come from
//! a validator that does the work: c2patool, the POC's c2pa-rs signer, or a future Audio Provenance COSE
//! verifier. [`ClaimSignatureEvidence::NotEvaluated`] is what this crate alone can honestly report,
//! and it classifies as `mark_found_claim_not_trusted`.

use audio_provenance_core::VerificationStatus;
use audio_provenance_manifest::BindingOutcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaimSignatureEvidence {
    /// A validator verified COSE_Sign1 over the claim: c2patool's `claimSignature.validated`.
    Validated,
    Invalid,
    /// The store carries no `c2pa.signature` box.
    Absent,
    /// Nobody has looked. This is what `audio-provenance-c2pa` alone can say.
    NotEvaluated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialEvidence {
    /// The signing certificate chained to a configured anchor.
    Trusted,
    /// No anchor covers the signer: c2patool's `signingCredential.untrusted`. A self-issued local
    /// root lands here for every party that has not installed it, which is the POC's own case.
    Untrusted,
    /// An anchor covers the signer and rejected it: revoked, expired, or otherwise refused.
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HardBindingEvidence {
    Match,
    Mismatch,
    /// The claim carries no `c2pa.hash.data`, so nothing binds it to these bytes.
    Absent,
}

impl HardBindingEvidence {
    pub const fn from_outcome(outcome: BindingOutcome) -> Self {
        match outcome {
            BindingOutcome::Match => Self::Match,
            BindingOutcome::Mismatch => Self::Mismatch,
            BindingOutcome::Uncoverable => Self::Absent,
        }
    }
}

/// Every fact the four states are a function of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct C2paEvidence {
    /// A manifest store was located in the container or beside it.
    pub store_present: bool,
    /// Every hashed URI in the claim recomputed over the assertion store presented with it.
    pub assertion_store_intact: bool,
    pub signature: ClaimSignatureEvidence,
    pub hard_binding: HardBindingEvidence,
    pub credential: CredentialEvidence,
}

impl C2paEvidence {
    /// Nothing was found. Every other field is the value that cannot promote it.
    pub const fn nothing_found() -> Self {
        Self {
            store_present: false,
            assertion_store_intact: false,
            signature: ClaimSignatureEvidence::Absent,
            hard_binding: HardBindingEvidence::Absent,
            credential: CredentialEvidence::Untrusted,
        }
    }
}

/// The POC's `c2pa_claim.validation.state` vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum C2paValidationState {
    Verified,
    RegisteredButChanged,
    MarkFoundClaimNotTrusted,
    NothingFound,
}

impl C2paValidationState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::RegisteredButChanged => "registered_but_changed",
            Self::MarkFoundClaimNotTrusted => "mark_found_claim_not_trusted",
            Self::NothingFound => "nothing_found",
        }
    }

    pub const fn to_status(self) -> VerificationStatus {
        match self {
            Self::Verified => VerificationStatus::Verified,
            Self::RegisteredButChanged => VerificationStatus::Changed,
            Self::MarkFoundClaimNotTrusted => VerificationStatus::Untrusted,
            Self::NothingFound => VerificationStatus::NotFound,
        }
    }
}

/// The total function from evidence to state.
pub const fn classify(evidence: C2paEvidence) -> C2paValidationState {
    use C2paValidationState as S;

    if !evidence.store_present {
        return S::NothingFound;
    }
    if !evidence.assertion_store_intact {
        return S::MarkFoundClaimNotTrusted;
    }
    match evidence.signature {
        ClaimSignatureEvidence::Invalid
        | ClaimSignatureEvidence::Absent
        | ClaimSignatureEvidence::NotEvaluated => return S::MarkFoundClaimNotTrusted,
        ClaimSignatureEvidence::Validated => {}
    }
    match evidence.hard_binding {
        HardBindingEvidence::Mismatch => return S::RegisteredButChanged,
        HardBindingEvidence::Absent => return S::MarkFoundClaimNotTrusted,
        HardBindingEvidence::Match => {}
    }
    match evidence.credential {
        CredentialEvidence::Trusted => S::Verified,
        CredentialEvidence::Untrusted | CredentialEvidence::Rejected => S::MarkFoundClaimNotTrusted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed() -> C2paEvidence {
        C2paEvidence {
            store_present: true,
            assertion_store_intact: true,
            signature: ClaimSignatureEvidence::Validated,
            hard_binding: HardBindingEvidence::Match,
            credential: CredentialEvidence::Trusted,
        }
    }

    /// The invariant the product rests on: a perfect signature over unmoved bytes is still only
    /// `untrusted` while no anchor covers the signer. This is the POC's own situation.
    #[test]
    fn an_unanchored_signer_never_reaches_verified_however_intact_the_bytes_are() {
        let unanchored = C2paEvidence {
            credential: CredentialEvidence::Untrusted,
            ..signed()
        };
        assert_eq!(
            classify(unanchored),
            C2paValidationState::MarkFoundClaimNotTrusted
        );
        assert_eq!(classify(signed()), C2paValidationState::Verified);
    }

    /// A corrupt assertion store outranks the binding, so a tampered manifest can never present
    /// itself as a clean report about the audio.
    #[test]
    fn a_broken_assertion_store_is_untrusted_and_not_changed() {
        let corrupt = C2paEvidence {
            assertion_store_intact: false,
            hard_binding: HardBindingEvidence::Mismatch,
            ..signed()
        };
        assert_eq!(
            classify(corrupt),
            C2paValidationState::MarkFoundClaimNotTrusted
        );
    }

    #[test]
    fn the_state_names_agree_with_the_manifest_crates_own_mapping() {
        for state in [
            C2paValidationState::Verified,
            C2paValidationState::RegisteredButChanged,
            C2paValidationState::MarkFoundClaimNotTrusted,
            C2paValidationState::NothingFound,
        ] {
            assert_eq!(
                audio_provenance_manifest::c2pa_validation_state_to_status(state.as_str()),
                Some(state.to_status()),
                "{}",
                state.as_str()
            );
        }
    }
}
