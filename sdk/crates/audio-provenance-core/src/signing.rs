use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use ed25519_dalek::{Signature, Signer, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroize;

use crate::canonical::canonical_json;
use crate::error::{KeyError, SignatureError, VocabularyError};
use crate::hashing::{sha256, sha256_hex};
use crate::vocabulary::ProofLevel;

pub const SIGNER_ID_HEX_LEN: usize = 16;

pub const PORTABLE_SIGNATURE_NOTES: &str = "The public key independently verifies canonical manifest integrity. A valid self-generated signature proves key possession, not creator identity, authorship, or external trust.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SignatureAlgorithm {
    #[default]
    Ed25519,
    /// Ed25519 over the raw 32-byte SHA-256 digest of the canonical manifest.
    ///
    /// This is deliberately a distinct wire algorithm. Labelling a digest signature as plain
    /// Ed25519 would make local and remote signers disagree about the bytes the signature covers.
    #[serde(rename = "Ed25519-SHA256")]
    Ed25519Sha256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Canonicalization {
    #[default]
    #[serde(rename = "apw-json-sort-v1")]
    ApwJsonSortV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TrustScope {
    #[default]
    #[serde(rename = "self_generated_demo_key_integrity")]
    SelfGeneratedDemoKeyIntegrity,
    /// The signing operation was delegated to an authenticated remote key-custody service.
    /// This still proves key possession rather than the human or organisation operating it.
    #[serde(rename = "remote_key_custody_integrity")]
    RemoteKeyCustodyIntegrity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SignerIdentity {
    #[default]
    #[serde(rename = "not_established")]
    NotEstablished,
}

pub fn signer_id_for_public_key(public_key: &[u8; 32]) -> String {
    let mut digest = sha256_hex(public_key);
    digest.truncate(SIGNER_ID_HEX_LEN);
    digest
}

pub struct SigningKey {
    inner: ed25519_dalek::SigningKey,
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SigningKey")
            .field("signer_id", &self.signer_id())
            .finish_non_exhaustive()
    }
}

impl SigningKey {
    pub fn from_raw_bytes(raw: &[u8]) -> Result<Self, KeyError> {
        let key: [u8; 32] = raw
            .try_into()
            .map_err(|_| KeyError::PrivateKeyLength { found: raw.len() })?;
        let mut key = key;
        let inner = ed25519_dalek::SigningKey::from_bytes(&key);
        key.zeroize();
        Ok(Self { inner })
    }

    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.inner.verifying_key().to_bytes()
    }

    pub fn public_key_hex(&self) -> String {
        hex::encode(self.public_key_bytes())
    }

    pub fn signer_id(&self) -> String {
        signer_id_for_public_key(&self.public_key_bytes())
    }

    /// Signs `domain || 0x00 || message`.
    ///
    /// IMPORTANT: there is deliberately no undomained signing method. A manifest signature is taken
    /// over bare canonical JSON, so every other protocol that signs with a Audio Provenance key must live
    /// in a disjoint input space or one of its signatures could be replayed as the other. Callers
    /// pass a protocol-unique `domain`; `sha256` domain separation in [`crate::locator`] uses the
    /// same shape.
    pub fn sign_domain_separated(&self, domain: &str, message: &[u8]) -> [u8; 64] {
        self.inner
            .sign(&domain_separated_input(domain, message))
            .to_bytes()
    }

    /// Signs the canonical bytes of `unsigned_manifest`, which MUST NOT contain the
    /// `portable_signature` or `manifest_signature` keys.
    pub fn sign_manifest(
        &self,
        unsigned_manifest: &Value,
        public_key_file: &str,
    ) -> Result<PortableSignature, SignatureError> {
        let content = canonical_json(unsigned_manifest)?;
        let public_key = self.public_key_bytes();
        Ok(PortableSignature {
            algorithm: SignatureAlgorithm::Ed25519,
            canonicalization: Canonicalization::ApwJsonSortV1,
            public_key_hex: hex::encode(public_key),
            public_key_file: public_key_file.to_string(),
            signer_id: signer_id_for_public_key(&public_key),
            signature_hex: hex::encode(self.inner.sign(&content).to_bytes()),
            signed_content_hash: sha256_hex(&content),
            trust_scope: TrustScope::SelfGeneratedDemoKeyIntegrity,
            signer_identity: SignerIdentity::NotEstablished,
            signer_identity_proof_level: ProofLevel::UnknownUnobserved,
            proof_level: ProofLevel::DirectlyObserved,
            notes: PORTABLE_SIGNATURE_NOTES.to_string(),
        })
    }
}

/// A source of portable manifest signatures.
///
/// Implementations may hold a local key or delegate the signing operation. The public key is part
/// of the interface because locator allocation must happen before the manifest is signed.
pub trait ManifestSigner: fmt::Debug + Send + Sync {
    fn public_key_bytes(&self) -> [u8; 32];

    fn sign_manifest(
        &self,
        unsigned_manifest: &Value,
        public_key_file: &str,
    ) -> Result<PortableSignature, SignatureError>;
}

impl ManifestSigner for SigningKey {
    fn public_key_bytes(&self) -> [u8; 32] {
        Self::public_key_bytes(self)
    }

    fn sign_manifest(
        &self,
        unsigned_manifest: &Value,
        public_key_file: &str,
    ) -> Result<PortableSignature, SignatureError> {
        Self::sign_manifest(self, unsigned_manifest, public_key_file)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortableSignature {
    pub algorithm: SignatureAlgorithm,
    pub canonicalization: Canonicalization,
    pub public_key_hex: String,
    pub public_key_file: String,
    pub signer_id: String,
    pub signature_hex: String,
    pub signed_content_hash: String,
    pub trust_scope: TrustScope,
    pub signer_identity: SignerIdentity,
    pub signer_identity_proof_level: ProofLevel,
    #[serde(rename = "apw:proof_level")]
    pub proof_level: ProofLevel,
    pub notes: String,
}

#[derive(Debug, Deserialize)]
struct PortableSignatureWire {
    algorithm: SignatureAlgorithm,
    canonicalization: Canonicalization,
    public_key_hex: String,
    public_key_file: String,
    signer_id: String,
    signature_hex: String,
    signed_content_hash: String,
    trust_scope: TrustScope,
    signer_identity: SignerIdentity,
    signer_identity_proof_level: ProofLevel,
    #[serde(rename = "apw:proof_level")]
    proof_level: ProofLevel,
    notes: String,
}

impl<'de> Deserialize<'de> for PortableSignature {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = PortableSignatureWire::deserialize(deserializer)?;
        if wire.signer_identity_proof_level != ProofLevel::UnknownUnobserved {
            return Err(serde::de::Error::custom(
                VocabularyError::SignerIdentityOverstated {
                    found: wire.signer_identity_proof_level.as_str(),
                },
            ));
        }
        if wire.proof_level != ProofLevel::DirectlyObserved {
            return Err(serde::de::Error::custom(
                VocabularyError::SignatureProofLevelInvalid {
                    found: wire.proof_level.as_str(),
                },
            ));
        }
        let signature = Self {
            algorithm: wire.algorithm,
            canonicalization: wire.canonicalization,
            public_key_hex: wire.public_key_hex,
            public_key_file: wire.public_key_file,
            signer_id: wire.signer_id,
            signature_hex: wire.signature_hex,
            signed_content_hash: wire.signed_content_hash,
            trust_scope: wire.trust_scope,
            signer_identity: wire.signer_identity,
            signer_identity_proof_level: wire.signer_identity_proof_level,
            proof_level: wire.proof_level,
            notes: wire.notes,
        };
        signature
            .public_key()
            .and_then(|_| signature.signature_bytes().map(|_| ()))
            .map_err(serde::de::Error::custom)?;
        Ok(signature)
    }
}

impl PortableSignature {
    pub fn public_key(&self) -> Result<VerifyingKey, SignatureError> {
        let raw = decode_fixed::<32>(&self.public_key_hex).map_err(|len| match len {
            Some(found) => SignatureError::Key(KeyError::PublicKeyLength { found }),
            None => SignatureError::Key(KeyError::MalformedHex),
        })?;
        VerifyingKey::from_bytes(&raw).map_err(|_| SignatureError::Key(KeyError::InvalidPublicKey))
    }

    pub fn signature_bytes(&self) -> Result<[u8; 64], SignatureError> {
        decode_fixed::<64>(&self.signature_hex).map_err(|len| match len {
            Some(found) => SignatureError::SignatureLength { found },
            None => SignatureError::Key(KeyError::MalformedHex),
        })
    }
}

fn decode_fixed<const N: usize>(value: &str) -> Result<[u8; N], Option<usize>> {
    let raw: Vec<u8> = hex::decode(value).map_err(|_| None)?;
    let len = raw.len();
    raw.try_into().map_err(|_| Some(len))
}

/// What a valid self-generated Ed25519 signature actually establishes. There is no constructor that
/// can claim more: the signer identity is fixed at `unknown_unobserved`, exactly as
/// `daemon/signing.py` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPossessionProof {
    trust_scope: TrustScope,
    signer_id: String,
    public_key: [u8; 32],
}

impl KeyPossessionProof {
    pub const fn trust_scope(&self) -> TrustScope {
        self.trust_scope
    }

    /// The key the signature actually verified under.
    ///
    /// IMPORTANT: a trust store MUST bind its records to these 32 bytes, not to [`Self::signer_id`].
    /// The signer id is a 64-bit truncation of the key's digest, so binding a name to it would make
    /// impersonation a 2^64 second-preimage search instead of a forgery.
    pub const fn public_key_bytes(&self) -> &[u8; 32] {
        &self.public_key
    }

    pub const fn signer_identity_proof_level(&self) -> ProofLevel {
        ProofLevel::UnknownUnobserved
    }

    pub fn signer_id(&self) -> &str {
        &self.signer_id
    }

    pub fn signer_id_matches_declaration(&self, declared: &str) -> bool {
        self.signer_id() == declared
    }
}

/// Verifies a portable signature over `unsigned_manifest`, which MUST have had the
/// `portable_signature` and `manifest_signature` keys removed, matching `daemon/verify.py`.
///
/// IMPORTANT: the canonical content hash is recomputed and compared against `signed_content_hash`
/// BEFORE the Ed25519 verification, mirroring `verify_ed25519_signature`. A signature that verifies
/// over bytes other than the manifest in hand must never be reported as covering that manifest.
pub fn verify_manifest_signature(
    unsigned_manifest: &Value,
    signature: &PortableSignature,
    public_key_override: Option<&[u8; 32]>,
) -> Result<KeyPossessionProof, SignatureError> {
    let verifying_key = match public_key_override {
        Some(raw) => VerifyingKey::from_bytes(raw)
            .map_err(|_| SignatureError::Key(KeyError::InvalidPublicKey))?,
        None => signature.public_key()?,
    };
    let signature_bytes = signature.signature_bytes()?;

    if signature.signed_content_hash.len() != 64
        || !signature
            .signed_content_hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(SignatureError::MalformedContentHash);
    }

    let content = canonical_json(unsigned_manifest)?;
    if sha256_hex(&content) != signature.signed_content_hash {
        return Err(SignatureError::ContentHashMismatch);
    }

    let signed_bytes = match signature.algorithm {
        SignatureAlgorithm::Ed25519 => content.as_slice(),
        SignatureAlgorithm::Ed25519Sha256 => {
            let digest = sha256(&content);
            verifying_key
                .verify_strict(&digest, &Signature::from_bytes(&signature_bytes))
                .map_err(|_| SignatureError::InvalidSignature)?;
            let public_key = verifying_key.to_bytes();
            return Ok(KeyPossessionProof {
                trust_scope: signature.trust_scope,
                signer_id: signer_id_for_public_key(&public_key),
                public_key,
            });
        }
    };

    verifying_key
        .verify_strict(signed_bytes, &Signature::from_bytes(&signature_bytes))
        .map_err(|_| SignatureError::InvalidSignature)?;

    let public_key = verifying_key.to_bytes();
    Ok(KeyPossessionProof {
        trust_scope: signature.trust_scope,
        signer_id: signer_id_for_public_key(&public_key),
        public_key,
    })
}

/// Verifies a signature over `domain || 0x00 || message`, the counterpart to
/// [`SigningKey::sign_domain_separated`].
pub fn verify_domain_separated(
    public_key: &[u8; 32],
    domain: &str,
    message: &[u8],
    signature: &[u8; 64],
) -> Result<(), SignatureError> {
    VerifyingKey::from_bytes(public_key)
        .map_err(|_| SignatureError::Key(KeyError::InvalidPublicKey))?
        .verify_strict(
            &domain_separated_input(domain, message),
            &Signature::from_bytes(signature),
        )
        .map_err(|_| SignatureError::InvalidSignature)
}

/// The 0x00 separator is what makes the framing unambiguous: no domain string may contain a NUL, so
/// no `(domain, message)` pair can produce the same bytes as a different pair.
fn domain_separated_input(domain: &str, message: &[u8]) -> Vec<u8> {
    debug_assert!(!domain.as_bytes().contains(&0), "domain must not hold NUL");
    let mut input = Vec::with_capacity(domain.len() + 1 + message.len());
    input.extend_from_slice(domain.as_bytes());
    input.push(0);
    input.extend_from_slice(message);
    input
}

/// Strips the two signature envelopes so the remainder is exactly what `daemon/verify.py` signs
/// over.
pub fn unsigned_manifest_view(manifest: &Value) -> Value {
    match manifest {
        Value::Object(map) => {
            let mut stripped = serde_json::Map::with_capacity(map.len());
            for (key, value) in map {
                if key != "portable_signature" && key != "manifest_signature" {
                    stripped.insert(key.clone(), value.clone());
                }
            }
            Value::Object(stripped)
        }
        other => other.clone(),
    }
}

#[cfg(feature = "std")]
pub mod fs {
    use super::{KeyError, SigningKey};
    use std::path::Path;
    use zeroize::Zeroize;

    /// The only filesystem path in this crate. Library callers that must run on wasm32 construct a
    /// [`SigningKey`] from bytes instead.
    pub fn load_signing_key(path: &Path) -> Result<SigningKey, KeyError> {
        let mut raw = std::fs::read(path)?;
        let key = SigningKey::from_raw_bytes(&raw);
        raw.zeroize();
        key
    }
}
