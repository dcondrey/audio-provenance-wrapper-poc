use alloc::string::String;

pub trait CodedError {
    fn code(&self) -> &'static str;
}

#[derive(Debug, thiserror::Error)]
pub enum CanonicalJsonError {
    #[error("value nesting exceeds {limit} levels at {path}")]
    DepthExceeded { limit: usize, path: String },
    #[error("NaN and infinities are not representable in apw-json-sort-v1")]
    NonFiniteFloat,
    #[error("number is not representable as a JSON number")]
    UnrepresentableNumber,
    /// IMPORTANT: an integer literal wider than i64/u64 is parsed into an f64, which both loses the
    /// exact value and collapses distinct documents onto identical canonical bytes. Since these
    /// bytes are the signed content, that would let a signature over one document verify another.
    #[error("integer literal {literal} exceeds the signable 64-bit range")]
    IntegerOutOfRange { literal: String },
    #[error("signing input is not well-formed JSON: {reason}")]
    Malformed { reason: String },
}

impl CodedError for CanonicalJsonError {
    fn code(&self) -> &'static str {
        match self {
            Self::DepthExceeded { .. } => "canonical_json_depth_exceeded",
            Self::NonFiniteFloat => "canonical_json_non_finite_float",
            Self::UnrepresentableNumber => "canonical_json_unrepresentable_number",
            Self::IntegerOutOfRange { .. } => "canonical_json_integer_out_of_range",
            Self::Malformed { .. } => "canonical_json_malformed",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("Ed25519 private key must be exactly 32 raw bytes, found {found}")]
    PrivateKeyLength { found: usize },
    #[error("Ed25519 public key must be exactly 32 raw bytes, found {found}")]
    PublicKeyLength { found: usize },
    #[error("key material is not valid hexadecimal")]
    MalformedHex,
    #[error("public key is not a valid Ed25519 point")]
    InvalidPublicKey,
    #[cfg(feature = "std")]
    #[error("could not read key material from disk")]
    Io(#[from] std::io::Error),
}

impl CodedError for KeyError {
    fn code(&self) -> &'static str {
        match self {
            Self::PrivateKeyLength { .. } => "key_private_length_invalid",
            Self::PublicKeyLength { .. } => "key_public_length_invalid",
            Self::MalformedHex => "key_hex_malformed",
            Self::InvalidPublicKey => "key_public_invalid",
            #[cfg(feature = "std")]
            Self::Io(_) => "key_io_failed",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SignatureError {
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error(transparent)]
    Canonicalization(#[from] CanonicalJsonError),
    #[error("portable signature must be exactly 64 raw bytes, found {found}")]
    SignatureLength { found: usize },
    #[error("signed_content_hash must be a 64-character SHA-256 hex digest")]
    MalformedContentHash,
    #[error("portable signed-content hash does not match canonical manifest")]
    ContentHashMismatch,
    #[error("Ed25519 signature is invalid")]
    InvalidSignature,
    #[error("remote signing failed: {detail}")]
    RemoteSigning { detail: String },
}

impl CodedError for SignatureError {
    fn code(&self) -> &'static str {
        match self {
            Self::Key(inner) => inner.code(),
            Self::Canonicalization(inner) => inner.code(),
            Self::SignatureLength { .. } => "signature_length_invalid",
            Self::MalformedContentHash => "signature_content_hash_malformed",
            Self::ContentHashMismatch => "signature_content_hash_mismatch",
            Self::InvalidSignature => "signature_invalid",
            Self::RemoteSigning { .. } => "signature_remote_failed",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LocatorError {
    #[error("locator_salt must be exactly {expected} lowercase hex characters, found {found}")]
    SaltHexLength { expected: usize, found: usize },
    #[error("locator_salt must contain only lowercase hexadecimal characters")]
    SaltHexCharset,
}

impl CodedError for LocatorError {
    fn code(&self) -> &'static str {
        match self {
            Self::SaltHexLength { .. } => "locator_salt_hex_length",
            Self::SaltHexCharset => "locator_salt_hex_charset",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VocabularyError {
    #[error("an established stem_export_association must remain inferred, found {found}")]
    AssociationOverstated { found: &'static str },
    #[error("an unavailable stem_export_association must remain unknown_unobserved, found {found}")]
    AssociationUnderstated { found: &'static str },
    #[error("complete_observed_path requires counters")]
    CoverageCountersMissing,
    #[error("declared coverage {declared} is not supported by the counters, which prove {derived}")]
    CoverageOverstated {
        declared: &'static str,
        derived: &'static str,
    },
    #[error("coverage {status} must carry proof level {expected}, found {found}")]
    CoverageProofLevelMismatch {
        status: &'static str,
        expected: &'static str,
        found: &'static str,
    },
    #[error(
        "a self-generated portable signer identity must remain unknown_unobserved, found {found}"
    )]
    SignerIdentityOverstated { found: &'static str },
    #[error("the portable signature act must be directly_observed, found {found}")]
    SignatureProofLevelInvalid { found: &'static str },
}

impl CodedError for VocabularyError {
    fn code(&self) -> &'static str {
        match self {
            Self::AssociationOverstated { .. } => "association_proof_level_overstated",
            Self::AssociationUnderstated { .. } => "association_proof_level_understated",
            Self::CoverageCountersMissing => "coverage_counters_missing",
            Self::CoverageOverstated { .. } => "coverage_status_overstated",
            Self::CoverageProofLevelMismatch { .. } => "coverage_proof_level_mismatch",
            Self::SignerIdentityOverstated { .. } => "signer_identity_proof_level_overstated",
            Self::SignatureProofLevelInvalid { .. } => "signature_proof_level_invalid",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error(transparent)]
    CanonicalJson(#[from] CanonicalJsonError),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error(transparent)]
    Locator(#[from] LocatorError),
    #[error(transparent)]
    Signature(#[from] SignatureError),
    #[error(transparent)]
    Vocabulary(#[from] VocabularyError),
}

impl CodedError for CoreError {
    fn code(&self) -> &'static str {
        match self {
            Self::CanonicalJson(inner) => inner.code(),
            Self::Key(inner) => inner.code(),
            Self::Locator(inner) => inner.code(),
            Self::Signature(inner) => inner.code(),
            Self::Vocabulary(inner) => inner.code(),
        }
    }
}
