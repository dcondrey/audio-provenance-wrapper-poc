use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde_json::Value;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::canonical::{canonical_json_utf8, CANONICALIZATION_PORTABLE};
use crate::error::{CoreError, Result};
use crate::hash::{hex_decode, hex_lower, sha256_hex};
use crate::proof::ProofLevel;

pub const TRUST_SCOPE_SELF_GENERATED: &str = "self_generated_demo_key_integrity";
pub const TRUST_SCOPE_LOCAL_SOFTWARE: &str = "local_software_integrity";
pub const TRUST_SCOPE_HARDWARE_PROVIDER: &str = "hardware_provider";

pub const PORTABLE_SIGNATURE_NOTES: &str =
    "The public key independently verifies canonical manifest integrity. \
     A valid self-generated signature proves key possession, not creator identity, authorship, or external trust.";

pub const PORTABLE_SIGNATURE_VALID_MESSAGE: &str =
    "Ed25519 signature verified with the public key; signer identity remains unverified";

const PRIVATE_KEY_MODE: u32 = 0o600;
const PUBLIC_KEY_MODE: u32 = 0o644;

/// A self-issued chain proves POSSESSION OF THE KEY. It does not establish
/// identity. `signer_identity` and `signer_identity_proof_level` are separate
/// fields from the signature's own proof level and are never merged.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PortableSignature {
    pub algorithm: String,
    pub canonicalization: String,
    pub public_key_hex: String,
    pub public_key_file: String,
    pub signer_id: String,
    pub signature_hex: String,
    pub signed_content_hash: String,
    pub trust_scope: String,
    pub signer_identity: String,
    pub signer_identity_proof_level: ProofLevel,
    #[serde(rename = "apw:proof_level")]
    pub proof_level: ProofLevel,
    pub notes: String,
}

/// Rejection reasons render to the exact strings the Python verifier emits,
/// because they become a `Finding::message` inside a hash-bound bundle member.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignatureRejection {
    #[error("portable public key is malformed")]
    MalformedPublicKey,
    #[error("portable public key must be 32 raw bytes")]
    PublicKeyWrongLength,
    #[error("portable signature is malformed")]
    MalformedSignature,
    #[error("portable signed-content hash does not match canonical manifest")]
    ContentHashMismatch,
    #[error("Ed25519 signature is invalid")]
    SignatureInvalid,
}

/// The secret key is zeroized on drop by `ed25519_dalek::SigningKey`.
pub struct Ed25519Signer {
    key: SigningKey,
    public_key_path: PathBuf,
}

impl core::fmt::Debug for Ed25519Signer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Ed25519Signer")
            .field("public_key_hex", &self.public_key_hex())
            .field("public_key_path", &self.public_key_path)
            .finish()
    }
}

impl Ed25519Signer {
    /// Loads a 32-byte raw seed, or generates and persists one at mode 0o600.
    /// Rewrites the raw public key at 0o644 when it is absent or stale.
    ///
    /// IMPORTANT: both paths must already be `~`-expanded. `public_key_path`
    /// is copied verbatim into `public_key_file`, which the local manifest
    /// signature covers, so the exact string is signature-relevant.
    pub fn load_or_create(private_key_path: &Path, public_key_path: &Path) -> Result<Self> {
        let key = if private_key_path.is_file() {
            let raw = Zeroizing::new(
                fs::read(private_key_path)
                    .map_err(|source| CoreError::io(private_key_path, source))?,
            );
            let seed: Zeroizing<[u8; 32]> =
                Zeroizing::new(raw.as_slice().try_into().map_err(|_| {
                    CoreError::KeyMaterial(
                        "Ed25519 private key file must contain exactly 32 raw bytes",
                    )
                })?);
            SigningKey::from_bytes(&seed)
        } else {
            let key = SigningKey::generate(&mut rand_core::OsRng);
            let seed = Zeroizing::new(key.to_bytes());
            write_key_file(private_key_path, seed.as_slice(), PRIVATE_KEY_MODE)?;
            key
        };

        let public_raw = key.verifying_key().to_bytes();
        let stale = match fs::read(public_key_path) {
            Ok(existing) => existing != public_raw,
            Err(_) => true,
        };
        if stale {
            write_key_file(public_key_path, &public_raw, PUBLIC_KEY_MODE)?;
        }

        Ok(Ed25519Signer {
            key,
            public_key_path: public_key_path.to_path_buf(),
        })
    }

    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    pub fn public_key_hex(&self) -> String {
        hex_lower(&self.public_key_bytes())
    }

    pub fn public_key_path(&self) -> &Path {
        &self.public_key_path
    }

    pub fn signer_id(&self) -> String {
        signer_id_of(&self.public_key_bytes())
    }

    /// Signs `canonical_json_utf8(unsigned)`. The caller supplies the exclusion
    /// set; see [`crate::canonical::PORTABLE_SIGNATURE_EXCLUDED_KEYS`].
    pub fn sign_manifest(&self, unsigned: &Value) -> Result<PortableSignature> {
        let content = canonical_json_utf8(unsigned)?;
        let public_raw = self.public_key_bytes();
        Ok(PortableSignature {
            algorithm: "Ed25519".to_owned(),
            canonicalization: CANONICALIZATION_PORTABLE.to_owned(),
            public_key_hex: hex_lower(&public_raw),
            public_key_file: self.public_key_path.display().to_string(),
            signer_id: signer_id_of(&public_raw),
            signature_hex: hex_lower(&self.key.sign(&content).to_bytes()),
            signed_content_hash: sha256_hex(&content),
            trust_scope: TRUST_SCOPE_SELF_GENERATED.to_owned(),
            signer_identity: "not_established".to_owned(),
            signer_identity_proof_level: ProofLevel::UnknownUnobserved,
            proof_level: ProofLevel::DirectlyObserved,
            notes: PORTABLE_SIGNATURE_NOTES.to_owned(),
        })
    }
}

pub fn signer_id_of(public_key: &[u8]) -> String {
    let digest = sha256_hex(public_key);
    digest.get(..16).unwrap_or(&digest).to_owned()
}

/// IMPORTANT: a canonicalization failure is reported as
/// [`SignatureRejection::ContentHashMismatch`]. The frozen vocabulary has no
/// variant for it, and it is unreachable from a parsed manifest: JSON has no
/// non-finite literal, so a `Value` from `serde_json` never carries one.
pub fn verify_portable_signature(
    unsigned: &Value,
    signature: &PortableSignature,
    public_key_override: Option<&[u8; 32]>,
) -> core::result::Result<&'static str, SignatureRejection> {
    let public_raw: Vec<u8> = match public_key_override {
        Some(bytes) => bytes.to_vec(),
        None => {
            hex_decode(&signature.public_key_hex).ok_or(SignatureRejection::MalformedPublicKey)?
        }
    };
    let public_raw: [u8; 32] = public_raw
        .try_into()
        .map_err(|_| SignatureRejection::PublicKeyWrongLength)?;
    let signature_bytes =
        hex_decode(&signature.signature_hex).ok_or(SignatureRejection::MalformedSignature)?;

    let content = canonical_json_utf8(unsigned).map_err(|_| SignatureRejection::ContentHashMismatch)?;
    let expected = sha256_hex(&content);
    if expected
        .as_bytes()
        .ct_eq(signature.signed_content_hash.as_bytes())
        .unwrap_u8()
        != 1
    {
        return Err(SignatureRejection::ContentHashMismatch);
    }

    // IMPORTANT: `verify_strict` also rejects small-order public keys, which
    // OpenSSL (and therefore the Python path) accepts. Both render the same
    // "Ed25519 signature is invalid" message, and the divergence is confined to
    // maliciously crafted keys that never belong to a real signer.
    let parsed: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| SignatureRejection::SignatureInvalid)?;
    let verifying_key =
        VerifyingKey::from_bytes(&public_raw).map_err(|_| SignatureRejection::SignatureInvalid)?;
    verifying_key
        .verify_strict(&content, &Signature::from_bytes(&parsed))
        .map_err(|_| SignatureRejection::SignatureInvalid)?;
    Ok(PORTABLE_SIGNATURE_VALID_MESSAGE)
}

fn write_key_file(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|source| CoreError::io(parent, source))?;
        }
    }
    fs::write(path, bytes).map_err(|source| CoreError::io(path, source))?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|source| CoreError::io(path, source))
}
