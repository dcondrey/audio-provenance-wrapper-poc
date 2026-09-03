use alloc::string::String;

use audio_provenance_core::CodedError;

#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    #[error("the manifest store carries no manifest superbox")]
    NoActiveManifest,
    #[error("the manifest store carries no claim box")]
    NoClaim,
    #[error("the store nests JUMBF boxes past {limit} levels")]
    DepthExceeded { limit: usize },
    #[error("the store holds more than {limit} JUMBF boxes")]
    BoxLimit { limit: usize },
    #[error("JUMBF structure is malformed at byte {offset}: {reason}")]
    MalformedJumbf { offset: usize, reason: &'static str },
    /// ISO/IEC 19566-5 extended (64-bit) box lengths are refused rather than guessed at, matching
    /// the reader in `audio-provenance-manifest`.
    #[error("JUMBF box at byte {offset} uses an extended length, which is not supported")]
    ExtendedLength { offset: usize },
    #[error("the claim box holds no CBOR content box")]
    ClaimNotCbor,
    #[error("the claim is not well-formed CBOR: {reason}")]
    MalformedClaim { reason: String },
    #[error("claim field {field} is missing or has the wrong type")]
    ClaimField { field: &'static str },
    #[error("a hashed URI in {field} is malformed: {reason}")]
    MalformedHashedUri {
        field: &'static str,
        reason: &'static str,
    },
}

impl CodedError for ClaimError {
    fn code(&self) -> &'static str {
        match self {
            Self::NoActiveManifest => "c2pa_active_manifest_absent",
            Self::NoClaim => "c2pa_claim_absent",
            Self::DepthExceeded { .. } => "c2pa_jumbf_depth_exceeded",
            Self::BoxLimit { .. } => "c2pa_jumbf_box_limit",
            Self::MalformedJumbf { .. } => "c2pa_jumbf_malformed",
            Self::ExtendedLength { .. } => "c2pa_jumbf_extended_length",
            Self::ClaimNotCbor => "c2pa_claim_not_cbor",
            Self::MalformedClaim { .. } => "c2pa_claim_malformed",
            Self::ClaimField { .. } => "c2pa_claim_field_invalid",
            Self::MalformedHashedUri { .. } => "c2pa_hashed_uri_malformed",
        }
    }
}
