//! The one capture-to-SDK boundary.
//!
//! The adapter admits the Python daemon's signed `audio-provenance-manifest-v0` record before it
//! reads any claim from it, requires the separately supplied handoff to be the exact value covered
//! by that signature, and then constructs a new SDK record only through [`crate::sign`]. Locator
//! allocation is derived from immutable input digests, so retrying after a crash produces the same
//! canonical record and record id.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use audio_provenance_core::{LocatorSalt, ManifestSigner, ProofLevel, sha256, sha256_hex};
use audio_provenance_manifest::{ManifestSchema, ProvenanceClaim, UnverifiedManifest};
use audio_provenance_registry::WritableRegistryBackend;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::SdkError;
use crate::producer::{PublishReceipt, SidecarOutput, SignOptions, SignResult, publish, sign};
use crate::verify::{VerifyOptions, verify};
use apw_trace::{SidecarPolicy, VerifyResult};

const MAX_HANDOFF_BYTES: u64 = 1_048_576;
const ADAPTER_DOMAIN: &[u8] = b"audio-provenance-capture-adapter-v1\0";

/// Development-only processing options. No identity authority is accepted here: the SDK signing
/// key proves key possession and the resulting identity remains `not_established` unless a
/// verifier independently supplies a trust anchor later.
#[derive(Debug)]
pub struct CaptureAdapterOptions {
    signer: Box<dyn ManifestSigner>,
    public_key_file: String,
    embedded_output: Option<PathBuf>,
    sidecar: SidecarOutput,
    expected_evidence_bundle_sha256: Option<String>,
    registry: Option<Box<dyn WritableRegistryBackend>>,
    mode: CaptureAdapterMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureAdapterMode {
    Development,
    ProductionRemoteCustody,
}

impl CaptureAdapterOptions {
    pub fn new(signer: impl ManifestSigner + 'static) -> Self {
        Self {
            signer: Box::new(signer),
            public_key_file: "ephemeral-development.key".to_string(),
            embedded_output: None,
            sidecar: SidecarOutput::Conventional,
            expected_evidence_bundle_sha256: None,
            registry: None,
            mode: CaptureAdapterMode::Development,
        }
    }

    /// Selects production remote custody. Development-only claims and receipt overrides are
    /// omitted; identity remains a verifier-side trust decision rather than an HSM assertion.
    pub fn production(signer: impl ManifestSigner + 'static) -> Self {
        Self {
            signer: Box::new(signer),
            public_key_file: "remote-key-custody".to_string(),
            embedded_output: None,
            sidecar: SidecarOutput::Conventional,
            expected_evidence_bundle_sha256: None,
            registry: None,
            mode: CaptureAdapterMode::ProductionRemoteCustody,
        }
    }

    #[must_use]
    pub fn public_key_file(mut self, label: impl Into<String>) -> Self {
        self.public_key_file = label.into();
        self
    }

    /// Write one `aprv` slot into a copy of the export. The capture export is never overwritten.
    #[must_use]
    pub fn embedded_output(mut self, output: impl Into<PathBuf>) -> Self {
        self.embedded_output = Some(output.into());
        self.sidecar = SidecarOutput::None;
        self
    }

    /// Write the SDK record as a sidecar. Conventional is the default.
    #[must_use]
    pub fn sidecar(mut self, output: SidecarOutput) -> Self {
        self.embedded_output = None;
        self.sidecar = output;
        self
    }

    /// Require a caller-supplied bundle digest as well as recomputing it locally.
    pub fn expected_evidence_bundle_sha256(
        mut self,
        digest: impl Into<String>,
    ) -> Result<Self, SdkError> {
        let digest = digest.into();
        require_sha256_hex("evidence_bundle_sha256", &digest)?;
        self.expected_evidence_bundle_sha256 = Some(digest);
        Ok(self)
    }

    #[must_use]
    pub fn registry(mut self, registry: impl WritableRegistryBackend + 'static) -> Self {
        self.registry = Some(Box::new(registry));
        self
    }
}

/// The immutable receipt a capture orchestrator records beside its already-signed capture
/// manifest. The manifest itself is not mutated after signing: doing so would invalidate it, and
/// including this record id in the evidence bundle that this record hashes would be circular.
#[derive(Debug, Serialize)]
pub struct CaptureAdapterResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub development_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<&'static str>,
    pub capture_manifest_sha256: String,
    pub capture_handoff_sha256: String,
    pub evidence_bundle_sha256: String,
    pub proof_objects_preserved: usize,
    pub sign: SignResult,
    pub publish: Option<PublishReceipt>,
    pub verification: VerifyResult,
}

/// Admit capture output, translate it without proof promotion, sign/embed, optionally publish, and
/// verify via the same public SDK entry point third parties use.
pub fn adapt_capture_export(
    export_path: &Path,
    capture_manifest_path: &Path,
    handoff_path: &Path,
    evidence_bundle_path: &Path,
    options: CaptureAdapterOptions,
) -> Result<CaptureAdapterResult, SdkError> {
    if let Some(output) = &options.embedded_output
        && paths_alias(export_path, output)?
    {
        return Err(SdkError::CaptureAdapterInvalid {
            reason: "embedded output must be a copy; the capture export is never overwritten"
                .to_string(),
        });
    }
    let capture_manifest_bytes = read_limited(
        capture_manifest_path,
        audio_provenance_manifest::MAX_MANIFEST_BYTES as u64,
    )?;
    let capture = UnverifiedManifest::parse(&capture_manifest_bytes)?
        .admit(ManifestSchema::ApwV0)
        .map_err(|failure| failure.error)?;

    let handoff_bytes = read_limited(handoff_path, MAX_HANDOFF_BYTES)?;
    let handoff: Value = serde_json::from_slice(&handoff_bytes).map_err(|error| {
        SdkError::CaptureAdapterInvalid {
            reason: format!("capture handoff is not valid JSON: {error}"),
        }
    })?;
    validate_handoff(&capture, &handoff)?;

    let evidence_bundle_sha256 = hash_file(evidence_bundle_path)?;
    if let Some(expected) = &options.expected_evidence_bundle_sha256
        && expected != &evidence_bundle_sha256
    {
        return Err(SdkError::DigestMismatch {
            what: "evidence bundle",
            expected: expected.clone(),
            observed: evidence_bundle_sha256,
        });
    }

    let capture_manifest_sha256 = sha256_hex(&capture_manifest_bytes);
    let capture_handoff_sha256 = sha256_hex(&handoff_bytes);
    let salt = stable_locator_salt(
        options.signer.as_ref(),
        &capture_manifest_sha256,
        &capture_handoff_sha256,
        &evidence_bundle_sha256,
    );

    let signed_at = capture
        .value()
        .get("created_at")
        .and_then(Value::as_str)
        .ok_or_else(|| SdkError::CaptureAdapterInvalid {
            reason: "admitted capture manifest has no RFC 3339 created_at".to_string(),
        })?;

    let verification_sidecar = match &options.sidecar {
        SidecarOutput::Conventional => SidecarPolicy::Conventional,
        SidecarOutput::Explicit(path) => SidecarPolicy::Explicit(path.clone()),
        SidecarOutput::None => SidecarPolicy::Disabled,
    };
    let mode = options.mode;
    let mut sign_options = SignOptions::new(BoxedManifestSigner(options.signer), signed_at)?
        .public_key_file(options.public_key_file)
        .with_locator_salt(salt)
        .sidecar(options.sidecar)
        .with_claim(digest_claim(
            "capture_manifest_sha256",
            &capture_manifest_sha256,
        ))
        .with_claim(digest_claim(
            "capture_handoff_sha256",
            &capture_handoff_sha256,
        ))
        .with_claim(digest_claim(
            "evidence_bundle_sha256",
            &evidence_bundle_sha256,
        ));

    if mode == CaptureAdapterMode::Development {
        sign_options = sign_options.with_claim(ProvenanceClaim {
            claim: "development_only".to_string(),
            value: Value::Bool(true),
            evidence: "capture-to-SDK adapter was explicitly configured in development mode"
                .to_string(),
            proof_level: ProofLevel::DirectlyObserved,
        });
    }

    if let Some(output) = &options.embedded_output {
        sign_options = sign_options
            .out(output.clone())
            .embed_manifest(true)
            .sidecar(SidecarOutput::None);
    }
    if let Some(association) = capture.association() {
        sign_options = sign_options.with_association(*association);
    }
    if let Some(coverage) = capture.observation_coverage() {
        sign_options = sign_options.with_coverage(*coverage);
    }
    for claim in capture.claims() {
        sign_options = sign_options.with_claim(claim.clone());
    }

    let mut copied = Vec::new();
    copy_proof_objects(&handoff, "", &mut copied)?;
    let proof_objects_preserved = copied.len();
    for claim in copied {
        sign_options = sign_options.with_claim(claim);
    }

    let signed = sign(export_path, &sign_options)?;
    let declared_export = handoff
        .pointer("/export_hard_hash/value")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if signed.content_sha256 != declared_export {
        return Err(SdkError::DigestMismatch {
            what: "capture export",
            expected: declared_export.to_string(),
            observed: signed.content_sha256,
        });
    }

    let published = options
        .registry
        .as_ref()
        .map(|registry| publish(registry.as_ref(), &signed))
        .transpose()?;

    let verification_path = options.embedded_output.as_deref().unwrap_or(export_path);
    let verification = verify(
        verification_path,
        &VerifyOptions::new().with_sidecar(verification_sidecar),
    )?;
    if verification
        .manifest
        .as_ref()
        .map(|manifest| manifest.bytes())
        != Some(signed.manifest_bytes.as_slice())
    {
        return Err(SdkError::CaptureAdapterInvalid {
            reason: "public SDK verification did not recover the record the adapter wrote"
                .to_string(),
        });
    }

    Ok(CaptureAdapterResult {
        development_only: (mode == CaptureAdapterMode::Development).then_some(true),
        identity: (mode == CaptureAdapterMode::Development).then_some("not_established"),
        capture_manifest_sha256,
        capture_handoff_sha256,
        evidence_bundle_sha256,
        proof_objects_preserved,
        sign: signed,
        publish: published,
        verification,
    })
}

/// Atomically writes the adapter receipt. Re-running with the same inputs replaces it with the same
/// semantic result while never exposing a partial JSON document.
pub fn write_capture_adapter_receipt(
    path: &Path,
    result: &CaptureAdapterResult,
) -> Result<(), SdkError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        std::fs::create_dir_all(parent).map_err(|source| SdkError::io(parent, source))?;
    }
    let bytes =
        serde_json::to_vec_pretty(result).map_err(|error| SdkError::CaptureAdapterInvalid {
            reason: format!("could not serialise adapter receipt: {error}"),
        })?;
    let temporary = path.with_extension("tmp");
    let mut file = File::create(&temporary).map_err(|source| SdkError::io(&temporary, source))?;
    file.write_all(&bytes)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|source| SdkError::io(&temporary, source))?;
    std::fs::rename(&temporary, path).map_err(|source| SdkError::io(path, source))
}

fn validate_handoff(
    capture: &audio_provenance_manifest::Manifest,
    handoff: &Value,
) -> Result<(), SdkError> {
    if handoff.get("record_type").and_then(Value::as_str)
        != Some("downstream_provenance_registration_handoff")
    {
        return Err(SdkError::CaptureAdapterInvalid {
            reason: "handoff record_type is not downstream_provenance_registration_handoff"
                .to_string(),
        });
    }
    require_sha256_hex(
        "export_hard_hash.value",
        handoff
            .pointer("/export_hard_hash/value")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    let signed_handoff = capture
        .value()
        .get("downstream_registration_handoff")
        .ok_or_else(|| SdkError::CaptureAdapterInvalid {
            reason: "capture manifest does not sign a downstream_registration_handoff".to_string(),
        })?;
    if signed_handoff != handoff {
        return Err(SdkError::CaptureAdapterInvalid {
            reason: "handoff differs from the value covered by the admitted capture manifest"
                .to_string(),
        });
    }
    let capture_session = capture.value().get("session_id").and_then(Value::as_str);
    if handoff.get("capture_session_id").and_then(Value::as_str) != capture_session {
        return Err(SdkError::CaptureAdapterInvalid {
            reason: "handoff capture_session_id differs from the admitted manifest".to_string(),
        });
    }
    if handoff
        .pointer("/signing_key/signer_identity")
        .and_then(Value::as_str)
        != Some("not_established")
    {
        return Err(SdkError::CaptureAdapterInvalid {
            reason: "capture handoff must keep signer_identity at not_established".to_string(),
        });
    }
    Ok(())
}

fn stable_locator_salt(
    signer: &dyn ManifestSigner,
    capture_manifest_sha256: &str,
    capture_handoff_sha256: &str,
    evidence_bundle_sha256: &str,
) -> LocatorSalt {
    let mut material = Vec::with_capacity(ADAPTER_DOMAIN.len() + 32 + 64 * 3);
    material.extend_from_slice(ADAPTER_DOMAIN);
    material.extend_from_slice(&signer.public_key_bytes());
    material.extend_from_slice(capture_manifest_sha256.as_bytes());
    material.extend_from_slice(capture_handoff_sha256.as_bytes());
    material.extend_from_slice(evidence_bundle_sha256.as_bytes());
    let digest = sha256(&material);
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&digest[..16]);
    LocatorSalt::from_bytes(salt)
}

#[derive(Debug)]
struct BoxedManifestSigner(Box<dyn ManifestSigner>);

impl ManifestSigner for BoxedManifestSigner {
    fn public_key_bytes(&self) -> [u8; 32] {
        self.0.public_key_bytes()
    }

    fn sign_manifest(
        &self,
        unsigned_manifest: &Value,
        public_key_file: &str,
    ) -> Result<audio_provenance_core::PortableSignature, audio_provenance_core::SignatureError>
    {
        self.0.sign_manifest(unsigned_manifest, public_key_file)
    }
}

fn digest_claim(name: &str, digest: &str) -> ProvenanceClaim {
    ProvenanceClaim {
        claim: name.to_string(),
        value: Value::String(digest.to_string()),
        evidence: "SHA-256 recomputed by the capture adapter".to_string(),
        proof_level: ProofLevel::DirectlyObserved,
    }
}

fn copy_proof_objects(
    value: &Value,
    pointer: &str,
    output: &mut Vec<ProvenanceClaim>,
) -> Result<(), SdkError> {
    match value {
        Value::Object(map) => {
            if let Some(level) = map.get("apw:proof_level") {
                let proof_level: ProofLevel =
                    serde_json::from_value(level.clone()).map_err(|error| {
                        SdkError::CaptureAdapterInvalid {
                            reason: format!("invalid proof level at {pointer}: {error}"),
                        }
                    })?;
                let copied_value = map.get("value").cloned().unwrap_or_else(|| value.clone());
                output.push(ProvenanceClaim {
                    claim: format!(
                        "capture_handoff:{}",
                        if pointer.is_empty() { "/" } else { pointer }
                    ),
                    value: copied_value,
                    evidence: format!(
                        "copied without promotion from admitted capture handoff {pointer}"
                    ),
                    proof_level,
                });
            }
            for (key, child) in map {
                let escaped = key.replace('~', "~0").replace('/', "~1");
                copy_proof_objects(child, &format!("{pointer}/{escaped}"), output)?;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                copy_proof_objects(child, &format!("{pointer}/{index}"), output)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn require_sha256_hex(field: &'static str, value: &str) -> Result<(), SdkError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(());
    }
    Err(SdkError::CaptureAdapterInvalid {
        reason: format!("{field} must be 64 lowercase hexadecimal characters"),
    })
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, SdkError> {
    let metadata = std::fs::metadata(path).map_err(|source| SdkError::io(path, source))?;
    if metadata.len() > limit {
        return Err(SdkError::CaptureInputTooLarge {
            path: path.display().to_string(),
            bytes: metadata.len(),
            max_bytes: limit,
        });
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| SdkError::CaptureInputTooLarge {
        path: path.display().to_string(),
        bytes: metadata.len(),
        max_bytes: limit,
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    File::open(path)
        .map_err(|source| SdkError::io(path, source))?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| SdkError::io(path, source))?;
    if bytes.len() as u64 > limit {
        return Err(SdkError::CaptureInputTooLarge {
            path: path.display().to_string(),
            bytes: bytes.len() as u64,
            max_bytes: limit,
        });
    }
    Ok(bytes)
}

fn hash_file(path: &Path) -> Result<String, SdkError> {
    let mut file = File::open(path).map_err(|source| SdkError::io(path, source))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|source| SdkError::io(path, source))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn paths_alias(left: &Path, right: &Path) -> Result<bool, SdkError> {
    if left == right {
        return Ok(true);
    }
    if right.exists() {
        let left = std::fs::canonicalize(left).map_err(|source| SdkError::io(left, source))?;
        let right = std::fs::canonicalize(right).map_err(|source| SdkError::io(right, source))?;
        return Ok(left == right);
    }
    Ok(false)
}
