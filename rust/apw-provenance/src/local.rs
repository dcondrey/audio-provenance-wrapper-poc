use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use apw_core::{canonical_json_utf8, sha256_file, sha256_hex, utc_timestamp, ProofLevel, VerificationState};
use hmac::{Hmac, Mac};
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::SecretKey;
use serde_json::{json, Value};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::chain::{
    generate_key, hex_decode, hex_lower, issue_leaf, issue_root, key_from_pkcs8_pem, key_id_of,
    key_to_pkcs8_pem, random_bytes, validate_certificate_chain, DEFAULT_CREATOR_COMMON_NAME,
    SIGNING_ALGORITHM,
};
use crate::error::{ProvenanceError, Result, RevokedKeyError};
use crate::mark::{
    descriptor_similarity, mark_limits, mark_mechanism, quantise_descriptor, FeatureSource,
    PcmFeatureSource, MARK_MATCH_THRESHOLD, MARK_WINDOW_COUNT, MARK_WINDOW_SECONDS,
};
use crate::provider::{
    IdentityReport, MarkAttachment, MatchedBy, ProvenanceProvider, RecoveredMark, RegistryReceipt,
    RevocationRecord, SigningIdentity, SigningMaterial, SigningRecord, VerificationOutcome,
};

pub const DEFAULT_STORE: &str = "~/.apw/provenance";

const IDENTITY_EVIDENCE: &str = "locally_generated_key_no_external_attestation";
const IDENTITY_LIMITS: &str = "The certificate binds a key to a name this machine chose. No \
     external party verified that the name belongs to the creator.";
const MARK_DOMAIN: &[u8] = b"apw-soft-binding-v1";
const CLAIM_TRUST_OK: &str = "claim signature and certificate chain validate";

/// Expand a leading `~` the way `pathlib.Path.expanduser()` does.
///
/// IMPORTANT: the Python store path is `~/.apw/provenance`. A Rust store that
/// skipped expansion would create a literal `./~` directory beside the daemon and
/// silently diverge from the identity the demo signs with.
pub fn expand_user(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let Some(rest) = text.strip_prefix('~') else {
        return path.to_path_buf();
    };
    let Some(home) = std::env::var_os("HOME") else {
        return path.to_path_buf();
    };
    let trimmed = rest.strip_prefix('/').unwrap_or(rest);
    if trimmed.is_empty() {
        PathBuf::from(home)
    } else {
        PathBuf::from(home).join(trimmed)
    }
}

/// File-backed reference implementation of the vault/mark/registry contract.
///
/// Key custody lives under `<store>/ca` and `<store>/keys`, signing and revocation
/// history in `history.jsonl`, the registry in `registry.jsonl`, and the soft
/// binding side index in `marks.jsonl`. Every log is append-only.
///
/// IMPORTANT: identities issued here are self-asserted. Their proof level is
/// [`ProofLevel::UserDeclared`]; no external party has attested to the creator's
/// identity, and possession of a key is not verified identity.
pub struct LocalReferenceProvider {
    store_dir: PathBuf,
    common_name: String,
    history_path: PathBuf,
    registry_path: PathBuf,
    marks_path: PathBuf,
    root_cert_pem: String,
    leaf_key: SecretKey,
    leaf_cert_pem: String,
    key_id: String,
    leaf_not_before_ms: i64,
    mark_key: Zeroizing<Vec<u8>>,
    features: Box<dyn FeatureSource>,
}

impl core::fmt::Debug for LocalReferenceProvider {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LocalReferenceProvider")
            .field("store_dir", &self.store_dir)
            .field("common_name", &self.common_name)
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

impl LocalReferenceProvider {
    pub fn new(store_dir: &Path) -> Result<Self> {
        Self::with_feature_source(
            store_dir,
            DEFAULT_CREATOR_COMMON_NAME,
            Box::new(PcmFeatureSource),
        )
    }

    pub fn with_common_name(store_dir: &Path, common_name: &str) -> Result<Self> {
        Self::with_feature_source(store_dir, common_name, Box::new(PcmFeatureSource))
    }

    pub fn with_feature_source(
        store_dir: &Path,
        common_name: &str,
        features: Box<dyn FeatureSource>,
    ) -> Result<Self> {
        let store_dir = expand_user(store_dir);
        create_dir(&store_dir)?;
        let (root_key, root_cert_pem) = load_or_create_root(&store_dir)?;
        let (leaf_key, leaf_cert_pem, key_id) =
            load_or_create_leaf(&store_dir, common_name, &root_key, &root_cert_pem)?;
        let leaf_not_before_ms = not_before_ms(&leaf_cert_pem)?;
        let mark_key = load_or_create_mark_key(&store_dir)?;
        Ok(LocalReferenceProvider {
            history_path: store_dir.join("history.jsonl"),
            registry_path: store_dir.join("registry.jsonl"),
            marks_path: store_dir.join("marks.jsonl"),
            store_dir,
            common_name: common_name.to_string(),
            root_cert_pem,
            leaf_key,
            leaf_cert_pem,
            key_id,
            leaf_not_before_ms,
            mark_key,
            features,
        })
    }

    pub fn store_dir(&self) -> &Path {
        &self.store_dir
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn trust_anchor_pem(&self) -> Vec<u8> {
        self.root_cert_pem.as_bytes().to_vec()
    }

    /// Leaf-then-issuer PEM, the ordering the C2PA signer requires.
    pub fn chain_pem(&self) -> Vec<u8> {
        let mut chain = self.leaf_cert_pem.clone();
        chain.push_str(&self.root_cert_pem);
        chain.into_bytes()
    }

    fn meta_path(&self, key_id: &str) -> PathBuf {
        self.store_dir.join("keys").join(key_id).join("meta.json")
    }

    fn meta(&self, key_id: &str) -> Result<Value> {
        let path = self.meta_path(key_id);
        if !path.is_file() {
            return Err(ProvenanceError::UnknownKey {
                key_id: key_id.to_string(),
            });
        }
        read_json(&path)
    }

    fn revoked(&self, key_id: &str) -> Result<bool> {
        let meta = self.meta(key_id)?;
        Ok(meta.get("revoked").is_some_and(apw_core::is_truthy))
    }

    fn descriptor(&self, audio_path: &Path) -> Result<Vec<i64>> {
        let sequence =
            self.features
                .feature_sequence(audio_path, MARK_WINDOW_SECONDS, MARK_WINDOW_COUNT)?;
        Ok(quantise_descriptor(&sequence))
    }

    fn registry_records(&self) -> Result<Vec<RegistryReceipt>> {
        read_jsonl(&self.registry_path)?
            .into_iter()
            .map(|record| {
                serde_json::from_value(record).map_err(|source| ProvenanceError::Record {
                    path: self.registry_path.clone(),
                    source,
                })
            })
            .collect()
    }

    /// Locate the registry record backing an asset, and say which key found it.
    ///
    /// IMPORTANT: the match key drives the reported proof level. A content or
    /// manifest digest is an exact match; a mark is a similarity match and can
    /// only ever support an `inferred` claim.
    fn lookup(
        &self,
        content_sha256: &str,
        mark_id: Option<&str>,
        manifest_sha256: Option<&str>,
    ) -> Result<(Option<RegistryReceipt>, MatchedBy)> {
        let records = self.registry_records()?;
        if let Some(found) = records
            .iter()
            .rev()
            .find(|record| record.content_sha256 == content_sha256)
        {
            return Ok((Some(found.clone()), MatchedBy::ContentSha256));
        }
        if let Some(digest) = manifest_sha256 {
            if let Some(found) = records
                .iter()
                .rev()
                .find(|record| record.manifest_sha256 == digest)
            {
                return Ok((Some(found.clone()), MatchedBy::ManifestSha256));
            }
        }
        if let Some(mark) = mark_id {
            if let Some(found) = records
                .iter()
                .rev()
                .find(|record| record.mark_id.as_deref() == Some(mark))
            {
                return Ok((Some(found.clone()), MatchedBy::MarkId));
            }
        }
        Ok((None, MatchedBy::None))
    }

    fn claim_trust(
        &self,
        record: &RegistryReceipt,
        manifest_bytes: Option<&[u8]>,
    ) -> Result<(bool, String)> {
        let validation =
            validate_certificate_chain(record.chain_pem.as_bytes(), &self.trust_anchor_pem());
        if !validation.valid {
            return Ok((false, validation.reason));
        }
        match self.revoked(&record.key_id) {
            Ok(true) => return Ok((false, "signing key is revoked".to_string())),
            Ok(false) => {}
            Err(ProvenanceError::UnknownKey { .. }) => {
                return Ok((
                    false,
                    "signing key is not present in this custody store".to_string(),
                ))
            }
            Err(error) => return Err(error),
        }
        if let Some(bytes) = manifest_bytes {
            if sha256_hex(bytes) != record.manifest_sha256 {
                return Ok((
                    false,
                    "supplied manifest does not match the registered manifest digest".to_string(),
                ));
            }
            if !claim_signature_verifies(record, bytes) {
                return Ok((false, "claim signature does not verify".to_string()));
            }
        }
        Ok((true, CLAIM_TRUST_OK.to_string()))
    }

}

impl ProvenanceProvider for LocalReferenceProvider {
    fn identity(&self) -> Result<IdentityReport> {
        let revoked = self.revoked(&self.key_id)?;
        Ok(IdentityReport {
            identity: SigningIdentity {
                key_id: self.key_id.clone(),
                subject_common_name: self.common_name.clone(),
                algorithm: SIGNING_ALGORITHM.to_string(),
                identity_evidence: IDENTITY_EVIDENCE.to_string(),
                revoked,
                created_at_ms: self.leaf_not_before_ms,
                proof_level: ProofLevel::UserDeclared,
            },
            trust_anchor_sha256: sha256_hex(&self.trust_anchor_pem()),
            limits: IDENTITY_LIMITS.to_string(),
        })
    }

    fn issue_signing_material(&self) -> Result<SigningMaterial> {
        if self.revoked(&self.key_id)? {
            return Err(RevokedKeyError {
                key_id: self.key_id.clone(),
                attempted: RevokedKeyError::ISSUE_MATERIAL,
            }
            .into());
        }
        let handle = key_to_pkcs8_pem(&self.leaf_key)?;
        Ok(SigningMaterial {
            key_id: self.key_id.clone(),
            algorithm: SIGNING_ALGORITHM.to_string(),
            certificate_chain_pem: self.chain_pem(),
            private_key_handle: Zeroizing::new(handle.as_bytes().to_vec()),
            trust_anchor_pem: self.trust_anchor_pem(),
            proof_level: ProofLevel::UserDeclared,
        })
    }

    fn sign_claim(&self, payload: &[u8]) -> Result<Vec<u8>> {
        if self.revoked(&self.key_id)? {
            return Err(RevokedKeyError {
                key_id: self.key_id.clone(),
                attempted: RevokedKeyError::SIGN_CLAIM,
            }
            .into());
        }
        let signing_key = SigningKey::from(&self.leaf_key);
        let signature: Signature = signing_key.sign(payload);
        let der = signature.to_der().as_bytes().to_vec();
        append_only(
            &self.history_path,
            &json!({
                "event": "sign",
                "key_id": self.key_id,
                "at": utc_timestamp(None),
                "payload_sha256": sha256_hex(payload),
                "signature_hex": hex_lower(&der),
                "apw:proof_level": ProofLevel::DirectlyObserved.as_str(),
            }),
        )?;
        Ok(der)
    }

    /// Attach a keyed feature-domain soft binding via a local side index.
    ///
    /// IMPORTANT: this does not modify a single audio sample. The "mark" is an
    /// HMAC over a coarse feature descriptor of the asset, stored in a local side
    /// index. It is a working, deterministic soft binding and nothing more: no
    /// inaudibility and no transcode survival is claimed, because neither has
    /// been measured. Its proof level is `inferred`, never `directly_observed`.
    fn embed_mark(&self, audio_path: &Path, payload: &Value) -> Result<MarkAttachment> {
        let descriptor = self.descriptor(audio_path)?;
        if descriptor.is_empty() {
            return Err(ProvenanceError::AssetTooShort {
                path: audio_path.to_path_buf(),
            });
        }
        let content_sha256 = sha256_file(audio_path)?;
        let descriptor_value = serde_json::to_value(&descriptor)?;

        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.mark_key)
            .map_err(|error| ProvenanceError::KeyMaterial(error.to_string()))?;
        mac.update(MARK_DOMAIN);
        mac.update(&canonical_json_utf8(&descriptor_value)?);
        mac.update(&canonical_json_utf8(payload)?);
        mac.update(content_sha256.as_bytes());
        let mark_id = hex_lower(&mac.finalize().into_bytes());

        append_only(
            &self.marks_path,
            &json!({
                "mark_id": mark_id,
                "descriptor": descriptor_value,
                "content_sha256": content_sha256,
                "payload": payload,
                "created_at": utc_timestamp(None),
            }),
        )?;

        Ok(MarkAttachment {
            mark_id,
            content_sha256,
            asset_modified: false,
            mechanism: mark_mechanism(),
            proof_level: ProofLevel::Inferred,
            limits: mark_limits(),
        })
    }

    fn recover_mark(&self, audio_path: &Path) -> Result<Option<RecoveredMark>> {
        if !self.marks_path.is_file() {
            return Ok(None);
        }
        // An asset this provider cannot describe has no mark here. It is not an
        // error, and it is never evidence of synthetic origin.
        let Ok(descriptor) = self.descriptor(audio_path) else {
            return Ok(None);
        };
        if descriptor.is_empty() {
            return Ok(None);
        }
        let mut best: Option<Value> = None;
        let mut best_similarity = 0.0f64;
        for record in read_jsonl(&self.marks_path)? {
            let candidate: Vec<i64> = record
                .get("descriptor")
                .and_then(Value::as_array)
                .map(|values| values.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            let similarity = descriptor_similarity(&descriptor, &candidate);
            if similarity > best_similarity {
                best_similarity = similarity;
                best = Some(record);
            }
        }
        let Some(record) = best else {
            return Ok(None);
        };
        if best_similarity < MARK_MATCH_THRESHOLD {
            return Ok(None);
        }
        Ok(Some(RecoveredMark {
            mark_id: record
                .get("mark_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            payload: record.get("payload").cloned().unwrap_or(Value::Null),
            registered_content_sha256: record
                .get("content_sha256")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            observed_content_sha256: sha256_file(audio_path)?,
            match_similarity: (best_similarity * 1e6).round() / 1e6,
            proof_level: ProofLevel::Inferred,
        }))
    }

    fn register(&self, manifest: &Value) -> Result<RegistryReceipt> {
        let content_sha256 = manifest
            .get("content_sha256")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if content_sha256.len() != 64 {
            return Err(ProvenanceError::MissingContentDigest);
        }
        let content_sha256 = content_sha256.to_string();
        let manifest_bytes = canonical_json_utf8(manifest)?;
        let manifest_sha256 = sha256_hex(&manifest_bytes);
        let signature = self.sign_claim(&manifest_bytes)?;

        let mut registry_input = manifest_sha256.clone();
        registry_input.push_str(&content_sha256);
        let registry_id = sha256_hex(registry_input.as_bytes())
            .get(..32)
            .unwrap_or_default()
            .to_string();

        let receipt = RegistryReceipt {
            registry_id,
            content_sha256,
            manifest_sha256,
            mark_id: manifest
                .get("mark_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            key_id: self.key_id.clone(),
            chain_pem: String::from_utf8_lossy(&self.chain_pem()).into_owned(),
            signature_hex: hex_lower(&signature),
            registered_at: utc_timestamp(None),
            proof_level: ProofLevel::DirectlyObserved,
        };
        append_only(&self.registry_path, &serde_json::to_value(&receipt)?)?;
        Ok(receipt)
    }

    fn verify(
        &self,
        asset_path: &Path,
        manifest_bytes: Option<&[u8]>,
    ) -> Result<VerificationOutcome> {
        let content_sha256 = sha256_file(asset_path)?;
        let mark = self.recover_mark(asset_path)?;
        let manifest_sha256 = manifest_bytes.map(sha256_hex);
        let (record, matched_by) = self.lookup(
            &content_sha256,
            mark.as_ref().map(|found| found.mark_id.as_str()),
            manifest_sha256.as_deref(),
        )?;

        let (state, reason) = match &record {
            None if mark.is_none() && manifest_bytes.is_none() => (
                VerificationState::NothingFound,
                "no mark and no registry record".to_string(),
            ),
            None => (
                VerificationState::MarkFoundClaimNotTrusted,
                "provenance data was presented but no registry record backs it".to_string(),
            ),
            Some(found) => {
                let (trusted, reason) = self.claim_trust(found, manifest_bytes)?;
                if !trusted {
                    (VerificationState::MarkFoundClaimNotTrusted, reason)
                } else if found.content_sha256 != content_sha256 {
                    (
                        VerificationState::RegisteredButChanged,
                        "asset bytes differ from the registered content digest".to_string(),
                    )
                } else {
                    (VerificationState::Verified, reason)
                }
            }
        };

        Ok(VerificationOutcome {
            state,
            reason,
            content_sha256,
            registry_id: record.map(|found| found.registry_id),
            mark,
            matched_by,
            proof_level: matched_by.proof_level(),
            normative_note: apw_core::NOTHING_FOUND_NORMATIVE_NOTE,
        })
    }

    fn revoke(&self, key_id: &str) -> Result<RevocationRecord> {
        let mut meta = self.meta(key_id)?;
        let revoked_at = utc_timestamp(None);
        if let Some(map) = meta.as_object_mut() {
            map.insert("revoked".to_string(), Value::Bool(true));
            map.insert("revoked_at".to_string(), Value::String(revoked_at.clone()));
        }
        write_pretty(&self.meta_path(key_id), &meta)?;

        let record = RevocationRecord {
            event: "revoke".to_string(),
            key_id: key_id.to_string(),
            at: revoked_at,
            // REQUIRED: counted before the revocation record is appended, so it
            // states how many signing records survive revocation.
            retained_signing_records: self.signing_history(key_id)?.len(),
            proof_level: ProofLevel::DirectlyObserved,
        };
        append_only(&self.history_path, &serde_json::to_value(&record)?)?;
        log::info!("Revoked provenance key {key_id}; signing history retained");
        Ok(record)
    }

    fn signing_history(&self, key_id: &str) -> Result<Vec<SigningRecord>> {
        let mut history = Vec::new();
        for record in read_jsonl(&self.history_path)? {
            let is_signature = record.get("event").and_then(Value::as_str) == Some("sign")
                && record.get("key_id").and_then(Value::as_str) == Some(key_id);
            if !is_signature {
                continue;
            }
            history.push(
                serde_json::from_value(record).map_err(|source| ProvenanceError::Record {
                    path: self.history_path.clone(),
                    source,
                })?,
            );
        }
        Ok(history)
    }
}

fn claim_signature_verifies(record: &RegistryReceipt, manifest_bytes: &[u8]) -> bool {
    let Some(signature_der) = hex_decode(&record.signature_hex) else {
        return false;
    };
    let Some(leaf_der) = x509_parser::pem::Pem::iter_from_buffer(record.chain_pem.as_bytes())
        .next()
        .and_then(core::result::Result::ok)
        .map(|entry| entry.contents)
    else {
        return false;
    };
    let Ok((_, leaf)) = x509_parser::parse_x509_certificate(&leaf_der) else {
        return false;
    };
    let Ok(verifying_key) = VerifyingKey::from_sec1_bytes(&leaf.public_key().subject_public_key.data)
    else {
        return false;
    };
    let Ok(signature) = Signature::from_der(&signature_der) else {
        return false;
    };
    verifying_key.verify(manifest_bytes, &signature).is_ok()
}

fn load_or_create_root(store_dir: &Path) -> Result<(SecretKey, String)> {
    let key_path = store_dir.join("ca").join("root_key.pem");
    let cert_path = store_dir.join("ca").join("root_cert.pem");
    if key_path.is_file() && cert_path.is_file() {
        let key = key_from_pkcs8_pem(&read_text(&key_path)?)?;
        return Ok((key, read_text(&cert_path)?));
    }
    let key = generate_key();
    let certificate = issue_root(&key)?;
    write_private(&key_path, key_to_pkcs8_pem(&key)?.as_bytes())?;
    write_public(&cert_path, certificate.as_bytes())?;
    log::info!("Created local provenance root CA at {}", cert_path.display());
    Ok((key, certificate))
}

fn load_or_create_leaf(
    store_dir: &Path,
    common_name: &str,
    root_key: &SecretKey,
    root_cert_pem: &str,
) -> Result<(SecretKey, String, String)> {
    let key_dir = store_dir.join("keys");
    let active = key_dir.join("active.json");
    if active.is_file() {
        let record = read_json(&active)?;
        let key_id = record
            .get("key_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProvenanceError::UnknownKey {
                key_id: "active".to_string(),
            })?
            .to_string();
        let key = key_from_pkcs8_pem(&read_text(&key_dir.join(&key_id).join("leaf_key.pem"))?)?;
        let certificate = read_text(&key_dir.join(&key_id).join("leaf_cert.pem"))?;
        return Ok((key, certificate, key_id));
    }

    let key = generate_key();
    let certificate = issue_leaf(&key, common_name, root_key, root_cert_pem)?;
    let key_id = key_id_of(&key.public_key())?;
    write_private(
        &key_dir.join(&key_id).join("leaf_key.pem"),
        key_to_pkcs8_pem(&key)?.as_bytes(),
    )?;
    write_public(
        &key_dir.join(&key_id).join("leaf_cert.pem"),
        certificate.as_bytes(),
    )?;
    write_pretty(
        &key_dir.join(&key_id).join("meta.json"),
        &json!({
            "key_id": key_id,
            "common_name": common_name,
            "created_at": utc_timestamp(None),
            "revoked": false,
        }),
    )?;
    write_pretty(&active, &json!({ "key_id": key_id }))?;
    log::info!("Issued local provenance leaf certificate {key_id}");
    Ok((key, certificate, key_id))
}

fn load_or_create_mark_key(store_dir: &Path) -> Result<Zeroizing<Vec<u8>>> {
    let path = store_dir.join("mark_key.bin");
    if path.is_file() {
        return Ok(Zeroizing::new(read_bytes(&path)?));
    }
    let key = random_bytes(32);
    write_private(&path, &key)?;
    Ok(key)
}

fn not_before_ms(certificate_pem: &str) -> Result<i64> {
    let der = x509_parser::pem::Pem::iter_from_buffer(certificate_pem.as_bytes())
        .next()
        .and_then(core::result::Result::ok)
        .map(|entry| entry.contents)
        .ok_or_else(|| ProvenanceError::Issuance("leaf certificate is not PEM".to_string()))?;
    let (_, certificate) = x509_parser::parse_x509_certificate(&der)
        .map_err(|error| ProvenanceError::Issuance(error.to_string()))?;
    Ok(certificate
        .validity()
        .not_before
        .timestamp()
        .saturating_mul(1000))
}

fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|source| ProvenanceError::io("create", path, source))
}

fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|source| ProvenanceError::io("read", path, source))
}

fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|source| ProvenanceError::io("read", path, source))
}

fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(&read_bytes(path)?).map_err(|source| ProvenanceError::Record {
        path: path.to_path_buf(),
        source,
    })
}

fn read_jsonl(path: &Path) -> Result<Vec<Value>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = read_text(path)?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|source| ProvenanceError::Record {
                path: path.to_path_buf(),
                source,
            })
        })
        .collect()
}

/// IMPORTANT: these logs are append-only evidence, so `apw_core::append_jsonl` is
/// deliberately not used: it rotates and drops oversize records, which would
/// silently remove registry and signing history a verifier depends on.
fn append_only(path: &Path, record: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_dir(parent)?;
    }
    let mut line = canonical_json_utf8(record)?;
    line.push(b'\n');
    let mut handle = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| ProvenanceError::io("append to", path, source))?;
    handle
        .write_all(&line)
        .map_err(|source| ProvenanceError::io("append to", path, source))
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_dir(parent)?;
    }
    fs::write(path, data).map_err(|source| ProvenanceError::io("write", path, source))?;
    set_mode(path, 0o600)
}

fn write_public(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_dir(parent)?;
    }
    fs::write(path, data).map_err(|source| ProvenanceError::io("write", path, source))
}

fn write_pretty(path: &Path, value: &Value) -> Result<()> {
    // Python writes these with json.dumps(indent=2) and no trailing newline.
    write_public(path, &serde_json::to_vec_pretty(value)?)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|source| ProvenanceError::io("set permissions on", path, source))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}
