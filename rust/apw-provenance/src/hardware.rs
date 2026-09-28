use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::chain::{hex_lower, random_bytes};
use crate::error::{ProvenanceError, Result};
use crate::local::expand_user;

pub const DEFAULT_DEVICE_KEY_PATH: &str = "~/.apw/device_key.bin";
pub const DEFAULT_DEMO_KEY_PATH: &str = "~/.apw/demo_signing_key.bin";

const SEAL_DOMAIN: &[u8] = b"apw-seal-";
const NONCE_BYTES: usize = 16;
const TAG_BYTES: usize = 16;

/// Unique hardware-bound identity for this machine.
///
/// IMPORTANT: `algorithm` says what actually produced it. A software fallback
/// reports `hmac-sha256-local`, which is not an attestation of any device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub device_id: String,
    pub public_key_hex: String,
    pub algorithm: String,
    pub created_at_ms: u64,
}

/// A hash chain root sealed to hardware.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareBinding {
    pub chain_root_hash: String,
    pub device_id: String,
    pub monotonic_counter: u64,
    pub clock_ms: u64,
    pub signature_hex: String,
    pub public_key_hex: String,
}

/// Self-entangled cosignature following the CPoE pattern.
///
/// Each cosignature chains the previous one, so forging checkpoint N requires
/// valid signatures for all preceding checkpoints.
///
/// `entangled_hash = SHA256(domain_separator || content_hash || software_signature
/// || hardware_clock_ms || monotonic_counter || device_id || previous_cosignature)`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareCosignature {
    pub entangled_hash: String,
    pub content_hash: String,
    pub hardware_clock_ms: u64,
    pub monotonic_counter: u64,
    pub device_id: String,
    pub signature_hex: String,
    pub previous_cosignature_hash: String,
}

impl HardwareCosignature {
    pub const DOMAIN_SEPARATOR: &'static [u8] = b"apw-hw-cosign-v1";
}

/// Interface for hardware security modules.
///
/// IMPORTANT: implementing this trait does not by itself make evidence
/// attestable. [`SoftwareProvider`] implements it with filesystem keys and is
/// explicitly not attestable; see its documentation.
pub trait HardwareProvider: Send + Sync {
    fn device_identity(&self) -> Result<DeviceIdentity>;

    /// Sign with the device-bound private key, which never leaves the module.
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>>;

    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<bool>;

    /// Encrypt so that only this device can decrypt.
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>>;

    fn unseal(&self, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>>;

    /// A counter that increments on each call and cannot be rolled back. Used to
    /// detect replay against the hash chain.
    fn monotonic_counter(&self) -> u64;

    /// What the monotonic counter is actually backed by, in the words the
    /// manifest records (`hardware_cosignature.counter_scope`).
    fn counter_scope(&self) -> String {
        "provider_defined".to_owned()
    }

    fn clock_ms(&self) -> u64;

    /// Attest that a hash chain root was produced on this device at this counter.
    fn bind_chain_root(&self, chain_root_hash: &str) -> Result<HardwareBinding> {
        let identity = self.device_identity()?;
        let counter = self.monotonic_counter();
        let clock = self.clock_ms();

        let mut payload = Vec::new();
        payload.extend_from_slice(chain_root_hash.as_bytes());
        payload.extend_from_slice(identity.device_id.as_bytes());
        payload.extend_from_slice(&counter.to_be_bytes());
        payload.extend_from_slice(&clock.to_be_bytes());
        let signature = self.sign(&payload)?;

        Ok(HardwareBinding {
            chain_root_hash: chain_root_hash.to_string(),
            device_id: identity.device_id,
            monotonic_counter: counter,
            clock_ms: clock,
            signature_hex: hex_lower(&signature),
            public_key_hex: identity.public_key_hex,
        })
    }

    fn cosign_checkpoint(
        &self,
        content_hash: &str,
        software_signature: &str,
        previous_cosignature_hash: &str,
    ) -> Result<HardwareCosignature> {
        let identity = self.device_identity()?;
        let counter = self.monotonic_counter();
        let clock = self.clock_ms();

        let mut entangle_input = Vec::new();
        entangle_input.extend_from_slice(HardwareCosignature::DOMAIN_SEPARATOR);
        entangle_input.extend_from_slice(content_hash.as_bytes());
        entangle_input.extend_from_slice(software_signature.as_bytes());
        entangle_input.extend_from_slice(&clock.to_be_bytes());
        entangle_input.extend_from_slice(&counter.to_be_bytes());
        entangle_input.extend_from_slice(identity.device_id.as_bytes());
        entangle_input.extend_from_slice(previous_cosignature_hash.as_bytes());
        let entangled_hash = hex_lower(&Sha256::digest(&entangle_input));
        let signature = self.sign(entangled_hash.as_bytes())?;

        Ok(HardwareCosignature {
            entangled_hash,
            content_hash: content_hash.to_string(),
            hardware_clock_ms: clock,
            monotonic_counter: counter,
            device_id: identity.device_id,
            signature_hex: hex_lower(&signature),
            previous_cosignature_hash: previous_cosignature_hash.to_string(),
        })
    }
}

/// Fallback provider using a filesystem-stored HMAC key.
///
/// IMPORTANT: NOT attestable. Evidence produced with this provider carries proof
/// level `directly_observed` for the hash chain but `unknown_unobserved` for
/// hardware binding. An auditor can verify chain integrity but not that the chain
/// was produced on a specific device.
pub struct SoftwareProvider {
    key_path: PathBuf,
    seed: Zeroizing<Vec<u8>>,
    device_id: String,
    counter: AtomicU64,
}

impl core::fmt::Debug for SoftwareProvider {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SoftwareProvider")
            .field("key_path", &self.key_path)
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

impl SoftwareProvider {
    pub fn new(key_path: &Path) -> Result<Self> {
        let key_path = expand_user(key_path);
        let seed = load_or_create_seed(&key_path)?;
        let device_id = hex_lower(&Sha256::digest(&seed))
            .get(..16)
            .unwrap_or_default()
            .to_string();
        Ok(SoftwareProvider {
            key_path,
            seed,
            device_id,
            counter: AtomicU64::new(0),
        })
    }

    fn mac(&self, key: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key)
            .map_err(|error| ProvenanceError::KeyMaterial(error.to_string()))?;
        mac.update(data);
        Ok(mac.finalize().into_bytes().to_vec())
    }

    /// Counter-varied keystream: HMAC(key, nonce || counter) per block.
    ///
    /// IMPORTANT: a repeated fixed-key block leaks identical ciphertext for
    /// identical aligned plaintext blocks (ECB-style). Mixing in a big-endian
    /// block counter makes every block's keystream distinct.
    fn keystream(&self, key: &[u8], nonce: &[u8], length: usize) -> Result<Zeroizing<Vec<u8>>> {
        let mut out = Zeroizing::new(Vec::with_capacity(length));
        let mut counter: u64 = 0;
        while out.len() < length {
            let mut input = nonce.to_vec();
            input.extend_from_slice(&counter.to_be_bytes());
            out.extend_from_slice(&self.mac(key, &input)?);
            counter += 1;
        }
        out.truncate(length);
        Ok(out)
    }
}

impl HardwareProvider for SoftwareProvider {
    fn counter_scope(&self) -> String {
        "persisted local file counter, not hardware-backed; rollback is prevented only by \
         filesystem permissions on the signer state file"
            .to_owned()
    }

    fn device_identity(&self) -> Result<DeviceIdentity> {
        let created_at_ms = std::fs::metadata(&self.key_path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|since| since.as_millis() as u64)
            .unwrap_or_default();
        Ok(DeviceIdentity {
            device_id: self.device_id.clone(),
            public_key_hex: String::new(),
            algorithm: "hmac-sha256-local".to_string(),
            created_at_ms,
        })
    }

    fn sign(&self, data: &[u8]) -> Result<Vec<u8>> {
        self.mac(&self.seed, data)
    }

    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<bool> {
        let expected = self.mac(&self.seed, data)?;
        Ok(bool::from(expected.ct_eq(signature)))
    }

    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = random_bytes(NONCE_BYTES);
        let mut key_input = SEAL_DOMAIN.to_vec();
        key_input.extend_from_slice(&nonce);
        let key = Zeroizing::new(self.mac(&self.seed, &key_input)?);
        let keystream = self.keystream(&key, &nonce, plaintext.len())?;
        let ciphertext: Vec<u8> = plaintext
            .iter()
            .zip(keystream.iter())
            .map(|(byte, mask)| byte ^ mask)
            .collect();
        let tag = self.mac(&key, &ciphertext)?;

        let mut sealed = Vec::with_capacity(NONCE_BYTES + TAG_BYTES + ciphertext.len());
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(tag.get(..TAG_BYTES).unwrap_or_default());
        sealed.extend_from_slice(&ciphertext);
        Ok(sealed)
    }

    fn unseal(&self, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let (Some(nonce), Some(tag), Some(ciphertext)) = (
            sealed.get(..NONCE_BYTES),
            sealed.get(NONCE_BYTES..NONCE_BYTES + TAG_BYTES),
            sealed.get(NONCE_BYTES + TAG_BYTES..),
        ) else {
            return Err(ProvenanceError::Seal("sealed data too short"));
        };
        let mut key_input = SEAL_DOMAIN.to_vec();
        key_input.extend_from_slice(nonce);
        let key = Zeroizing::new(self.mac(&self.seed, &key_input)?);
        let expected = self.mac(&key, ciphertext)?;
        // REQUIRED: authenticate before decrypting, in constant time.
        if !bool::from(expected.get(..TAG_BYTES).unwrap_or_default().ct_eq(tag)) {
            return Err(ProvenanceError::Seal("sealed data integrity check failed"));
        }
        let keystream = self.keystream(&key, nonce, ciphertext.len())?;
        Ok(Zeroizing::new(
            ciphertext
                .iter()
                .zip(keystream.iter())
                .map(|(byte, mask)| byte ^ mask)
                .collect(),
        ))
    }

    fn monotonic_counter(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn clock_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_millis() as u64)
            .unwrap_or_default()
    }
}

fn load_or_create_seed(path: &Path) -> Result<Zeroizing<Vec<u8>>> {
    if path.is_file() {
        return Ok(Zeroizing::new(std::fs::read(path).map_err(|source| {
            ProvenanceError::io("read", path, source)
        })?));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| ProvenanceError::io("create", parent, source))?;
    }
    let seed = random_bytes(32);
    std::fs::write(path, &seed).map_err(|source| ProvenanceError::io("write", path, source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|source| ProvenanceError::io("set permissions on", path, source))?;
    }
    log::info!("Created software signing key at {}", path.display());
    Ok(seed)
}

/// Select the best available hardware provider for this platform.
///
/// IMPORTANT: Secure Enclave and TPM 2.0 backends are not implemented, so this
/// always returns the operational local software signer and logs that its output
/// is not hardware-attested. It never silently upgrades the claim.
pub fn detect_hardware_provider(software_key_path: &Path) -> Result<Box<dyn HardwareProvider>> {
    #[cfg(target_os = "macos")]
    log::warn!("Secure Enclave integration is not implemented; using a local software integrity key");

    #[cfg(target_os = "linux")]
    if Path::new("/dev/tpm0").exists() || Path::new("/dev/tpmrm0").exists() {
        log::warn!(
            "TPM 2.0 hardware is present but its provider is not implemented; \
             using a local software integrity key"
        );
    }

    Ok(Box::new(SoftwareProvider::new(software_key_path)?))
}
