use alloc::string::String;
use alloc::vec::Vec;

use audio_provenance_core::{CanonicalJsonError, CodedError, SignatureError};

/// Refused before any parse. A manifest is a record, not a payload: the largest real POC manifest
/// in the prior art is 38 KB.
pub const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest is {found} bytes, over the {limit}-byte admission limit")]
    TooLarge { limit: usize, found: usize },
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    #[error(transparent)]
    Signature(#[from] SignatureError),
    #[error("manifest must be a JSON object")]
    NotAnObject,
    #[error("expected schema {expected}, found {found}")]
    SchemaMismatch {
        expected: &'static str,
        found: String,
    },
    #[error("required field is missing: {field}")]
    MissingField { field: &'static str },
    #[error("field {field} must be {expected}")]
    FieldType {
        field: &'static str,
        expected: &'static str,
    },
    #[error("{field} must be a 64-character lowercase SHA-256 hex digest")]
    MalformedDigest { field: &'static str },
    #[error("{field} must be a UTC calendar date spelled YYYY-MM-DD")]
    MalformedDate { field: &'static str },
    #[error("portable_signature is not a well-formed signature block: {reason}")]
    SignatureBlock { reason: String },
    /// The received bytes are not the bytes `apw-json-sort-v1` produces for the parsed value.
    #[error("manifest bytes are not canonical, so the record id derived from them is not stable")]
    NoncanonicalManifest,
    #[error("manifest failed {count} invariant check(s): {first}")]
    Invariants { count: usize, first: String },
}

impl CodedError for ManifestError {
    fn code(&self) -> &'static str {
        match self {
            Self::TooLarge { .. } => "manifest_too_large",
            Self::Canonical(inner) => inner.code(),
            Self::Signature(inner) => inner.code(),
            Self::NotAnObject => "manifest_not_an_object",
            Self::SchemaMismatch { .. } => "manifest_schema_mismatch",
            Self::MissingField { .. } => "manifest_field_missing",
            Self::FieldType { .. } => "manifest_field_type_invalid",
            Self::MalformedDigest { .. } => "manifest_digest_malformed",
            Self::MalformedDate { .. } => "manifest_date_malformed",
            Self::SignatureBlock { .. } => "manifest_signature_block_malformed",
            Self::NoncanonicalManifest => "noncanonical_manifest",
            Self::Invariants { .. } => "manifest_invariants_failed",
        }
    }
}

impl ManifestError {
    pub(crate) fn from_findings(messages: Vec<String>) -> Self {
        let count = messages.len();
        let first = messages
            .into_iter()
            .next()
            .unwrap_or_else(|| String::from("no detail recorded"));
        Self::Invariants { count, first }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum C2paError {
    #[error("the container carries no C2PA manifest store")]
    NotPresent,
    #[error("RIFF/WAVE structure is malformed at byte {offset}: {reason}")]
    MalformedRiff { offset: usize, reason: &'static str },
    #[error("JUMBF box at byte {offset} is malformed: {reason}")]
    MalformedJumbf { offset: usize, reason: &'static str },
    #[error("JUMBF nesting exceeds {limit} levels")]
    JumbfDepthExceeded { limit: usize },
    #[error("JUMBF store declares more than {limit} boxes")]
    JumbfBoxLimit { limit: usize },
    /// ISO/IEC 19566-5 inherits ISO-BMFF's extended box lengths. c2pa-rs emits neither, so a file
    /// carrying one is refused rather than guessed at.
    #[error("JUMBF box at byte {offset} uses an unsupported extended length form")]
    JumbfExtendedLength { offset: usize },
    #[error("C2PA store is {found} bytes, over the {limit}-byte limit")]
    TooLarge { limit: usize, found: usize },
    #[error("assertion {label} is not well-formed CBOR: {reason}")]
    MalformedAssertion { label: &'static str, reason: String },
    #[error("the manifest store carries no hard-binding assertion")]
    NoHardBinding,
    #[error("hard binding declares unsupported hash algorithm {found}")]
    UnsupportedHashAlgorithm { found: String },
    #[error(
        "hard-binding exclusion at {start}..+{length} does not lie inside the {total}-byte asset"
    )]
    ExclusionOutOfRange {
        start: u64,
        length: u64,
        total: usize,
    },
}

impl CodedError for C2paError {
    fn code(&self) -> &'static str {
        match self {
            Self::NotPresent => "c2pa_store_absent",
            Self::MalformedRiff { .. } => "c2pa_riff_malformed",
            Self::MalformedJumbf { .. } => "c2pa_jumbf_malformed",
            Self::JumbfDepthExceeded { .. } => "c2pa_jumbf_depth_exceeded",
            Self::JumbfBoxLimit { .. } => "c2pa_jumbf_box_limit",
            Self::JumbfExtendedLength { .. } => "c2pa_jumbf_extended_length",
            Self::TooLarge { .. } => "c2pa_store_too_large",
            Self::MalformedAssertion { .. } => "c2pa_assertion_malformed",
            Self::NoHardBinding => "c2pa_hard_binding_absent",
            Self::UnsupportedHashAlgorithm { .. } => "c2pa_hash_algorithm_unsupported",
            Self::ExclusionOutOfRange { .. } => "c2pa_exclusion_out_of_range",
        }
    }
}
