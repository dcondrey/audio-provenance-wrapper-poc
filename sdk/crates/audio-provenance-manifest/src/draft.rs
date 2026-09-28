use alloc::string::{String, ToString};
use alloc::vec::Vec;

use audio_provenance_core::{
    AssociationClaim, LocatorSalt, ObservationCoverage, PortableSignature, canonical_json,
};
use serde_json::{Map, Value};

use crate::binding::{AudioFingerprint, HardBinding, require_calendar_date};
use crate::error::ManifestError;
use crate::manifest::{AUDIO_PROVENANCE_SCHEMA_ID, MarkBinding, ProvenanceClaim};

/// A Audio Provenance record under construction: everything a signer needs and nothing it may not assert.
///
/// There is no signer-identity field. Identity is resolved by a trust store at verification time,
/// never declared by the document, so a draft cannot write one in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ManifestDraft {
    signed_at: Option<String>,
    hard_binding: Option<HardBinding>,
    fingerprint: Option<AudioFingerprint>,
    mark: Option<MarkBinding>,
    locator_salt: Option<LocatorSalt>,
    association: Option<AssociationClaim>,
    coverage: Option<ObservationCoverage>,
    claims: Vec<ProvenanceClaim>,
}

impl ManifestDraft {
    pub fn new(signed_at: &str, hard_binding: HardBinding) -> Result<Self, ManifestError> {
        Ok(Self {
            signed_at: Some(require_calendar_date(signed_at, "signed_at")?),
            hard_binding: Some(hard_binding),
            ..Self::default()
        })
    }

    #[must_use]
    pub fn with_fingerprint(mut self, fingerprint: AudioFingerprint) -> Self {
        self.fingerprint = Some(fingerprint);
        self
    }

    #[must_use]
    pub const fn with_mark(mut self, mark: MarkBinding) -> Self {
        self.mark = Some(mark);
        self
    }

    /// The salt the Watermark locator was allocated from. REQUIRED: a Audio Provenance record that does not
    /// carry one names no locator, so no mark can ever resolve it.
    #[must_use]
    pub const fn with_locator_salt(mut self, salt: LocatorSalt) -> Self {
        self.locator_salt = Some(salt);
        self
    }

    #[must_use]
    pub const fn with_association(mut self, association: AssociationClaim) -> Self {
        self.association = Some(association);
        self
    }

    #[must_use]
    pub const fn with_coverage(mut self, coverage: ObservationCoverage) -> Self {
        self.coverage = Some(coverage);
        self
    }

    #[must_use]
    pub fn with_claim(mut self, claim: ProvenanceClaim) -> Self {
        self.claims.push(claim);
        self
    }

    /// The value a signer canonicalises and signs. It carries no signature block, which is exactly
    /// the view `verify_manifest_signature` recomputes.
    pub fn to_unsigned_value(&self) -> Result<Value, ManifestError> {
        let binding = self
            .hard_binding
            .as_ref()
            .ok_or(ManifestError::MissingField {
                field: "hard_binding",
            })?;
        let signed_at = self
            .signed_at
            .as_ref()
            .ok_or(ManifestError::MissingField { field: "signed_at" })?;
        let locator_salt = self.locator_salt.ok_or(ManifestError::MissingField {
            field: "locator_salt",
        })?;

        let mut binding_map = Map::new();
        binding_map.insert("algorithm".into(), Value::String("sha256".into()));
        binding_map.insert(
            "content_sha256".into(),
            Value::String(binding.content_sha256().to_string()),
        );
        if let Some(bytes) = binding.content_bytes() {
            binding_map.insert("content_bytes".into(), Value::from(bytes));
        }
        if let Some(decoded) = binding.decoded_audio_sha256() {
            binding_map.insert(
                "decoded_audio_sha256".into(),
                Value::String(decoded.to_string()),
            );
        }

        let mut root = Map::new();
        root.insert(
            "schema".into(),
            Value::String(AUDIO_PROVENANCE_SCHEMA_ID.into()),
        );
        root.insert("signed_at".into(), Value::String(signed_at.clone()));
        root.insert("hard_binding".into(), Value::Object(binding_map));
        root.insert("locator_salt".into(), Value::String(locator_salt.to_hex()));
        root.insert("claims".into(), to_value(&self.claims)?);
        if let Some(fingerprint) = &self.fingerprint {
            let mut map = Map::new();
            map.insert(
                "algorithm".into(),
                Value::String(fingerprint.algorithm().to_string()),
            );
            map.insert(
                "digest".into(),
                Value::String(fingerprint.digest_hex().to_string()),
            );
            root.insert("fingerprint".into(), Value::Object(map));
        }
        if let Some(mark) = &self.mark {
            root.insert("mark".into(), to_value(mark)?);
        }
        if let Some(association) = &self.association {
            root.insert("stem_export_association".into(), to_value(association)?);
        }
        if let Some(coverage) = &self.coverage {
            root.insert("observation_coverage".into(), to_value(coverage)?);
        }
        Ok(Value::Object(root))
    }

    /// Attaches a signature produced over [`Self::to_unsigned_value`] and returns the CANONICAL
    /// bytes of the complete record, which is the form `UnverifiedManifest::admit` requires for the
    /// Audio Provenance family and the form the registry hashes into a record id.
    pub fn seal(&self, signature: &PortableSignature) -> Result<Vec<u8>, ManifestError> {
        let mut value = self.to_unsigned_value()?;
        if let Value::Object(map) = &mut value {
            map.insert("portable_signature".into(), to_value(signature)?);
        }
        Ok(canonical_json(&value)?)
    }
}

/// Attaches `signature` to an unsigned manifest value and returns the canonical record bytes.
///
/// For a caller that holds only the value a signer already signed (a two-call remote signing flow)
/// and not the [`ManifestDraft`] that produced it. Nothing here checks the signature; admission of
/// the returned bytes does.
pub fn seal_unsigned_value(
    unsigned: &Value,
    signature: &PortableSignature,
) -> Result<Vec<u8>, ManifestError> {
    let mut value = unsigned.clone();
    match &mut value {
        Value::Object(map) => {
            map.insert("portable_signature".into(), to_value(signature)?);
        }
        _ => {
            return Err(ManifestError::SignatureBlock {
                reason: "an unsigned manifest must be a JSON object".to_string(),
            });
        }
    }
    Ok(canonical_json(&value)?)
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, ManifestError> {
    serde_json::to_value(value).map_err(|error| ManifestError::SignatureBlock {
        reason: error.to_string(),
    })
}
