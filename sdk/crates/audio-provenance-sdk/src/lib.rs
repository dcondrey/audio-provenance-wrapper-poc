//! Audio Provenance: audio provenance you can verify from the file alone.
//!
//! ```no_run
//! use std::path::Path;
//! use audio_provenance_sdk::{VerifyOptions, verify};
//!
//! let result = verify(Path::new("filename.wav"), &VerifyOptions::new())?;
//! result.status;      // Verified | Changed | Untrusted | NotFound
//! result.identity;    // Some("Signal Room Studios"), or None
//! result.signed_at;   // Some("2026-03-14")
//! result.r#match;     // 1.0
//! # Ok::<(), audio_provenance_sdk::SdkError>(())
//! ```
//!
//! # `identity` is usually `None`, and that is correct
//!
//! `identity` is `Some` if and only if a configured trust anchor resolved the signing key to a
//! name. A valid self-generated Ed25519 signature proves KEY POSSESSION and says nothing about who
//! holds the key, so the common case for an unanchored signer is `identity: None` with status
//! `untrusted`, even though the signature is cryptographically perfect. An SDK that printed a
//! self-asserted name as a verified identity would be worse than one that printed nothing.
//!
//! # An outage is not a verdict
//!
//! `not_found` means "nothing was recovered", and a caller reads it as a fact about the work. When
//! a rung could not run, that reading is unearned, so [`verify`] returns
//! [`SdkError::RegistryUnavailable`] rather than a `not_found` resting on a dropped connection. A
//! definite verdict is unaffected: `verified`, `changed` and `untrusted` rest on a candidate that
//! was actually admitted and are returned with `incomplete` set if a lower rung faulted.
//!
//! # Acoustic re-recording is unsupported
//!
//! Audio played through a loudspeaker and captured by a microphone returns `not_found`. The failure
//! is structural, not a tuning deficit, and [`capabilities`] reports it as the literal string
//! `"unsupported"`, never `null` and never absent. Do not describe a Audio Provenance mark as surviving
//! playback in a room, a phone recording, or over-the-air capture.
//!
//! # `match == 1.0` is an exact test for a hard binding
//!
//! Every soft basis caps at `0.99`. A soft binding also cannot reach `verified` at all until a
//! `audio-provenance-bench` null-test report supplies the false-positive rate the result must publish; see
//! [`VerifyOptions::with_null_test_report`].
//!
//! # The four statuses come from Trace
//!
//! This crate defines no status of its own. [`audio_provenance_core::VerificationStatus`] is the vocabulary,
//! the mapping lives in `apw_trace::status`, and [`VerifyResult`] serialises as the CLI's `--json`
//! output verbatim rather than through a second hand-built shape.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod capabilities;
pub mod capture_adapter;
#[cfg(feature = "http")]
pub mod cloudflare_signer;
pub mod error;
pub mod producer;
pub mod verify;

mod riff;

pub use capabilities::{
    ACOUSTIC_RERECORDING, Capabilities, FingerprintCapabilities, MarkCapabilities, capabilities,
};
pub use capture_adapter::{
    CaptureAdapterOptions, CaptureAdapterResult, adapt_capture_export,
    write_capture_adapter_receipt,
};
#[cfg(feature = "http")]
pub use cloudflare_signer::{CloudflareAuth, CloudflareSigner, RemoteSigner, SignerError};
pub use error::SdkError;
pub use producer::{
    EmbedOptions, EmbedResult, MarkPlan, PublishReceipt, SidecarOutput, SignOptions, SignResult,
    embed, open_local_registry, publish, sign,
};
pub use verify::{VerifyOptions, inspect, verify, verify_bytes};

pub use audio_provenance_audio::{AudioBuffer, BitDepth};
pub use audio_provenance_core::{
    CodedError, LocatorSalt, ManifestSigner, ProofLevel, SigningKey, VerificationStatus,
    signing::fs::load_signing_key,
};
pub use audio_provenance_manifest::{
    AudioFingerprint, Finding, HardBinding, Manifest, ManifestDraft, MarkBinding, ProvenanceClaim,
    Severity,
};
pub use audio_provenance_registry::{
    HttpRegistryAuth, HttpRegistryBackend, HttpRegistryOptions, LocalRegistryBackend, Lookup,
    RegistryBackend, RegistryKind, RegistryRecord, RegistrySource, WritableRegistryBackend,
};
pub use apw_watermark::{Watermark, Payload};
pub use apw_trace::result::FindingReport;
pub use apw_trace::{
    BindingKind, BindingReport, DEFAULT_SOFT_BINDING_THRESHOLD, FileTrustStore, InspectReport,
    MatchBasis, NoTrustAnchors, NullTestTable, RecoveryMethod, RecoveryReport, RecoveryStep,
    SidecarPolicy, SignatureReport, StepOutcome, TrustAnchor, TrustResolution, TrustStore,
    VerifyResult,
};
