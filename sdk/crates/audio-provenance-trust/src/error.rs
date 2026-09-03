//! Operational faults. A chain that legitimately resolves to nothing is a [`crate::Refusal`], not
//! an error: "this store does not vouch for that key" is an answer, and only a malformed store, an
//! unreadable path or an impossible issuance lands here.

use std::io;

use audio_provenance_core::{CanonicalJsonError, CodedError, KeyError, SignatureError};

#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    #[error("not a `YYYY-MM-DDTHH:MM:SSZ` UTC instant: {found:?}")]
    Instant { found: String },
    #[error("validity window {not_before} .. {not_after} is empty")]
    EmptyWindow {
        not_before: String,
        not_after: String,
    },
    #[error("{what} is not well-formed: {reason}")]
    Malformed { what: &'static str, reason: String },
    #[error("expected a {expected} document, found type {found:?}")]
    UnexpectedType {
        expected: &'static str,
        found: String,
    },
    #[error("{field} is not {expected_bytes} bytes of lowercase hex")]
    Hex {
        field: &'static str,
        expected_bytes: usize,
    },
    #[error("{field} is empty")]
    FieldEmpty { field: &'static str },
    #[error("{field} is {found} bytes, over the {limit}-byte limit")]
    FieldTooLong {
        field: &'static str,
        limit: usize,
        found: usize,
    },
    /// A display name reaches a terminal verbatim. Control bytes in one are an escape-sequence
    /// injection, not a naming choice, so they are refused at the parse boundary.
    #[error("{field} contains a control character or is not valid printable text")]
    UnprintableText { field: &'static str },
    #[error("store holds {found} {what}, over the {limit} limit")]
    TooMany {
        what: &'static str,
        limit: usize,
        found: usize,
    },
    #[error("{path} is {found} bytes, over the {limit}-byte trust-store limit")]
    StoreTooLarge {
        path: String,
        limit: u64,
        found: u64,
    },
    #[error("could not {action} {path}")]
    Io {
        action: &'static str,
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("the {what} signature on {id} does not verify under the declared key")]
    DocumentSignatureInvalid { what: &'static str, id: String },
    #[error("expected format {expected}, found {found:?}")]
    UnsupportedFormat {
        expected: &'static str,
        found: String,
    },
    #[error("{what} declares {declared} but the key given derives {derived}")]
    Inconsistent {
        what: &'static str,
        declared: String,
        derived: String,
    },
    #[error("{id} is declared twice in the same store")]
    Duplicate { id: String },
    #[error(transparent)]
    Canonical(#[from] CanonicalJsonError),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error(transparent)]
    Signature(#[from] SignatureError),
}

impl TrustError {
    pub fn io(action: &'static str, path: impl std::fmt::Display) -> impl Fn(io::Error) -> Self {
        let path = path.to_string();
        move |source| Self::Io {
            action,
            path: path.clone(),
            source,
        }
    }
}

impl CodedError for TrustError {
    fn code(&self) -> &'static str {
        match self {
            Self::Instant { .. } => "trust_instant_invalid",
            Self::EmptyWindow { .. } => "trust_window_empty",
            Self::Malformed { .. } => "trust_document_malformed",
            Self::UnexpectedType { .. } => "trust_document_type_unexpected",
            Self::Hex { .. } => "trust_hex_invalid",
            Self::FieldEmpty { .. } => "trust_field_empty",
            Self::FieldTooLong { .. } => "trust_field_too_long",
            Self::UnprintableText { .. } => "trust_field_unprintable",
            Self::TooMany { .. } => "trust_store_too_many_entries",
            Self::StoreTooLarge { .. } => "trust_store_too_large",
            Self::Io { .. } => "trust_path_unreadable",
            Self::DocumentSignatureInvalid { .. } => "trust_document_signature_invalid",
            Self::UnsupportedFormat { .. } => "trust_format_unsupported",
            Self::Inconsistent { .. } => "trust_document_inconsistent",
            Self::Duplicate { .. } => "trust_document_duplicate",
            Self::Canonical(inner) => inner.code(),
            Self::Key(inner) => inner.code(),
            Self::Signature(inner) => inner.code(),
        }
    }
}
