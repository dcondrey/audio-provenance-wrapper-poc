//! One error type for the facade, every variant carrying a stable code string.

use audio_provenance_audio::AudioError;
use audio_provenance_core::{CodedError, KeyError, SignatureError};
use audio_provenance_manifest::ManifestError;
use audio_provenance_registry::{ConfigError, RegistryError, ResolveError};
use genotrace::GenoTraceError;

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error(transparent)]
    Recovery(#[from] GenoTraceError),

    /// A registry rung was meant to run and could not.
    ///
    /// IMPORTANT: this is why `verify` returns a `Result` at all beyond caller errors. An outage
    /// collapsed into `not_found` would publish "this work is unregistered" on the strength of a
    /// dropped connection. GenoTrace keeps the distinction as `incomplete`; the facade refuses to
    /// hand a caller a `not_found` that rests on it.
    #[error("registry rung {method} could not run: {detail}")]
    RegistryUnavailable {
        method: &'static str,
        detail: String,
    },

    /// A non-registry rung was meant to run and could not, and nothing was recovered.
    #[error("recovery rung {method} could not run: {detail}")]
    RecoveryIncomplete {
        method: &'static str,
        detail: String,
    },

    #[error("option {option} is invalid: {reason}")]
    InvalidOption {
        option: &'static str,
        reason: String,
    },

    /// Marking changes the audio a hard binding covers, so a mark applied after signing invalidates
    /// the record that was just written. GENOMARK_SPEC section 3.6 fixes the order: embed, then
    /// hash, then sign.
    #[error(
        "{path} already carries a Audio Provenance manifest; marking it would invalidate that record"
    )]
    MarkAfterSign { path: String },

    #[error("{container} is not a container this build can write a manifest into")]
    ContainerNotWritable { container: &'static str },

    #[error("the operating system could not supply {bytes} bytes of entropy")]
    Entropy { bytes: usize },

    /// A mark allocated under one key, signed with another. The record would land under a locator
    /// no mark points at.
    #[error("the signing key derives locator {expected}, but the mark carries {found}")]
    LocatorKeyMismatch { expected: String, found: String },

    #[error("{path} is not a usable RIFF/WAVE file: {reason}")]
    MalformedRiff { path: String, reason: &'static str },

    #[error("registry {name:?} is a {kind} backend; no publish protocol is defined for it")]
    PublishUnsupported { name: String, kind: &'static str },

    /// The record the registry would store does not hash to the digest the signer reported.
    #[error("record digest {derived} does not match the signed manifest's {declared}")]
    RecordDigestMismatch { declared: String, derived: String },

    #[error("capture adapter input is invalid: {reason}")]
    CaptureAdapterInvalid { reason: String },

    #[error("{what} digest {observed} does not match expected {expected}")]
    DigestMismatch {
        what: &'static str,
        expected: String,
        observed: String,
    },

    #[error("capture input {path} is {bytes} bytes; maximum is {max_bytes}")]
    CaptureInputTooLarge {
        path: String,
        bytes: u64,
        max_bytes: u64,
    },

    #[error("could not read or write {path}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    Signature(#[from] SignatureError),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error(transparent)]
    Mark(#[from] genomark::GenoMarkError),
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    RegistryResolve(#[from] ResolveError),
    #[error(transparent)]
    RegistryConfig(#[from] ConfigError),
}

impl SdkError {
    pub(crate) fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }
}

impl CodedError for SdkError {
    fn code(&self) -> &'static str {
        match self {
            Self::Recovery(inner) => inner.code(),
            Self::RegistryUnavailable { .. } => "registry_unavailable",
            Self::RecoveryIncomplete { .. } => "recovery_incomplete",
            Self::InvalidOption { .. } => "option_invalid",
            Self::MarkAfterSign { .. } => "mark_after_sign",
            Self::ContainerNotWritable { .. } => "container_not_writable",
            Self::Entropy { .. } => "entropy_unavailable",
            Self::LocatorKeyMismatch { .. } => "locator_key_mismatch",
            Self::MalformedRiff { .. } => "riff_malformed",
            Self::PublishUnsupported { .. } => "registry_publish_unsupported",
            Self::RecordDigestMismatch { .. } => "record_digest_mismatch",
            Self::CaptureAdapterInvalid { .. } => "capture_adapter_input_invalid",
            Self::DigestMismatch { .. } => "capture_adapter_digest_mismatch",
            Self::CaptureInputTooLarge { .. } => "capture_adapter_input_too_large",
            Self::Io { .. } => "sdk_io_failed",
            Self::Manifest(inner) => inner.code(),
            Self::Signature(inner) => inner.code(),
            Self::Key(inner) => inner.code(),
            Self::Mark(inner) => inner.code(),
            Self::Audio(inner) => inner.code(),
            Self::Registry(inner) => inner.code(),
            Self::RegistryResolve(inner) => inner.code(),
            Self::RegistryConfig(inner) => inner.code(),
        }
    }
}
