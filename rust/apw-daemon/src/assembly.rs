use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use apw_core::{
    path_name, pretty_json_bytes, sha256_file, sha256_hex, sha256_prefix, utc_timestamp_seconds,
    without_top_level_keys, Ed25519Signer, ExportEvidence, IngredientEvidence, Manifest,
    ManifestBuilder, ProofLevel, StemEvidence, PORTABLE_SIGNATURE_EXCLUDED_KEYS, PROOF_LEVEL_KEY,
};
use serde_json::{json, Map, Value};

use crate::error::{DaemonError, Result};
use crate::probe::AudioProbe;
use crate::services::{ClaimIssuer, ClaimRequest, ExportAssociator, ForgeryAnalyzer, LocalSealer, TimeAnchor};
use crate::session::SessionSnapshot;

/// Every layer the manifest can account for. A layer absent from the session is
/// declared unobserved by name rather than left out silently.
pub const ALL_LAYERS: [&str; 8] = [
    "audio_buffer",
    "transport",
    "midi",
    "session",
    "sample_watcher",
    "project_differ",
    "input_capture",
    "screen_observer",
];

const ASSOCIATION_BASIS: &str =
    "A bounded sequence of relative RMS, zero-crossing, crest-factor, and coarse energy-envelope \
     features emitted from accepted routed plug-in windows was compared with equivalent streaming-\
     extracted export features using time-offset search. The relationship remains inferred and does \
     not establish complete routing.";

const HANDOFF_BOUNDARY: &str =
    "This neutral handoff is not a provider-specific API payload and claims no compatibility \
     with proprietary watermark, recovery, identity, signing, or registry technology.";

const CLAIM_HANDOFF_BOUNDARY: &str =
    "The claim is signed with a certificate chain this machine issued to itself. \
     A downstream registrar must re-issue under its own credential policy before \
     the claim carries any identity meaning.";

const MISSING_DOWNSTREAM_REQUIREMENTS: [&str; 8] = [
    "verified creator or institution identity",
    "author-controlled credential and key policy",
    "production certificate chain issued by a recognised authority",
    "publication on a recognised C2PA trust list",
    "audio-native soft binding or watermark",
    "resilient recovery from the audio",
    "registry publication",
    "consent and rights verification",
];

pub struct AssemblyContext<'a> {
    pub session_id: &'a str,
    pub stem_id: &'a str,
    pub source_category: &'a str,
    pub source_category_proof_level: ProofLevel,
    pub evidence_dir: &'a Path,
    pub manifest_dir: &'a Path,
    pub session_started_at: &'a str,
    pub generate_html_report: bool,
    /// The previous manifest's cosignature hash, `"genesis"` for the first.
    pub previous_cosignature_hash: &'a str,
}

pub struct ManifestServices<'a> {
    pub audio: &'a dyn AudioProbe,
    pub associator: &'a dyn ExportAssociator,
    pub forgery: &'a dyn ForgeryAnalyzer,
    pub claims: &'a dyn ClaimIssuer,
    pub sealer: &'a dyn LocalSealer,
    pub portable_signer: Option<&'a Ed25519Signer>,
    pub time_anchor: Option<&'a dyn TimeAnchor>,
    pub ots_anchor: Option<&'a dyn TimeAnchor>,
}

pub struct AssemblyInputs<'a> {
    pub snapshot: &'a SessionSnapshot,
    pub coverage: Value,
    pub session_diagnostics: Value,
    pub receipt_summary: Value,
    /// The saved-project projection, when a project watcher is running.
    pub session_facts: Option<Value>,
    /// Sample paths referenced by the saved project, for the claim issuer.
    pub project_sample_refs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GeneratedManifest {
    pub manifest: Manifest,
    pub manifest_path: PathBuf,
    pub report_path: PathBuf,
    pub verification_path: PathBuf,
    pub handoff_path: PathBuf,
    pub bundle_index_path: PathBuf,
    pub bundle_path: PathBuf,
    pub export_hash: String,
    /// The cosignature hash the next manifest in this session entangles.
    pub cosignature_hash: Option<String>,
}

/// Assembles, signs and writes the manifest for one detected export.
///
/// IMPORTANT: the order here is the security property, not a style. Everything a
/// signature must cover is attached before that signature is computed: the
/// hardware chain-root binding precedes both signatures, and the portable
/// Ed25519 signature precedes the local seal, whose signing input includes it.
/// Reordering these steps silently narrows what the signatures commit to.
pub fn generate_manifest(
    context: &AssemblyContext,
    services: &ManifestServices,
    inputs: &AssemblyInputs,
    export_path: &Path,
    export_version: u32,
) -> Result<GeneratedManifest> {
    let export_hash = sha256_file(export_path)?;
    let metadata =
        fs::metadata(export_path).map_err(|source| DaemonError::io("stat", export_path, source))?;
    let export_metadata = services.audio.metadata(export_path);

    let mut builder = ManifestBuilder::new(context.session_id, apw_core::utc_timestamp(None));
    builder.set_export(ExportEvidence {
        file_path: export_path.to_string_lossy().into_owned(),
        file_name: path_name(export_path),
        sha256: export_hash.clone(),
        format: extension_of(export_path),
        file_size_bytes: metadata.len(),
        duration_seconds: export_metadata.duration_seconds,
        sample_rate_hz: export_metadata.sample_rate.clone(),
        channel_count: export_metadata.channels,
        exported_at: utc_timestamp_seconds(None),
        export_version,
    });

    let snapshot = inputs.snapshot;
    let first_hash = &snapshot.first_hash_event;
    let last_hash = &snapshot.last_hash_event;
    let first_hash_ms = integer_field(first_hash, "source_timestamp_ms");
    let last_hash_ms = integer_field(last_hash, "source_timestamp_ms");
    let first_received_at = optional_string(first_hash, "received_at");
    let last_received_at = optional_string(last_hash, "received_at");
    let last_window_hash = string_field(last_hash, "window_hash");
    let chain_genesis = match optional_string(first_hash, "prev_hash") {
        Some(genesis) => genesis,
        None => "genesis".to_owned(),
    };
    let stem_sample_rate = integer_field(last_hash, "sample_rate_hz");
    let stem_channels = integer_field(last_hash, "channel_count");

    let mut stems: Vec<StemEvidence> = Vec::new();
    let mut ingredients: Vec<IngredientEvidence> = Vec::new();
    for event in &snapshot.events {
        let Some(map) = event.as_object() else {
            continue;
        };
        match map.get("event_type").and_then(Value::as_str) {
            Some("sample_file_observed") => {
                let ingredient = IngredientEvidence {
                    file_name: string_field(map, "file_name"),
                    sha256: string_field(map, "sha256"),
                    proof_level: map
                        .get("proof_level")
                        .and_then(Value::as_str)
                        .and_then(|level| ProofLevel::parse(level).ok())
                        .unwrap_or(ProofLevel::UnknownUnobserved),
                    correlation_confidence: None,
                    audio_fingerprint: map.get("audio_fingerprint").cloned(),
                };
                builder.add_ingredient(ingredient.clone());
                ingredients.push(ingredient);
            }
            Some("composite_edit") => {
                builder.add_composite_edit(event.clone());
            }
            _ => {}
        }
    }

    if snapshot.chain_length > 0 {
        let stem = StemEvidence {
            stem_id: context.stem_id.to_owned(),
            hash_chain_root: last_window_hash.clone(),
            hash_chain_genesis: chain_genesis,
            hash_chain_length: snapshot.chain_length,
            first_observed_ms: first_hash_ms,
            last_observed_ms: last_hash_ms,
            first_received_at,
            last_received_at,
            sample_rate_hz: u32::try_from(stem_sample_rate.max(0)).unwrap_or(u32::MAX),
            channel_count: u16::try_from(stem_channels.max(0)).unwrap_or(u16::MAX),
            source_category: context.source_category.to_owned(),
            source_category_proof_level: context.source_category_proof_level,
            plugin_instance_ids: snapshot.plugin_instance_ids.clone(),
            proof_level: ProofLevel::DirectlyObserved,
        };
        builder.add_stem(stem.clone());
        stems.push(stem);
    }

    builder.set_coverage(inputs.coverage.clone());

    let mut association = services
        .associator
        .associate(export_path, &snapshot.feature_events);
    if let Some(record) = association.as_object_mut() {
        record.insert("capture_session_id".to_owned(), json!(context.session_id));
        record.insert(
            "stem_ids".to_owned(),
            if snapshot.chain_length > 0 {
                json!([context.stem_id])
            } else {
                json!([])
            },
        );
        record.insert("export_file_name".to_owned(), json!(path_name(export_path)));
        record.insert("basis".to_owned(), json!(ASSOCIATION_BASIS));
    }
    builder.set_audio_association(association.clone());
    builder.set_session_diagnostics(inputs.session_diagnostics.clone());
    builder.set_host_environment(derive_host_environment(snapshot));

    let active: BTreeSet<&str> = snapshot
        .active_layers
        .iter()
        .map(String::as_str)
        .collect();
    for layer in ALL_LAYERS.iter().filter(|layer| !active.contains(*layer)) {
        builder
            .unobserved_mut()
            .push(format!("layer_{layer}_not_active"));
    }

    builder.set_forgery_report(services.forgery.analyze(&snapshot.events));

    let stem = file_stem_of(export_path);
    let suffix = if export_version == 1 {
        String::new()
    } else {
        format!("_v{export_version:03}")
    };
    let manifest_path = context
        .manifest_dir
        .join(format!("{stem}{suffix}_manifest.json"));
    let report_path = context
        .manifest_dir
        .join(format!("{stem}{suffix}_provenance.html"));
    let artifact_dir = context.manifest_dir.join("artifacts");
    fs::create_dir_all(&artifact_dir)
        .map_err(|source| DaemonError::io("create", &artifact_dir, source))?;
    let verification_path = artifact_dir.join(format!("{stem}{suffix}_verification.json"));
    let handoff_path = artifact_dir.join(format!("{stem}{suffix}_handoff.json"));
    let bundle_index_path = artifact_dir.join(format!("{stem}{suffix}_bundle_index.json"));
    let bundle_path = artifact_dir.join(format!("{stem}{suffix}_evidence_bundle.zip"));
    // IMPORTANT: the signed copy goes under manifest_dir/artifacts, never over the
    // export. export.sha256 is already committed, and the export watcher scans
    // export_dir non-recursively, so a signed sibling here is not re-detected.
    let signed_asset_path = artifact_dir.join(format!(
        "{stem}{suffix}_c2pa{}",
        dotted_extension(export_path)
    ));
    let sidecar_path = artifact_dir.join(format!(
        "{stem}{suffix}{}.c2pa",
        dotted_extension(export_path)
    ));

    let claim = services.claims.issue(&ClaimRequest {
        export_path,
        export_hash: &export_hash,
        stems: &stems,
        ingredients: &ingredients,
        project_sample_refs: &inputs.project_sample_refs,
        signed_asset_path,
        sidecar_path,
        manifest_dir: context.manifest_dir.to_path_buf(),
    });
    builder.set_c2pa_claim(claim.clone());

    let (evidence_hashes, evidence_files) = hash_evidence_directory(context.evidence_dir)?;

    let mut manifest = builder
        .build()
        .map_err(|source| DaemonError::assembly("cannot assemble the manifest", source))?;

    if let Some(session_facts) = &inputs.session_facts {
        manifest.insert("session_facts", session_facts.clone());
    }
    manifest.insert(
        "evidence_binding",
        json!({
            "evidence_directory": resolved_string(context.evidence_dir),
            "evidence_file_hashes": evidence_hashes,
            "evidence_files": evidence_files,
            "last_window_hash": last_window_hash,
            "chain_length": snapshot.chain_length,
            PROOF_LEVEL_KEY: ProofLevel::DirectlyObserved.as_str(),
        }),
    );
    if let Some(Value::Object(capture_session)) = manifest.get_mut("capture_session") {
        capture_session.insert("started_at".to_owned(), json!(context.session_started_at));
        capture_session.insert("state_at_manifest".to_owned(), json!("active"));
    }
    manifest.insert(
        "daemon_receipt_acknowledgement",
        inputs.receipt_summary.clone(),
    );
    let receipt_status = inputs
        .receipt_summary
        .get("status")
        .map(apw_core::python_str)
        .unwrap_or_default();
    let receipt_sent = inputs
        .receipt_summary
        .get("counters")
        .and_then(|counters| counters.get("sent"))
        .map(apw_core::python_str)
        .unwrap_or_else(|| "0".to_owned());
    let receipt_proof_level = inputs
        .receipt_summary
        .get(PROOF_LEVEL_KEY)
        .cloned()
        .unwrap_or_else(|| json!(ProofLevel::UnknownUnobserved.as_str()));
    if let Some(Value::Array(claim_summary)) = manifest.get_mut("claim_summary") {
        let entry = json!({
            "claim": "daemon_receipt_acknowledgement",
            "value": receipt_status,
            "evidence": format!(
                "Daemon dispatched {receipt_sent} local ACK packets; \
                 plug-in processing of each packet is outside daemon observability."
            ),
            PROOF_LEVEL_KEY: receipt_proof_level,
        });
        let position = claim_summary.len().min(2);
        claim_summary.insert(position, entry);
    }

    let claim_record = manifest.get("c2pa_claim").cloned().unwrap_or(Value::Null);
    manifest.insert(
        "presentation",
        json!({
            "html_report": if context.generate_html_report {
                json!(path_name(&report_path))
            } else {
                Value::Null
            },
            "derived_from": path_name(&manifest_path),
            "verifier_result": relative_artifact(&verification_path, context.manifest_dir),
            "downstream_handoff": relative_artifact(&handoff_path, context.manifest_dir),
            "bundle_index": if context.generate_html_report {
                json!(relative_artifact(&bundle_index_path, context.manifest_dir))
            } else {
                Value::Null
            },
            "evidence_bundle": if context.generate_html_report {
                json!(relative_artifact(&bundle_path, context.manifest_dir))
            } else {
                Value::Null
            },
            "c2pa_signed_asset": claim_record
                .get("signed_asset")
                .and_then(|asset| asset.get("relative_path"))
                .cloned()
                .unwrap_or(Value::Null),
            "c2pa_sidecar_manifest": claim_record
                .get("sidecar_manifest")
                .cloned()
                .unwrap_or(Value::Null),
            PROOF_LEVEL_KEY: ProofLevel::DirectlyObserved.as_str(),
        }),
    );

    let handoff = build_handoff(HandoffInputs {
        context,
        export_path,
        export_hash: &export_hash,
        association: &association,
        coverage: &inputs.coverage,
        evidence_files: manifest
            .get("evidence_binding")
            .and_then(|binding| binding.get("evidence_files"))
            .cloned()
            .unwrap_or_else(|| json!({})),
        manifest_name: &path_name(&manifest_path),
        bundle_name: if context.generate_html_report {
            Some(path_name(&bundle_path))
        } else {
            None
        },
        bundle_index_name: if context.generate_html_report {
            Some(path_name(&bundle_index_path))
        } else {
            None
        },
        c2pa_claim: &claim_record,
        chain_root: &last_window_hash,
        chain_length: snapshot.chain_length,
        portable_signer: services.portable_signer,
    });
    manifest.insert("downstream_registration_handoff", handoff.clone());

    if let Some(anchor) = services.time_anchor {
        manifest.insert("time_anchor", anchor.anchor_record(&export_hash));
    }
    if let Some(anchor) = services.ots_anchor {
        manifest.insert("time_anchor_opentimestamps", anchor.anchor_record(&export_hash));
    }

    if snapshot.chain_length > 0 && !last_window_hash.is_empty() {
        // Added before signing so both manifest signatures cover the binding.
        if let Some(binding) = services.sealer.bind_chain_root(&last_window_hash) {
            manifest.insert("hardware_binding", binding);
        }
    }

    if let Some(signer) = services.portable_signer {
        let unsigned = without_top_level_keys(manifest.as_value(), &PORTABLE_SIGNATURE_EXCLUDED_KEYS);
        match signer.sign_manifest(&unsigned) {
            Ok(signature) => match serde_json::to_value(&signature) {
                Ok(record) => {
                    manifest.insert("portable_signature", record);
                }
                Err(error) => log::error!("Could not encode the portable signature: {error}"),
            },
            Err(error) => log::error!("Could not create portable Ed25519 signature: {error}"),
        }
    }

    let mut cosignature_hash = None;
    match manifest.local_signing_input() {
        Ok(signing_input) => {
            let signed_content_hash = sha256_hex(&signing_input);
            if let Some(seal) = services.sealer.seal(
                &signing_input,
                &signed_content_hash,
                context.previous_cosignature_hash,
            ) {
                manifest.insert("manifest_signature", seal.record);
                cosignature_hash = Some(seal.entangled_hash);
            }
        }
        Err(error) => log::warn!("Could not sign manifest: {error}"),
    }

    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent).map_err(|source| DaemonError::io("create", parent, source))?;
    }
    manifest.write_pretty(&manifest_path)?;
    let handoff_bytes = pretty_json_bytes(&handoff)?;
    fs::write(&handoff_path, handoff_bytes)
        .map_err(|source| DaemonError::io("write", &handoff_path, source))?;

    if snapshot.chain_length == 0 {
        log::warn!("Export was hashed, but no routed-audio hash events were received");
    }

    Ok(GeneratedManifest {
        manifest,
        manifest_path,
        report_path,
        verification_path,
        handoff_path,
        bundle_index_path,
        bundle_path,
        export_hash,
        cosignature_hash,
    })
}

struct HandoffInputs<'a> {
    context: &'a AssemblyContext<'a>,
    export_path: &'a Path,
    export_hash: &'a str,
    association: &'a Value,
    coverage: &'a Value,
    evidence_files: Value,
    manifest_name: &'a str,
    bundle_name: Option<String>,
    bundle_index_name: Option<String>,
    c2pa_claim: &'a Value,
    chain_root: &'a str,
    chain_length: u64,
    portable_signer: Option<&'a Ed25519Signer>,
}

/// The neutral downstream registration record. It is a candidate input: nothing
/// here has been submitted to, or accepted by, any registry.
fn build_handoff(inputs: HandoffInputs) -> Value {
    let context = inputs.context;
    json!({
        "record_type": "downstream_provenance_registration_handoff",
        "status": "candidate_input_not_submitted",
        "capture_session_id": context.session_id,
        "export_hard_hash": {
            "algorithm": "sha256",
            "value": inputs.export_hash,
            PROOF_LEVEL_KEY: ProofLevel::DirectlyObserved.as_str(),
        },
        "routed_observation_commitment": {
            "hash_chain_root": if inputs.chain_root.is_empty() {
                Value::Null
            } else {
                json!(inputs.chain_root)
            },
            "hash_chain_length": inputs.chain_length,
            PROOF_LEVEL_KEY: if inputs.chain_length > 0 {
                ProofLevel::DirectlyObserved.as_str()
            } else {
                ProofLevel::UnknownUnobserved.as_str()
            },
        },
        "coverage": inputs.coverage,
        "audio_association": inputs.association,
        "creator_declarations": [{
            "name": "source_category",
            "value": context.source_category,
            PROOF_LEVEL_KEY: context.source_category_proof_level.as_str(),
        }],
        "signing_key": {
            "algorithm": "Ed25519",
            "public_key_hex": inputs
                .portable_signer
                .map(Ed25519Signer::public_key_hex)
                .unwrap_or_default(),
            "public_key_file": inputs
                .portable_signer
                .map(|signer| signer.public_key_path().to_string_lossy().into_owned())
                .unwrap_or_default(),
            "trust_scope": apw_core::TRUST_SCOPE_SELF_GENERATED,
            "signer_identity": "not_established",
            PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
        },
        "evidence_bundle": {
            "manifest": resolved_string(&context.manifest_dir.join(inputs.manifest_name)),
            "export": resolved_string(inputs.export_path),
            "evidence_directory": resolved_string(context.evidence_dir),
            "files": inputs.evidence_files,
            "downloadable_archive": match &inputs.bundle_name {
                Some(name) => json!(format!("artifacts/{name}")),
                None => Value::Null,
            },
            "signed_bundle_index": match &inputs.bundle_index_name {
                Some(name) => json!(format!("artifacts/{name}")),
                None => Value::Null,
            },
            "index_scope": "all archive payload entries; the signed index is not self-hashed",
            PROOF_LEVEL_KEY: ProofLevel::DirectlyObserved.as_str(),
        },
        "c2pa_claim": handoff_c2pa_claim(inputs.c2pa_claim),
        "descriptive_c2pa_assertion_mapping": {
            "status": "descriptive_projection_not_the_signed_claim",
            "assertions": ["c2pa.hash.data", "c2pa.ingredient", "c2pa.actions", "apw.unobserved"],
            PROOF_LEVEL_KEY: ProofLevel::Inferred.as_str(),
        },
        "missing_downstream_requirements": MISSING_DOWNSTREAM_REQUIREMENTS,
        "boundary": HANDOFF_BOUNDARY,
    })
}

/// Summarises the signed claim for a downstream registrar: the full claim lives
/// in the manifest, so the handoff carries only what a registrar needs to locate
/// and re-check it, plus the trust caveat.
fn handoff_c2pa_claim(claim: &Value) -> Value {
    let status = claim.get("status");
    let unavailable = !claim.is_object()
        || status.is_none()
        || status == Some(&Value::Null)
        || status.and_then(Value::as_str) == Some("unavailable");
    if unavailable {
        return json!({
            "status": "unavailable",
            "reason": claim
                .get("reason")
                .cloned()
                .unwrap_or_else(|| json!("No C2PA claim was produced for this export.")),
            PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
        });
    }
    let nested = |parent: &str, child: &str| -> Value {
        claim
            .get(parent)
            .filter(|value| value.is_object())
            .and_then(|value| value.get(child))
            .cloned()
            .unwrap_or(Value::Null)
    };
    json!({
        "status": status.cloned().unwrap_or(Value::Null),
        "signed_asset": nested("signed_asset", "relative_path"),
        "signed_asset_sha256": nested("signed_asset", "sha256"),
        "sidecar_manifest": claim.get("sidecar_manifest").cloned().unwrap_or(Value::Null),
        "hard_binding_algorithm": nested("hard_binding", "algorithm"),
        "validation_state": nested("validation", "state"),
        "trust_anchor_scope": nested("validation", "trust_anchor_scope"),
        "signer_key_id": nested("signer", "key_id"),
        "signer_identity": "not_established",
        "boundary": CLAIM_HANDOFF_BOUNDARY,
        PROOF_LEVEL_KEY: ProofLevel::DirectlyObserved.as_str(),
    })
}

/// Hashes each evidence file's prefix at its length now, so a line appended after
/// this point cannot invalidate the binding it is not covered by.
fn hash_evidence_directory(evidence_dir: &Path) -> Result<(Value, Value)> {
    let mut names: Vec<PathBuf> = match fs::read_dir(evidence_dir) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();

    let mut hashes = Map::new();
    let mut files = Map::new();
    for path in names {
        let metadata = fs::metadata(&path).map_err(|source| DaemonError::io("stat", &path, source))?;
        let byte_length = metadata.len();
        let digest = sha256_prefix(&path, byte_length)?;
        let name = path_name(&path);
        hashes.insert(name.clone(), json!(digest));
        files.insert(
            name,
            json!({
                "sha256": digest,
                "byte_length": byte_length,
                "binding_scope": "file_prefix_at_manifest_creation",
            }),
        );
    }
    Ok((Value::Object(hashes), Value::Object(files)))
}

fn integer_field(map: &Map<String, Value>, key: &str) -> i64 {
    map.get(key)
        .and_then(|value| match value {
            Value::Number(number) => number.as_i64().or_else(|| number.as_f64().map(|f| f as i64)),
            _ => None,
        })
        .unwrap_or(0)
}

fn string_field(map: &Map<String, Value>, key: &str) -> String {
    map.get(key)
        .filter(|value| apw_core::is_truthy(value))
        .map(apw_core::python_str)
        .unwrap_or_default()
}

fn optional_string(map: &Map<String, Value>, key: &str) -> Option<String> {
    let value = string_field(map, key);
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn file_stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn dotted_extension(path: &Path) -> String {
    match path.extension() {
        Some(ext) => format!(".{}", ext.to_string_lossy()),
        None => String::new(),
    }
}

fn resolved_string(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// `Path.relative_to` with Python's fallback: an artifact outside the manifest
/// directory keeps its full path rather than raising inside the export watcher.
fn relative_artifact(path: &Path, manifest_dir: &Path) -> String {
    path.strip_prefix(manifest_dir)
        .map(|relative| relative.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

const HOST_ENVIRONMENT_SCOPE: &str = "The host application that loaded the capture plug-in, as reported by the \
plug-in wrapper. Naming the host does not extend observation to anything \
the host did outside the capture path.";

/// `derive_host_environment`: recognition and identity are separate, so an
/// unrecognised host is never read back as a host named "Unknown".
pub fn derive_host_environment(snapshot: &SessionSnapshot) -> Value {
    derive_host_environment_for(snapshot, crate::host_identity::current_platform())
}

/// [`derive_host_environment`] for an explicit platform (`None` matches any).
pub fn derive_host_environment_for(snapshot: &SessionSnapshot, platform: Option<&str>) -> Value {
    let unknown = |status: &str, basis: String| {
        json!({
            "status": status,
            "host_recognised": false,
            "host_name": Value::Null,
            "host_executable_name": Value::Null,
            "wrapper_format": Value::Null,
            "basis": basis,
            "scope": HOST_ENVIRONMENT_SCOPE,
            PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
        })
    };
    let Some(observed) = &snapshot.host_environment else {
        return unknown("unobserved", "The plug-in reported no host environment in this session.".to_owned());
    };
    if snapshot.host_environment_conflicts > 0 {
        return unknown(
            "conflicting_observations",
            format!(
                "{} later host environment report(s) disagreed with the first, so no single \
                 host is established.",
                snapshot.host_environment_conflicts
            ),
        );
    }
    let field = |key: &str| observed.get(key).cloned().unwrap_or(Value::Null);
    let recognised = observed.get("host_recognised") == Some(&Value::Bool(true));
    if !recognised {
        // IMPORTANT: only here. A wrapper-recognised host is never overridden, and a
        // table hit is an inference from a process name, never an observation.
        let executable = observed.get("host_executable_name").and_then(Value::as_str);
        match crate::host_identity::identify_host(executable, false, None, platform) {
            Ok(identity) if identity.identification == crate::host_identity::IDENT_INFERRED => {
                return json!({
                    "status": "host_inferred",
                    "host_recognised": false,
                    "host_name": identity.host_name,
                    "host_executable_name": field("host_executable_name"),
                    "wrapper_format": field("wrapper_format"),
                    "identification": identity.identification,
                    "host_id": identity.host_id,
                    "source_url": identity.source_url,
                    "basis": "The plug-in wrapper did not recognise the host application. Its executable \
                              file name matched an entry in the host executable table, so the host is \
                              inferred from that name; any program can carry any file name.",
                    "scope": HOST_ENVIRONMENT_SCOPE,
                    PROOF_LEVEL_KEY: identity.proof_level,
                });
            }
            Ok(_) => {}
            Err(error) => {
                log::warn!("Host executable table unusable; not identifying the host: {error}");
            }
        }
    }
    json!({
        "status": if recognised { "observed" } else { "host_unrecognised" },
        "host_recognised": recognised,
        "host_name": if recognised { field("host_name") } else { Value::Null },
        "host_executable_name": field("host_executable_name"),
        "wrapper_format": field("wrapper_format"),
        "basis": if recognised {
            "The plug-in wrapper named the host application that loaded it."
        } else {
            "The plug-in wrapper did not recognise the host application, so the host is not \
             identified. Its executable name and the plug-in format remain as observed."
        },
        "scope": HOST_ENVIRONMENT_SCOPE,
        PROOF_LEVEL_KEY: if recognised {
            ProofLevel::DirectlyObserved.as_str()
        } else {
            ProofLevel::UnknownUnobserved.as_str()
        },
    })
}
