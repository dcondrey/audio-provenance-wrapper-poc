//! Stage 2: from received bytes to an admitted manifest, or to the reason it did not become one.
//!
//! `audio_provenance_manifest::UnverifiedManifest::admit` does the canonicality assert, the signature check
//! and the invariant port, in that order, and is the only exit to a `Manifest`. This module's job
//! is to classify a failure onto the status mapping's axes without collapsing distinct reasons.

use audio_provenance_core::CodedError;
use audio_provenance_manifest::{
    AdmissionFailure, Finding, Manifest, ManifestError, ManifestSchema, Severity,
    UnverifiedManifest,
};

use crate::status::{RejectionReason, SignatureOutcome};

/// What Stage 2 concluded about one set of received bytes.
#[derive(Debug)]
pub enum Admission {
    Admitted {
        manifest: Box<Manifest>,
    },
    /// The bytes never reached a binding evaluation. The status mapping treats this as
    /// `CandidateRejected` or as a signature axis failure depending on which.
    Rejected {
        rejection: Option<RejectionReason>,
        signature: SignatureOutcome,
        code: &'static str,
        detail: String,
        findings: Vec<Finding>,
    },
    /// Not a candidate at all: the bytes are not a manifest the ladder can even attempt. Rung 1
    /// records `embedded_unparseable` and continues; forcing a downgrade gains an attacker nothing,
    /// because every lower rung's ceiling is at most what this rung could have given.
    Unparseable {
        code: &'static str,
        detail: String,
    },
}

/// Parses and admits under both schemas.
///
/// The DECLARED schema does not select the admission path; it is untrusted text. Each supported
/// schema is attempted and the first that admits wins, so a manifest cannot pick a laxer validator
/// by lying about what it is.
pub fn admit(bytes: &[u8]) -> Admission {
    let unverified = match UnverifiedManifest::parse(bytes) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Admission::Unparseable {
                code: error.code(),
                detail: error.to_string(),
            };
        }
    };

    let mut last: Option<AdmissionFailure> = None;
    for schema in [ManifestSchema::AudioProvenanceV1, ManifestSchema::ApwV0] {
        match unverified.clone().admit(schema) {
            Ok(manifest) => {
                return Admission::Admitted {
                    manifest: Box::new(manifest),
                };
            }
            Err(failure) => {
                // A schema mismatch says only "not this one". Any other failure is a real finding
                // about the manifest and is the one worth reporting.
                let informative = !matches!(failure.error, ManifestError::SchemaMismatch { .. });
                if informative || last.is_none() {
                    last = Some(failure);
                }
            }
        }
    }

    match last {
        Some(failure) => classify(failure),
        None => Admission::Unparseable {
            code: "manifest_schema_mismatch",
            detail: "bytes declare no schema this build admits".to_string(),
        },
    }
}

fn classify(failure: AdmissionFailure) -> Admission {
    let code = failure.error.code();
    let detail = failure.error.to_string();
    let findings = failure
        .findings
        .into_iter()
        .filter(|finding| finding.severity() != Severity::Info)
        .collect();

    let (rejection, signature) = match &failure.error {
        ManifestError::NoncanonicalManifest => (
            Some(RejectionReason::NoncanonicalManifest),
            SignatureOutcome::Valid,
        ),
        ManifestError::Invariants { .. } => (
            Some(RejectionReason::InvariantsFailed),
            SignatureOutcome::Valid,
        ),
        ManifestError::MissingField {
            field: "portable_signature",
        } => (None, SignatureOutcome::Absent),
        ManifestError::SignatureBlock { .. } => (None, SignatureOutcome::UnsupportedAlgorithm),
        ManifestError::Signature(_) => (None, SignatureOutcome::Invalid),
        // Everything else is a structural defect discovered before the signature could be judged.
        // Reporting it as a valid signature would be a round-up, so it is an invalid one.
        ManifestError::TooLarge { .. }
        | ManifestError::Canonical(_)
        | ManifestError::NotAnObject
        | ManifestError::SchemaMismatch { .. }
        | ManifestError::MissingField { .. }
        | ManifestError::FieldType { .. }
        | ManifestError::MalformedDigest { .. }
        | ManifestError::MalformedDate { .. } => (None, SignatureOutcome::Invalid),
    };

    Admission::Rejected {
        rejection,
        signature,
        code,
        detail,
        findings,
    }
}
