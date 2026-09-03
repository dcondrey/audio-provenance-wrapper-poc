use std::sync::Arc;

use apw_core::{
    ProofLevel, PROOF_LEVEL_KEY, TRUST_SCOPE_HARDWARE_PROVIDER, TRUST_SCOPE_LOCAL_SOFTWARE,
};
use apw_daemon::{LocalSealer, Seal};
use apw_provenance::HardwareProvider;
use serde_json::{json, Value};

const SOFTWARE_NOTES: &str =
    "The local HMAC seal detects changes when checked with the same secret key; it is not \
     hardware attestation or third-party identity verification.";
const HARDWARE_NOTES: &str = "Signature produced by the configured hardware provider.";

/// Binds the hash-chain root and seals the assembled manifest with the device
/// provider.
///
/// IMPORTANT: a software provider's seal detects change when re-checked with the
/// same key. It is not attestation and not identity, which is why
/// `hardware_attested` and the proof level are both derived from what the
/// provider actually is rather than from the fact that a signature exists.
pub struct ProviderSealer {
    provider: Arc<dyn HardwareProvider>,
    hardware_attested: bool,
}

impl ProviderSealer {
    pub fn new(provider: Arc<dyn HardwareProvider>, hardware_attested: bool) -> Self {
        ProviderSealer {
            provider,
            hardware_attested,
        }
    }

    fn proof_level(&self) -> ProofLevel {
        if self.hardware_attested {
            ProofLevel::DirectlyObserved
        } else {
            ProofLevel::UnknownUnobserved
        }
    }
}

impl LocalSealer for ProviderSealer {
    fn bind_chain_root(&self, hash_chain_root: &str) -> Option<Value> {
        let binding = match self.provider.bind_chain_root(hash_chain_root) {
            Ok(binding) => binding,
            Err(error) => {
                log::warn!("Could not bind hash chain root to device: {error}");
                return None;
            }
        };
        let mut record = match serde_json::to_value(&binding) {
            Ok(Value::Object(fields)) => fields,
            _ => {
                log::warn!("Could not encode the hardware binding record");
                return None;
            }
        };
        record.insert("hardware_attested".to_owned(), json!(self.hardware_attested));
        record.insert(
            PROOF_LEVEL_KEY.to_owned(),
            json!(self.proof_level().as_str()),
        );
        Some(Value::Object(record))
    }

    fn seal(
        &self,
        signing_input: &[u8],
        signed_content_hash: &str,
        previous_cosignature_hash: &str,
    ) -> Option<Seal> {
        let identity = match self.provider.device_identity() {
            Ok(identity) => identity,
            Err(error) => {
                log::warn!("Could not sign manifest: {error}");
                return None;
            }
        };
        let signature = match self.provider.sign(signing_input) {
            Ok(signature) => apw_provenance::hex_lower(&signature),
            Err(error) => {
                log::warn!("Could not sign manifest: {error}");
                return None;
            }
        };
        // The cosignature entangles the software signature, so it cannot precede
        // signing. It lives inside manifest_signature because the verifier
        // excludes that key when recomputing signed_content_hash.
        let cosignature = match self.provider.cosign_checkpoint(
            signed_content_hash,
            &signature,
            previous_cosignature_hash,
        ) {
            Ok(cosignature) => cosignature,
            Err(error) => {
                log::warn!("Could not sign manifest: {error}");
                return None;
            }
        };
        let cosignature_record = match serde_json::to_value(&cosignature) {
            Ok(value) => value,
            Err(error) => {
                log::warn!("Could not encode the hardware cosignature: {error}");
                return None;
            }
        };

        Some(Seal {
            record: json!({
                "algorithm": identity.algorithm,
                "device_id": identity.device_id,
                "public_key_hex": identity.public_key_hex,
                "signature_hex": signature,
                "signed_content_hash": signed_content_hash,
                "trust_scope": if self.hardware_attested {
                    TRUST_SCOPE_HARDWARE_PROVIDER
                } else {
                    TRUST_SCOPE_LOCAL_SOFTWARE
                },
                "hardware_attested": self.hardware_attested,
                PROOF_LEVEL_KEY: self.proof_level().as_str(),
                "notes": if self.hardware_attested {
                    HARDWARE_NOTES
                } else {
                    SOFTWARE_NOTES
                },
                "hardware_cosignature": cosignature_record,
            }),
            entangled_hash: cosignature.entangled_hash,
        })
    }
}
