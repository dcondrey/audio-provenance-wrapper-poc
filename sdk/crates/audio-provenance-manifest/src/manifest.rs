use alloc::string::{String, ToString};
use alloc::vec::Vec;

use audio_provenance_core::{
    AssociationClaim, KeyPossessionProof, LocatorSalt, ObservationCoverage, PortableSignature,
    ProofLevel, VerificationStatus, canonical_json, locator_from_signed_manifest, sha256,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::binding::{AudioFingerprint, BindingOutcome, HardBinding};
use crate::error::ManifestError;
use crate::finding::Finding;
use crate::invariants::{APW_SCHEMA_ID, c2pa_validation_state_to_status};

pub const AUDIO_PROVENANCE_SCHEMA_ID: &str = "audio-provenance-manifest-v1";

pub use audio_provenance_core::LOCATOR_BYTES;

/// Which record family a set of bytes claims to be, and therefore what its signature covers.
///
/// A caller states this before admission rather than letting the document choose: what a signature
/// covers is not a property the signed document may assert about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestSchema {
    /// The audio-provenance POC's record. Written with `json.dump(indent=2)`, so the received bytes
    /// are NOT canonical and must never be byte-compared against `apw-json-sort-v1`.
    ApwV0,
    /// The Audio Provenance record. The registry's record id is SHA-256 over its canonical bytes, so
    /// admission asserts the received bytes already are those bytes.
    AudioProvenanceV1,
}

impl ManifestSchema {
    pub const fn id(self) -> &'static str {
        match self {
            Self::ApwV0 => APW_SCHEMA_ID,
            Self::AudioProvenanceV1 => AUDIO_PROVENANCE_SCHEMA_ID,
        }
    }

    pub const fn requires_canonical_bytes(self) -> bool {
        matches!(self, Self::AudioProvenanceV1)
    }
}

/// The Watermark fields a document CAN carry.
///
/// IMPORTANT: the locator is deliberately absent, and storing one here would be a second spelling
/// of the same fact. It is derived from the signer's public key and the record's `locator_salt`,
/// both already in the document, so a declared copy could only ever agree or contradict. Read it
/// from [`Manifest::mark_locator`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkBinding {
    pub version: u8,
    pub namespace: u8,
}

/// What a signature establishes about WHO signed.
///
/// The `identity` is non-null if and only if the level is `externally_verified`. That is enforced
/// by construction here rather than checked downstream: a valid self-generated signature proves key
/// possession, and there is no variant that can pair a name with a lesser level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignerIdentityBinding {
    KeyPossession { signer_id: String },
    ExternallyVerified { identity: String, authority: String },
}

impl SignerIdentityBinding {
    /// The ONLY caller is a trust resolver that has chained the signing key to a configured anchor.
    /// A manifest can never assert this about itself; nothing in this crate constructs it.
    pub fn externally_verified(identity: &str, authority: &str) -> Self {
        Self::ExternallyVerified {
            identity: identity.to_string(),
            authority: authority.to_string(),
        }
    }

    pub const fn proof_level(&self) -> ProofLevel {
        match self {
            Self::KeyPossession { .. } => ProofLevel::UnknownUnobserved,
            Self::ExternallyVerified { .. } => ProofLevel::ExternallyVerified,
        }
    }

    pub fn identity(&self) -> Option<&str> {
        match self {
            Self::KeyPossession { .. } => None,
            Self::ExternallyVerified { identity, .. } => Some(identity),
        }
    }

    pub fn authority(&self) -> Option<&str> {
        match self {
            Self::KeyPossession { .. } => None,
            Self::ExternallyVerified { authority, .. } => Some(authority),
        }
    }

    pub fn signer_id(&self) -> Option<&str> {
        match self {
            Self::KeyPossession { signer_id } => Some(signer_id),
            Self::ExternallyVerified { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceClaim {
    pub claim: String,
    pub value: Value,
    pub evidence: String,
    #[serde(rename = "apw:proof_level")]
    pub proof_level: ProofLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum C2paClaimStatus {
    Embedded,
    Sidecar,
    Unavailable,
}

/// The `c2pa_claim` section of a POC manifest, reduced to what a verifier acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct C2paClaimRecord {
    status: C2paClaimStatus,
    reason: Option<String>,
    validation_state: Option<VerificationStatus>,
    binding_covers_export: bool,
}

impl C2paClaimRecord {
    pub const fn status(&self) -> C2paClaimStatus {
        self.status
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// The POC's `c2pa_claim.validation.state` in Audio Provenance's vocabulary. See
    /// [`crate::invariants::C2PA_VALIDATION_STATES`] for the table.
    pub const fn verification_status(&self) -> Option<VerificationStatus> {
        self.validation_state
    }

    /// Whether the C2PA hard binding covers the same bytes the manifest hashed as its export.
    pub const fn binding_covers_export(&self) -> bool {
        self.binding_covers_export
    }
}

/// A signature-verified provenance record.
///
/// THE ONLY WAY TO OBTAIN ONE is [`crate::UnverifiedManifest::admit`]. There is no public
/// constructor, no `Deserialize`, and no conversion from the unverified type, so every accessor
/// below is reachable only after the Ed25519 signature verified over the canonical bytes and the
/// invariant port produced no error. That is the whole point of the module: a reviewer can grep for
/// `UnverifiedManifest`, see that `admit` is its only exit, and know the guarantee is total.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub(crate) bytes: Vec<u8>,
    pub(crate) value: Value,
    pub(crate) schema: ManifestSchema,
    pub(crate) signature: PortableSignature,
    pub(crate) signer: SignerIdentityBinding,
    pub(crate) hard_binding: Option<HardBinding>,
    pub(crate) fingerprint: Option<AudioFingerprint>,
    pub(crate) mark: Option<MarkBinding>,
    pub(crate) locator_salt: Option<LocatorSalt>,
    pub(crate) signed_at: Option<String>,
    pub(crate) association: Option<AssociationClaim>,
    pub(crate) coverage: Option<ObservationCoverage>,
    pub(crate) claims: Vec<ProvenanceClaim>,
    pub(crate) c2pa_claim: Option<C2paClaimRecord>,
    pub(crate) findings: Vec<Finding>,
}

impl Manifest {
    /// Exactly the bytes admission received and the signature was checked against.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn value(&self) -> &Value {
        &self.value
    }

    pub const fn schema(&self) -> ManifestSchema {
        self.schema
    }

    pub const fn signature(&self) -> &PortableSignature {
        &self.signature
    }

    pub const fn signer(&self) -> &SignerIdentityBinding {
        &self.signer
    }

    pub const fn hard_binding(&self) -> Option<&HardBinding> {
        self.hard_binding.as_ref()
    }

    pub const fn fingerprint(&self) -> Option<&AudioFingerprint> {
        self.fingerprint.as_ref()
    }

    pub const fn mark(&self) -> Option<&MarkBinding> {
        self.mark.as_ref()
    }

    /// The 16 signed bytes the Watermark locator is derived from. Required on a Audio Provenance record;
    /// absent on a POC record that predates the field.
    pub const fn locator_salt(&self) -> Option<LocatorSalt> {
        self.locator_salt
    }

    pub fn signed_at(&self) -> Option<&str> {
        self.signed_at.as_deref()
    }

    pub const fn association(&self) -> Option<&AssociationClaim> {
        self.association.as_ref()
    }

    pub const fn observation_coverage(&self) -> Option<&ObservationCoverage> {
        self.coverage.as_ref()
    }

    pub fn claims(&self) -> &[ProvenanceClaim] {
        &self.claims
    }

    pub const fn c2pa_claim(&self) -> Option<&C2paClaimRecord> {
        self.c2pa_claim.as_ref()
    }

    /// Non-error findings raised during admission. Errors never reach this type; they fail
    /// admission instead.
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// SHA-256 over the manifest's canonical bytes, which is `audio-provenance-registry`'s record id.
    pub fn record_digest(&self) -> Result<[u8; 32], ManifestError> {
        Ok(sha256(&canonical_json(&self.value)?))
    }

    /// The Watermark locator this record answers to: `sha256(domain || 0x00 || public key || salt)`
    /// truncated to 48 bits, over the document's OWN declared key and salt. Re-deriving it is what
    /// detects a registry serving a manifest that does not claim the mark's locator. `None` when
    /// the document carries no usable salt, which is a mismatch rather than a pass.
    pub fn mark_locator(&self) -> Option<[u8; LOCATOR_BYTES]> {
        locator_from_signed_manifest(&self.value)
    }

    /// Recomputes the hard binding against the digest of the audio actually presented.
    ///
    /// Returns [`BindingOutcome::Uncoverable`] when the manifest declares no hard binding, which is
    /// the case a soft binding may substitute for. A binding that is present and MISMATCHED is
    /// never overridden by a soft one.
    pub fn evaluate_hard_binding(&self, observed_content_sha256: &str) -> BindingOutcome {
        self.hard_binding
            .as_ref()
            .map_or(BindingOutcome::Uncoverable, |binding| {
                binding.evaluate_content(observed_content_sha256)
            })
    }

    /// What the verified signature establishes: key possession, never identity.
    pub fn key_possession(&self) -> Result<KeyPossessionProof, ManifestError> {
        let unsigned = audio_provenance_core::unsigned_manifest_view(&self.value);
        Ok(audio_provenance_core::verify_manifest_signature(
            &unsigned,
            &self.signature,
            None,
        )?)
    }
}

pub(crate) fn read_c2pa_claim(value: &Value) -> Option<C2paClaimRecord> {
    let claim = value.get("c2pa_claim")?.as_object()?;
    let status = match claim.get("status").and_then(Value::as_str)? {
        "embedded" => C2paClaimStatus::Embedded,
        "sidecar" => C2paClaimStatus::Sidecar,
        "unavailable" => C2paClaimStatus::Unavailable,
        _ => return None,
    };
    Some(C2paClaimRecord {
        status,
        reason: claim
            .get("reason")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        validation_state: claim
            .get("validation")
            .and_then(|v| v.get("state"))
            .and_then(Value::as_str)
            .and_then(c2pa_validation_state_to_status),
        binding_covers_export: claim.get("source_sha256_matches_export")
            == Some(&Value::Bool(true)),
    })
}

pub(crate) fn read_claims(value: &Value) -> Vec<ProvenanceClaim> {
    value
        // `audio-provenance-v1` writes `claims`; the capture interop family calls the same shape
        // `claim_summary`. Reading only the latter made SDK-authored claims signature-covered but
        // invisible to every public accessor.
        .get("claims")
        .or_else(|| value.get("claim_summary"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}
