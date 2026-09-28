//! Two-call signing for a caller-supplied signer, so this package never holds a private key.
//!
//! `ManifestSigner` is synchronous and WebCrypto, a remote HSM and a hardware token are not, so a
//! JavaScript signer cannot sit behind that trait. The split mirrors `locators`: [`prepare_signing`]
//! returns what must be signed, the caller signs it however it likes, and [`seal_manifest`] checks
//! the signature and assembles the record.
//!
//! The signature is Ed25519 over the raw 32-byte SHA-256 digest of the canonical unsigned manifest
//! (`Ed25519-SHA256`), the same wire algorithm the remote key-custody Worker returns. No API here
//! takes or returns a private key.
//!
//! The record claims `self_generated_demo_key_integrity`, the weakest existing trust scope, because
//! this package cannot observe where the caller's key lives. Identity is fixed at `not_established`
//! and cannot be raised here; only a configured trust anchor discloses a name at verification.

use audio_provenance_core::{
    Canonicalization, LOCATOR_SALT_BYTES, LocatorSalt, PortableSignature,
    ProofLevel, SignatureAlgorithm, SignerIdentity, TrustScope, canonical_json, derive_locator,
    sha256, sha256_hex, signer_id_for_public_key,
};
use audio_provenance_core::signing::PORTABLE_SIGNATURE_NOTES;
use audio_provenance_manifest::{
    HardBinding, ManifestDraft, ManifestSchema, UnverifiedManifest, seal_unsigned_value,
};
use audio_provenance_registry::SignedAt;
use apw_trace::{IngestLimits, ingest_bytes, reference_fingerprint};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::json::{coded, invalid_option, throw, to_camel_json};

const ALGORITHM: &str = "Ed25519-SHA256";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrepareOptions {
    /// 64 lowercase hex characters: the Ed25519 public key the caller's signer will sign under.
    public_key_hex: String,
    /// RFC 3339 UTC instant. Supplied by the caller; this crate reads no clock for it.
    signed_at: String,
    /// Deterministic locator salt for tests or a re-published locator. Random when absent.
    #[serde(default)]
    locator_salt_hex: Option<String>,
}

#[derive(Debug, Serialize)]
struct Prepared {
    /// Canonical JSON text of the unsigned manifest. A string, not an object, so the key rewrite
    /// applied to results cannot touch the signed field names.
    unsigned_manifest: String,
    algorithm: &'static str,
    /// Lowercase hex of the 32 bytes the caller's signer must sign.
    digest_hex: String,
    signer_id: String,
    locator: String,
    locator_salt: String,
    content_sha256: String,
    content_bytes: u64,
    decoded_audio_sha256: String,
}

/// Builds the unsigned record for `audio` and the digest a signer must sign.
#[wasm_bindgen(js_name = prepareSigning)]
pub fn prepare_signing(audio: Vec<u8>, options_json: String) -> Result<String, JsValue> {
    let options: PrepareOptions = serde_json::from_str(&options_json)
        .map_err(|error| invalid_option("signing options", &error.to_string()))?;
    let public_key = parse_public_key(&options.public_key_hex)?;
    let signed_at = SignedAt::parse(&options.signed_at)
        .map_err(|error| invalid_option("signedAt", &error.to_string()))?;
    let salt = match options.locator_salt_hex.as_deref() {
        Some(hex) => LocatorSalt::parse_hex(hex)
            .map_err(|error| invalid_option("locatorSaltHex", &error.to_string()))?,
        None => fresh_salt()?,
    };

    let ingested = ingest_bytes(audio, IngestLimits::default()).map_err(coded)?;
    let decoded = ingested.decoded_audio_sha256();
    let binding = HardBinding::new(
        ingested.content_sha256(),
        Some(ingested.content_bytes()),
        Some(&decoded),
    )
    .map_err(coded)?;
    let mut draft = ManifestDraft::new(signed_at.date(), binding)
        .map_err(coded)?
        .with_locator_salt(salt);
    if let Some(fingerprint) = reference_fingerprint(ingested.audio()).map_err(coded)? {
        draft = draft.with_fingerprint(fingerprint);
    }
    let unsigned = draft.to_unsigned_value().map_err(coded)?;
    let content = canonical_json(&unsigned).map_err(coded)?;

    to_camel_json(&Prepared {
        unsigned_manifest: String::from_utf8(content.clone())
            .map_err(|_| throw("serialization_failed", "canonical manifest is not UTF-8"))?,
        algorithm: ALGORITHM,
        digest_hex: hex::encode(sha256(&content)),
        signer_id: signer_id_for_public_key(&public_key),
        locator: hex::encode(derive_locator(&public_key, &salt)),
        locator_salt: salt.to_hex(),
        content_sha256: ingested.content_sha256().to_string(),
        content_bytes: ingested.content_bytes(),
        decoded_audio_sha256: decoded,
    })
}

/// Attaches `signature_hex` to a prepared manifest and returns the canonical record bytes.
///
/// The signature is verified under `public_key_hex` and the finished record is re-admitted through
/// the same path a verifier uses before any byte is returned, so a wrong key, a signature over other
/// bytes, or a tampered `unsigned_manifest` is refused here rather than at the verifier.
#[wasm_bindgen(js_name = sealManifest)]
pub fn seal_manifest(
    unsigned_manifest: String,
    public_key_hex: String,
    signature_hex: String,
) -> Result<Vec<u8>, JsValue> {
    let public_key = parse_public_key(&public_key_hex)?;
    let unsigned: serde_json::Value = serde_json::from_str(&unsigned_manifest)
        .map_err(|error| invalid_option("unsignedManifest", &error.to_string()))?;
    if unsigned.get("portable_signature").is_some() || unsigned.get("manifest_signature").is_some()
    {
        return Err(invalid_option(
            "unsignedManifest",
            "already carries a signature block",
        ));
    }
    let content = canonical_json(&unsigned).map_err(coded)?;
    let signature = PortableSignature {
        algorithm: SignatureAlgorithm::Ed25519Sha256,
        canonicalization: Canonicalization::ApwJsonSortV1,
        public_key_hex,
        public_key_file: String::new(),
        signer_id: signer_id_for_public_key(&public_key),
        signature_hex,
        signed_content_hash: sha256_hex(&content),
        trust_scope: TrustScope::SelfGeneratedDemoKeyIntegrity,
        signer_identity: SignerIdentity::NotEstablished,
        signer_identity_proof_level: ProofLevel::UnknownUnobserved,
        proof_level: ProofLevel::DirectlyObserved,
        notes: PORTABLE_SIGNATURE_NOTES.to_string(),
    };
    // Length and hex are checked here so a malformed signature is a named option error.
    signature
        .signature_bytes()
        .map_err(|error| invalid_option("signatureHex", &error.to_string()))?;
    let bytes = seal_unsigned_value(&unsigned, &signature).map_err(coded)?;
    UnverifiedManifest::parse(&bytes)
        .map_err(coded)?
        .admit(ManifestSchema::AudioProvenanceV1)
        .map_err(|failure| coded(failure.error))?;
    Ok(bytes)
}

fn parse_public_key(hex_key: &str) -> Result<[u8; 32], JsValue> {
    let lowercase = hex_key.bytes().all(|b| !b.is_ascii_uppercase());
    let raw = hex::decode(hex_key).ok().filter(|_| lowercase);
    raw.and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| {
            invalid_option("publicKeyHex", "expected 64 lowercase hex characters (32 bytes)")
        })
}

fn fresh_salt() -> Result<LocatorSalt, JsValue> {
    let mut raw = [0u8; LOCATOR_SALT_BYTES];
    getrandom::fill(&mut raw)
        .map_err(|_| throw("entropy_unavailable", "the platform random source failed"))?;
    Ok(LocatorSalt::from_bytes(raw))
}
