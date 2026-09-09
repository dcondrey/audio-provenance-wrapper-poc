use alloc::string::ToString;
use alloc::vec::Vec;

use audio_provenance_core::{
    LocatorSalt, ObservationCoverage, PortableSignature, canonical_json, parse_signing_input,
    unsigned_manifest_view, verify_manifest_signature,
};
use serde_json::Value;

use crate::binding::{AudioFingerprint, HardBinding, require_calendar_date};
use crate::error::{MAX_MANIFEST_BYTES, ManifestError};
use crate::finding::{Finding, error_messages, has_errors};
use crate::invariants::{validate_apw_invariants, validate_audio_provenance_invariants};
use crate::manifest::{
    Manifest, ManifestSchema, MarkBinding, SignerIdentityBinding, read_c2pa_claim, read_claims,
};

/// Bytes that parsed as JSON and NOTHING MORE.
///
/// No accessor here reports a binding, a claim, a coverage status or an identity, and there is no
/// conversion into [`Manifest`] other than [`Self::admit`]. Grepping this type is how a reviewer
/// confirms the invariant is total: everything it exposes is either the raw bytes the caller
/// already held or the signature envelope, which is what [`Self::admit`] is about to check.
#[derive(Debug, Clone, PartialEq)]
pub struct UnverifiedManifest {
    bytes: Vec<u8>,
    value: Value,
}

#[derive(Debug)]
pub struct AdmissionFailure {
    pub error: ManifestError,
    pub findings: Vec<Finding>,
    pub manifest: UnverifiedManifest,
}

impl UnverifiedManifest {
    /// IMPORTANT: parses through [`parse_signing_input`], never `serde_json::from_slice`. An
    /// integer literal wider than 64 bits collapses distinct documents onto identical canonical
    /// bytes, and those bytes are the signed content.
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge {
                limit: MAX_MANIFEST_BYTES,
                found: bytes.len(),
            });
        }
        let value = parse_signing_input(bytes)?;
        if !value.is_object() {
            return Err(ManifestError::NotAnObject);
        }
        Ok(Self {
            bytes: bytes.to_vec(),
            value,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The `schema` string the document claims. It is UNTRUSTED and does not select the admission
    /// path; a caller compares it against the [`ManifestSchema`] it intends to admit.
    pub fn declared_schema(&self) -> Option<&str> {
        self.value.get("schema").and_then(Value::as_str)
    }

    /// The signature envelope, unchecked. Exposed because Trace records it on a candidate it
    /// could not admit; it carries no bound data.
    pub fn signature_block(&self) -> Option<&Value> {
        self.value.get("portable_signature")
    }

    /// The one exit from the unverified type.
    ///
    /// Order matters and follows TRACE stage 2: canonicality (where it applies), then the
    /// signature, and only then the schema invariants. Nothing bound is read before the signature
    /// check, because everything that reads bound data lives on [`Manifest`].
    pub fn admit(self, schema: ManifestSchema) -> Result<Manifest, AdmissionFailure> {
        let mut findings = Vec::new();
        match self.admit_inner(schema, &mut findings) {
            Ok(manifest) => Ok(manifest),
            Err(error) => Err(AdmissionFailure {
                error,
                findings,
                manifest: self,
            }),
        }
    }

    fn admit_inner(
        &self,
        schema: ManifestSchema,
        findings: &mut Vec<Finding>,
    ) -> Result<Manifest, ManifestError> {
        let declared = self.declared_schema().unwrap_or_default();
        if declared != schema.id() {
            return Err(ManifestError::SchemaMismatch {
                expected: schema.id(),
                found: declared.to_string(),
            });
        }

        // IMPORTANT: only the Audio Provenance family. The POC writes its manifests with
        // `json.dump(indent=2)` and signs `canonical_json` of the value it parsed back, so its
        // received bytes are never canonical and asserting otherwise would reject every real one.
        if schema.requires_canonical_bytes() && canonical_json(&self.value)? != self.bytes {
            return Err(ManifestError::NoncanonicalManifest);
        }

        let signature_value = self
            .value
            .get("portable_signature")
            .filter(|v| !v.is_null())
            .ok_or(ManifestError::MissingField {
                field: "portable_signature",
            })?;
        let signature: PortableSignature = serde_json::from_value(signature_value.clone())
            .map_err(|error| ManifestError::SignatureBlock {
                reason: error.to_string(),
            })?;

        let proof =
            verify_manifest_signature(&unsigned_manifest_view(&self.value), &signature, None)?;

        findings.extend(match schema {
            ManifestSchema::ApwV0 => validate_apw_invariants(&self.value),
            ManifestSchema::AudioProvenanceV1 => validate_audio_provenance_invariants(&self.value),
        });
        if has_errors(findings) {
            return Err(ManifestError::from_findings(error_messages(findings)));
        }

        let (hard_binding, fingerprint, mark, locator_salt, signed_at) = match schema {
            ManifestSchema::ApwV0 => (
                read_apw_binding(&self.value)?,
                None,
                None,
                None,
                apw_signed_at(&self.value),
            ),
            ManifestSchema::AudioProvenanceV1 => (
                Some(read_audio_provenance_binding(&self.value)?),
                read_audio_provenance_fingerprint(&self.value)?,
                read_mark(&self.value)?,
                Some(read_locator_salt(&self.value)?),
                Some(read_signed_at(&self.value)?),
            ),
        };

        Ok(Manifest {
            bytes: self.bytes.clone(),
            value: self.value.clone(),
            schema,
            signature,
            signer: SignerIdentityBinding::KeyPossession {
                signer_id: proof.signer_id().to_string(),
            },
            hard_binding,
            fingerprint,
            mark,
            locator_salt,
            signed_at,
            association: read_section(&self.value, "stem_export_association", findings),
            coverage: read_coverage(&self.value, findings),
            claims: read_claims(&self.value),
            c2pa_claim: read_c2pa_claim(&self.value),
            findings: core::mem::take(findings),
        })
    }
}

/// A vocabulary section that fails to deserialize is a WARNING, never an admission failure.
///
/// `audio-provenance-core`'s `AssociationClaim` and `ObservationCoverage` derive their proof level and
/// status from evidence and refuse a document that disagrees. Their rules and `daemon/schema.py`'s
/// have drifted apart (see `coverage_conjunction_divergence`), so letting one reject a manifest the
/// ported invariants accepted would make this crate stricter than the prior art it interoperates
/// with. The section is reported absent and the reason is recorded.
fn read_section<T: serde::de::DeserializeOwned>(
    value: &Value,
    field: &'static str,
    findings: &mut Vec<Finding>,
) -> Option<T> {
    let section = value.get(field)?;
    match serde_json::from_value(section.clone()) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            findings.push(Finding::warning(
                "vocabulary_section_unreadable",
                alloc::format!("$.{field}"),
                alloc::format!("{field} could not be read as a typed claim: {error}"),
            ));
            None
        }
    }
}

/// IMPORTANT: a document declaring `unknown_coverage` is taken at its word and its counters are
/// discarded. `audio-provenance-core` derives the status from the counters, and an all-zero counter block
/// satisfies its conjunction vacuously, so a session that observed NOTHING would otherwise derive
/// `complete_observed_path`. `daemon/schema.py` never runs that conjunction unless the manifest
/// claims the strong status, and reading a weaker claim than the evidence would support is the only
/// direction that is safe.
fn read_coverage(value: &Value, findings: &mut Vec<Finding>) -> Option<ObservationCoverage> {
    let section = value.get("observation_coverage")?;
    if section.get("status").and_then(Value::as_str) == Some("unknown_coverage") {
        return Some(ObservationCoverage::unknown());
    }
    read_section(value, "observation_coverage", findings)
}

fn read_apw_binding(value: &Value) -> Result<Option<HardBinding>, ManifestError> {
    let Some(export) = value.get("export").and_then(Value::as_object) else {
        return Ok(None);
    };
    let Some(digest) = export.get("sha256").and_then(Value::as_str) else {
        return Ok(None);
    };
    Ok(Some(HardBinding::new(
        digest,
        export.get("file_size_bytes").and_then(Value::as_u64),
        None,
    )?))
}

fn apw_signed_at(value: &Value) -> Option<alloc::string::String> {
    let created = value.get("created_at")?.as_str()?;
    require_calendar_date(created.get(..10)?, "created_at").ok()
}

fn read_audio_provenance_binding(value: &Value) -> Result<HardBinding, ManifestError> {
    let binding = value.get("hard_binding").and_then(Value::as_object).ok_or(
        ManifestError::MissingField {
            field: "hard_binding",
        },
    )?;
    let content = binding
        .get("content_sha256")
        .and_then(Value::as_str)
        .ok_or(ManifestError::MissingField {
            field: "hard_binding.content_sha256",
        })?;
    HardBinding::new(
        content,
        binding.get("content_bytes").and_then(Value::as_u64),
        binding.get("decoded_audio_sha256").and_then(Value::as_str),
    )
}

fn read_audio_provenance_fingerprint(
    value: &Value,
) -> Result<Option<AudioFingerprint>, ManifestError> {
    let Some(fingerprint) = value.get("fingerprint").and_then(Value::as_object) else {
        return Ok(None);
    };
    let algorithm = fingerprint.get("algorithm").and_then(Value::as_str).ok_or(
        ManifestError::MissingField {
            field: "fingerprint.algorithm",
        },
    )?;
    let digest =
        fingerprint
            .get("digest")
            .and_then(Value::as_str)
            .ok_or(ManifestError::MissingField {
                field: "fingerprint.digest",
            })?;
    Ok(Some(AudioFingerprint::new(algorithm, digest)?))
}

fn read_mark(value: &Value) -> Result<Option<MarkBinding>, ManifestError> {
    let Some(mark) = value.get("mark") else {
        return Ok(None);
    };
    serde_json::from_value(mark.clone())
        .map(Some)
        .map_err(|_| ManifestError::FieldType {
            field: "mark",
            expected: "an object carrying byte-valued version and namespace",
        })
}

/// Runs AFTER `validate_audio_provenance_invariants`, which already refused a missing or malformed salt,
/// so the error arms here are unreachable through `admit` and exist to keep the reader honest.
fn read_locator_salt(value: &Value) -> Result<LocatorSalt, ManifestError> {
    let text =
        value
            .get("locator_salt")
            .and_then(Value::as_str)
            .ok_or(ManifestError::MissingField {
                field: "locator_salt",
            })?;
    LocatorSalt::parse_hex(text).map_err(|_| ManifestError::MalformedDigest {
        field: "locator_salt",
    })
}

fn read_signed_at(value: &Value) -> Result<alloc::string::String, ManifestError> {
    let text = value
        .get("signed_at")
        .and_then(Value::as_str)
        .ok_or(ManifestError::MissingField { field: "signed_at" })?;
    require_calendar_date(text, "signed_at")
}
