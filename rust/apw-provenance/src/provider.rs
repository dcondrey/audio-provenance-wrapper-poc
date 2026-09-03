use std::path::Path;

use apw_core::{ProofLevel, VerificationState};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::error::Result;

/// Identity, soft binding, registry and verification behind one vendor-neutral seam.
///
/// The capability groups this models: key custody (`identity`,
/// `issue_signing_material`, `sign_claim`, `revoke`, `signing_history`), a soft
/// binding (`embed_mark`, `recover_mark`), a manifest registry (`register`) and a
/// verifier (`verify`).
///
/// IMPORTANT: no implementation of this trait may run on the audio thread.
pub trait ProvenanceProvider: Send + Sync {
    /// Describe the active signing identity and how strongly it is established.
    ///
    /// REQUIRED: a locally generated identity is never stronger than
    /// [`ProofLevel::UserDeclared`]. Possession of a key is not verified identity.
    fn identity(&self) -> Result<IdentityReport>;

    fn issue_signing_material(&self) -> Result<SigningMaterial>;

    /// Sign claim bytes with the active key and record the act in signing history.
    fn sign_claim(&self, payload: &[u8]) -> Result<Vec<u8>>;

    /// Attach a soft binding to an asset and return what was attached.
    ///
    /// REQUIRED: the returned value states the binding's actual mechanism and its
    /// measured limits. An implementation that has not measured perceptual
    /// transparency or transcode survival MUST NOT claim either.
    fn embed_mark(&self, audio_path: &Path, payload: &Value) -> Result<MarkAttachment>;

    /// Recover a previously attached soft binding, or `None` if absent.
    ///
    /// IMPORTANT: `None` means this provider found no mark. It is never evidence
    /// that the asset is synthetic.
    fn recover_mark(&self, audio_path: &Path) -> Result<Option<RecoveredMark>>;

    fn register(&self, manifest: &Value) -> Result<RegistryReceipt>;

    fn verify(&self, asset_path: &Path, manifest_bytes: Option<&[u8]>) -> Result<VerificationOutcome>;

    /// Revoke a key.
    ///
    /// REQUIRED: revocation marks the key unusable for new signatures and MUST
    /// retain every prior signing record. Erasing what a creator signed is a
    /// different operation and is not offered by this interface.
    fn revoke(&self, key_id: &str) -> Result<RevocationRecord>;

    /// Every signing record for a key, revoked or not.
    fn signing_history(&self, key_id: &str) -> Result<Vec<SigningRecord>>;
}

/// The signing identity and how strongly the provider established it.
///
/// IMPORTANT: `key_id` identifies a key, not a person. `identity_evidence` is the
/// only field that says how (or whether) the subject name was attested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigningIdentity {
    pub key_id: String,
    pub subject_common_name: String,
    pub algorithm: String,
    pub identity_evidence: String,
    pub revoked: bool,
    pub created_at_ms: i64,
    #[serde(rename = "apw:proof_level")]
    pub proof_level: ProofLevel,
}

/// What `identity()` publishes: the identity plus the anchor it chains to and the
/// limits of the claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityReport {
    pub identity: SigningIdentity,
    pub trust_anchor_sha256: String,
    pub limits: String,
}

impl Serialize for IdentityReport {
    fn serialize<S: Serializer>(&self, serializer: S) -> core::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(9))?;
        map.serialize_entry("key_id", &self.identity.key_id)?;
        map.serialize_entry("subject_common_name", &self.identity.subject_common_name)?;
        map.serialize_entry("algorithm", &self.identity.algorithm)?;
        map.serialize_entry("identity_evidence", &self.identity.identity_evidence)?;
        map.serialize_entry("revoked", &self.identity.revoked)?;
        map.serialize_entry("created_at_ms", &self.identity.created_at_ms)?;
        map.serialize_entry("trust_anchor_sha256", &self.trust_anchor_sha256)?;
        map.serialize_entry(apw_core::PROOF_LEVEL_KEY, &self.identity.proof_level)?;
        map.serialize_entry("limits", &self.limits)?;
        map.end()
    }
}

/// X.509 material used to sign a claim.
///
/// `certificate_chain_pem` is leaf-then-issuer concatenated PEM, the ordering the
/// C2PA signer requires. `private_key_handle` is opaque to callers: a local
/// provider returns key bytes, a remote provider returns a service handle whose
/// private key never leaves the vault.
#[derive(Clone)]
pub struct SigningMaterial {
    pub key_id: String,
    pub algorithm: String,
    pub certificate_chain_pem: Vec<u8>,
    pub private_key_handle: Zeroizing<Vec<u8>>,
    pub trust_anchor_pem: Vec<u8>,
    pub proof_level: ProofLevel,
}

// IMPORTANT: hand-written so a derived Debug cannot print the PKCS#8 key into a
// log line or an error format.
impl core::fmt::Debug for SigningMaterial {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SigningMaterial")
            .field("key_id", &self.key_id)
            .field("algorithm", &self.algorithm)
            .field("certificate_chain_pem_bytes", &self.certificate_chain_pem.len())
            .field("private_key_handle", &"<redacted>")
            .field("trust_anchor_pem_bytes", &self.trust_anchor_pem.len())
            .field("proof_level", &self.proof_level)
            .finish()
    }
}

/// What a soft binding attachment actually did, and what it does not claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MarkAttachment {
    pub mark_id: String,
    pub content_sha256: String,
    pub asset_modified: bool,
    pub mechanism: String,
    #[serde(rename = "apw:proof_level")]
    pub proof_level: ProofLevel,
    pub limits: Vec<String>,
}

/// A recovered soft binding. `match_similarity` is resemblance, not identity,
/// which is why the proof level is never better than [`ProofLevel::Inferred`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecoveredMark {
    pub mark_id: String,
    pub payload: Value,
    pub registered_content_sha256: String,
    pub observed_content_sha256: String,
    pub match_similarity: f64,
    #[serde(rename = "apw:proof_level")]
    pub proof_level: ProofLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryReceipt {
    pub registry_id: String,
    pub content_sha256: String,
    pub manifest_sha256: String,
    pub mark_id: Option<String>,
    pub key_id: String,
    pub chain_pem: String,
    pub signature_hex: String,
    pub registered_at: String,
    #[serde(rename = "apw:proof_level")]
    pub proof_level: ProofLevel,
}

/// Which key found the registry record. It drives the reported proof level: a
/// content or manifest digest is an exact match, a mark is a similarity match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchedBy {
    ContentSha256,
    ManifestSha256,
    MarkId,
    None,
}

// IMPORTANT: one rendering, not two. A derived Serialize beside `as_str` lets the
// wire value and the value callers read drift apart.
impl Serialize for MatchedBy {
    fn serialize<S: Serializer>(&self, serializer: S) -> core::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl MatchedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchedBy::ContentSha256 => "content_sha256",
            MatchedBy::ManifestSha256 => "manifest_sha256",
            MatchedBy::MarkId => "mark_id",
            MatchedBy::None => "none",
        }
    }

    pub fn proof_level(self) -> ProofLevel {
        match self {
            MatchedBy::ContentSha256 | MatchedBy::ManifestSha256 => ProofLevel::DirectlyObserved,
            MatchedBy::MarkId => ProofLevel::Inferred,
            MatchedBy::None => ProofLevel::UnknownUnobserved,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VerificationOutcome {
    pub state: VerificationState,
    pub reason: String,
    pub content_sha256: String,
    pub registry_id: Option<String>,
    pub mark: Option<RecoveredMark>,
    pub matched_by: MatchedBy,
    #[serde(rename = "apw:proof_level")]
    pub proof_level: ProofLevel,
    pub normative_note: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigningRecord {
    pub event: String,
    pub key_id: String,
    pub at: String,
    pub payload_sha256: String,
    pub signature_hex: String,
    #[serde(rename = "apw:proof_level")]
    pub proof_level: ProofLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationRecord {
    pub event: String,
    pub key_id: String,
    pub at: String,
    pub retained_signing_records: usize,
    #[serde(rename = "apw:proof_level")]
    pub proof_level: ProofLevel,
}
