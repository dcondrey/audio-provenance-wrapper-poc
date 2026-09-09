//! Operational faults. No provenance condition reaches this type: a verdict is a verdict.

use std::io;

use audio_provenance_audio::AudioError;
use audio_provenance_core::{CodedError, KeyError, SignatureError};
use audio_provenance_manifest::ManifestError;
use audio_provenance_registry::{ConfigError, RegistryError, ResolveError};
use audio_provenance_trust::TrustError;
use apw_trace::TraceError;

use crate::exit;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error("could not {action} {path}")]
    Io {
        action: &'static str,
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("refusing to overwrite {path}")]
    WouldOverwrite { path: String },
    #[error("could not draw {bytes} bytes of key material from the operating system")]
    Entropy { bytes: usize },
    #[error("could not allocate an unregistered 48-bit locator in {attempts} attempts")]
    LocatorAllocation { attempts: usize },
    #[error("registry {name:?} could not be asked whether locator {locator} is free: {detail}")]
    LocatorCheckUnavailable {
        name: String,
        locator: String,
        detail: String,
    },
    #[error(transparent)]
    Trust(#[from] TrustError),
    #[error(transparent)]
    Trace(#[from] TraceError),
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error(transparent)]
    Signature(#[from] SignatureError),
    #[error(transparent)]
    Mark(#[from] apw_watermark::WatermarkError),
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error(transparent)]
    Sdk(#[from] audio_provenance_sdk::SdkError),
    #[error("could not serialise {what}")]
    Serialise {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

impl CliError {
    pub fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    pub fn io(action: &'static str, path: impl std::fmt::Display) -> impl Fn(io::Error) -> Self {
        let path = path.to_string();
        move |source| Self::Io {
            action,
            path: path.clone(),
            source,
        }
    }

    /// Exit codes above 4 are the operational band: a misuse, an input this build will not read, or
    /// an internal fault. None of them is a provenance answer.
    pub const fn exit(&self) -> u8 {
        match self {
            Self::Usage(_)
            | Self::WouldOverwrite { .. }
            | Self::Resolve(_)
            | Self::Config(_)
            | Self::Registry(_)
            | Self::Manifest(_)
            | Self::Key(_)
            | Self::Signature(_)
            | Self::Mark(_) => exit::USAGE,
            // A malformed, over-large or self-contradicting trust store is an input this build
            // refuses to read. It is never an answer about the file being verified.
            Self::Trust(inner) => match inner {
                TrustError::Io { .. } | TrustError::StoreTooLarge { .. } => exit::INPUT,
                TrustError::Instant { .. }
                | TrustError::EmptyWindow { .. }
                | TrustError::Malformed { .. }
                | TrustError::UnexpectedType { .. }
                | TrustError::Hex { .. }
                | TrustError::FieldEmpty { .. }
                | TrustError::FieldTooLong { .. }
                | TrustError::UnprintableText { .. }
                | TrustError::TooMany { .. }
                | TrustError::DocumentSignatureInvalid { .. }
                | TrustError::UnsupportedFormat { .. }
                | TrustError::Inconsistent { .. }
                | TrustError::Duplicate { .. }
                | TrustError::Canonical(_)
                | TrustError::Key(_)
                | TrustError::Signature(_) => exit::USAGE,
            },
            Self::Io { .. } | Self::Audio(_) => exit::INPUT,
            Self::Sdk(inner) => match inner {
                audio_provenance_sdk::SdkError::Io { .. }
                | audio_provenance_sdk::SdkError::Audio(_)
                | audio_provenance_sdk::SdkError::CaptureInputTooLarge { .. } => exit::INPUT,
                audio_provenance_sdk::SdkError::RegistryUnavailable { .. }
                | audio_provenance_sdk::SdkError::RecoveryIncomplete { .. } => exit::INCOMPLETE,
                _ => exit::USAGE,
            },
            // An outage is not a usage error and not a verdict. Marking is refused rather than run
            // blind, and the operational code says the search could not finish.
            Self::LocatorCheckUnavailable { .. } => exit::INCOMPLETE,
            Self::Trace(inner) => match inner {
                TraceError::Unreadable { .. }
                | TraceError::InputTooLarge { .. }
                | TraceError::DurationTooLong { .. }
                | TraceError::UnrecognisedContainer
                | TraceError::Undecodable { .. } => exit::INPUT,
                TraceError::InvalidOption { .. }
                | TraceError::NullTestMalformed { .. }
                | TraceError::NullTestFixtureCodec
                | TraceError::NullTestNotRun
                | TraceError::FingerprintIndexMalformed { .. } => exit::USAGE,
            },
            Self::Entropy { .. } | Self::LocatorAllocation { .. } | Self::Serialise { .. } => {
                exit::INTERNAL
            }
        }
    }
}

impl CodedError for CliError {
    fn code(&self) -> &'static str {
        match self {
            Self::Usage(_) => "usage_invalid",
            Self::Io { .. } => "path_unreadable",
            Self::WouldOverwrite { .. } => "output_exists",
            Self::Entropy { .. } => "entropy_unavailable",
            Self::LocatorAllocation { .. } => "locator_allocation_failed",
            Self::LocatorCheckUnavailable { .. } => "locator_check_unavailable",
            Self::Serialise { .. } => "serialisation_failed",
            Self::Trace(inner) => inner.code(),
            Self::Trust(inner) => inner.code(),
            Self::Registry(inner) => inner.code(),
            Self::Resolve(inner) => inner.code(),
            Self::Config(inner) => inner.code(),
            Self::Manifest(inner) => inner.code(),
            Self::Key(inner) => inner.code(),
            Self::Signature(inner) => inner.code(),
            Self::Mark(inner) => inner.code(),
            Self::Audio(inner) => inner.code(),
            Self::Sdk(inner) => inner.code(),
        }
    }
}

/// Renders the error and everything under it, so an `io::Error` source is never swallowed.
pub fn describe(error: &CliError) -> String {
    let mut out = format!("{}: {error}", error.code());
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(error);
    while let Some(inner) = source {
        let text = inner.to_string();
        if !out.ends_with(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = inner.source();
    }
    out
}
