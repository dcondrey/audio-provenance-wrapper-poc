use alloc::string::{String, ToString};
use alloc::vec::Vec;

use audio_provenance_core::{
    GENOMARK_PAYLOAD_VERSION, KeyPossessionProof, PortableSignature, SignatureError,
    canonical_json, locator_from_signed_manifest, unsigned_manifest_view,
    verify_manifest_signature,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::RegistryError;
use crate::ids::{ContentHash, Fingerprint, MarkId, RecordId, SignedAt};

pub const RECORD_FORMAT: &str = "audio-provenance-registry-record-v1";

/// A 48-bit locator collides at a rate of order one across 2^24 registered works, so a mark
/// lookup returns every match. Beyond this many the response is refused rather than truncated.
pub const MAX_MARK_MATCHES: usize = 16;
pub const MAX_FINGERPRINT_CANDIDATES: usize = 256;

/// What the registry stores for one signed manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryRecord {
    record_id: RecordId,
    mark_id: MarkId,
    content_hash: ContentHash,
    fingerprint: Option<Fingerprint>,
    signed_at: SignedAt,
    signature: PortableSignature,
    manifest: Value,
    manifest_bytes: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RecordEnvelope {
    record_format: String,
    content_sha256: ContentHash,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fingerprint: Option<Fingerprint>,
    signed_at: SignedAt,
    manifest: Value,
}

impl RegistryRecord {
    /// Build a record from a manifest that has already been signed.
    ///
    /// Every key is a pure function of the manifest: the record id is SHA-256 over its canonical
    /// bytes, and the mark id comes from its own `$.mark` block and its own declared key and salt.
    ///
    /// IMPORTANT: the mark version and namespace are NOT parameters. Passing them in was the one
    /// way a caller could store a record whose namespace contradicts the manifest it points at,
    /// which GenoTrace's soft-binding check would then silently discard at verify time.
    pub fn from_signed_manifest(
        manifest: Value,
        content_hash: ContentHash,
        fingerprint: Option<Fingerprint>,
        signed_at: SignedAt,
    ) -> Result<Self, RegistryError> {
        let Value::Object(ref fields) = manifest else {
            return Err(RegistryError::ManifestNotObject);
        };
        let signature_value = fields
            .get("portable_signature")
            .ok_or(RegistryError::MissingSignatureBlock)?;
        let signature: PortableSignature = serde_json::from_value(signature_value.clone())
            .map_err(|error| RegistryError::SignatureBlock {
                reason: error.to_string(),
            })?;

        let manifest_bytes = canonical_json(&manifest)?;
        let reparsed: Value = serde_json::from_slice(&manifest_bytes)
            .map_err(|_| RegistryError::NoncanonicalManifest)?;
        if reparsed != manifest {
            return Err(RegistryError::NoncanonicalManifest);
        }

        let locator =
            locator_from_signed_manifest(&manifest).ok_or(RegistryError::MissingLocatorSalt)?;
        let record_id = RecordId::from_manifest_bytes(&manifest_bytes);
        let mark_id = MarkId::new(
            mark_field(&manifest, "version", GENOMARK_PAYLOAD_VERSION)?,
            mark_field(&manifest, "namespace", 0)?,
            locator,
        )?;

        Ok(Self {
            record_id,
            mark_id,
            content_hash,
            fingerprint,
            signed_at,
            signature,
            manifest,
            manifest_bytes,
        })
    }

    pub fn from_envelope_bytes(bytes: &[u8]) -> Result<Self, RegistryError> {
        let envelope: RecordEnvelope =
            serde_json::from_slice(bytes).map_err(|error| RegistryError::MalformedRecord {
                reason: error.to_string(),
            })?;
        if envelope.record_format != RECORD_FORMAT {
            return Err(RegistryError::RecordFormat {
                found: envelope.record_format,
                expected: RECORD_FORMAT,
            });
        }
        Self::from_signed_manifest(
            envelope.manifest,
            envelope.content_sha256,
            envelope.fingerprint,
            envelope.signed_at,
        )
    }

    pub fn to_envelope_bytes(&self) -> Result<Vec<u8>, RegistryError> {
        let envelope = RecordEnvelope {
            record_format: RECORD_FORMAT.to_string(),
            content_sha256: self.content_hash,
            fingerprint: self.fingerprint.clone(),
            signed_at: self.signed_at.clone(),
            manifest: self.manifest.clone(),
        };
        let value =
            serde_json::to_value(&envelope).map_err(|error| RegistryError::MalformedRecord {
                reason: error.to_string(),
            })?;
        Ok(canonical_json(&value)?)
    }

    pub const fn record_id(&self) -> RecordId {
        self.record_id
    }

    pub const fn mark_id(&self) -> MarkId {
        self.mark_id
    }

    pub const fn content_hash(&self) -> ContentHash {
        self.content_hash
    }

    pub const fn fingerprint(&self) -> Option<&Fingerprint> {
        self.fingerprint.as_ref()
    }

    pub const fn signed_at(&self) -> &SignedAt {
        &self.signed_at
    }

    /// The signer binding as the manifest declares it. A `PortableSignature` proves nothing on
    /// its own; `verify_key_possession` is what turns it into evidence.
    pub const fn signature(&self) -> &PortableSignature {
        &self.signature
    }

    pub const fn manifest(&self) -> &Value {
        &self.manifest
    }

    /// The exact bytes the record id covers and the signature was checked against.
    pub fn manifest_bytes(&self) -> &[u8] {
        &self.manifest_bytes
    }

    pub fn verify_key_possession(&self) -> Result<KeyPossessionProof, SignatureError> {
        let unsigned = unsigned_manifest_view(&self.manifest);
        verify_manifest_signature(&unsigned, &self.signature, None)
    }
}

fn mark_field(manifest: &Value, field: &'static str, default: u8) -> Result<u8, RegistryError> {
    let Some(value) = manifest.get("mark").and_then(|mark| mark.get(field)) else {
        return Ok(default);
    };
    let found = value.as_u64().ok_or(RegistryError::MarkField {
        field,
        found: u8::MAX,
    })?;
    u8::try_from(found)
        .ok()
        .filter(|value| *value <= 0x0f)
        .ok_or(RegistryError::MarkField {
            field,
            found: u8::try_from(found).unwrap_or(u8::MAX),
        })
}

/// One or more records sharing a 48-bit locator.
///
/// The type is non-empty by construction: a `Found` result that carried no record would be a
/// miss wearing a hit's clothes.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkMatches {
    head: RegistryRecord,
    rest: Vec<RegistryRecord>,
}

impl MarkMatches {
    pub fn new(records: Vec<RegistryRecord>) -> Result<Self, RegistryError> {
        let found = records.len();
        if found == 0 || found > MAX_MARK_MATCHES {
            return Err(RegistryError::MarkMatchCount {
                limit: MAX_MARK_MATCHES,
                found,
            });
        }
        let mut iter = records.into_iter();
        let head = iter.next().ok_or(RegistryError::MarkMatchCount {
            limit: MAX_MARK_MATCHES,
            found,
        })?;
        Ok(Self {
            head,
            rest: iter.collect(),
        })
    }

    pub const fn first(&self) -> &RegistryRecord {
        &self.head
    }

    pub fn len(&self) -> usize {
        1 + self.rest.len()
    }

    /// Never true. Present so `len()` does not draw a `clippy::len_without_is_empty`; the type
    /// cannot be empty.
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Two survivors mean `ambiguous_binding`, never a coin flip between them.
    pub fn is_ambiguous(&self) -> bool {
        !self.rest.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &RegistryRecord> {
        core::iter::once(&self.head).chain(self.rest.iter())
    }
}

/// A similarity a backend asserted.
///
/// IMPORTANT: a backend that can fabricate a score can fabricate a match, so this value is
/// advisory only. The caller re-derives similarity locally before any of it reaches a verdict;
/// the name says so at every use site.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct AdvisoryScore(f32);

impl AdvisoryScore {
    pub fn new(value: f32) -> Result<Self, RegistryError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(RegistryError::ScoreRange { found: value });
        }
        Ok(Self(value))
    }

    pub const fn advisory_value(self) -> f32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoredCandidate {
    record_id: RecordId,
    advisory_score: AdvisoryScore,
}

impl ScoredCandidate {
    pub const fn new(record_id: RecordId, advisory_score: AdvisoryScore) -> Self {
        Self {
            record_id,
            advisory_score,
        }
    }

    pub const fn record_id(&self) -> RecordId {
        self.record_id
    }

    pub const fn advisory_score(&self) -> AdvisoryScore {
        self.advisory_score
    }
}
