use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use apw_c2pa::{
    build_manifest, detect_format, sign_asset, verify_asset, C2paSigner, Ingredient,
    IngredientRelationship, ManifestSpec, SigningAlgorithm, SigningMode, SigningResult,
};
use apw_core::{sha256_file, ProofLevel, C2PA_CLAIM_SCOPE, PROOF_LEVEL_KEY};
use apw_daemon::{ClaimIssuer, ClaimRequest};
use apw_provenance::{ProvenanceProvider, SigningMaterial};
use serde_json::{json, Map, Value};

use crate::error::{one_line, CliError, Result};

/// Bounds carried over from `daemon/manifest_builder/generator.py`: project
/// sample references come from a file the daemon did not write, so the count and
/// the per-file hashing size are both capped rather than trusted.
const MAX_PROJECT_INGREDIENTS: usize = 32;
const MAX_INGREDIENT_HASH_BYTES: u64 = 256 * 1024 * 1024;

const STEM_DIGEST_MEANING: &str =
    "rolling hash-chain root over the observed routed windows, not a file digest";
const OBSERVED_SAMPLE_DIGEST_MEANING: &str =
    "sha-256 of the sample file as the daemon observed it";
const UNOBSERVED_SAMPLE_DIGEST_MEANING: &str =
    "sha-256 read from disk at manifest time; the daemon never observed this file \
     being imported, so it has no provenance of its own";

const TRUST_ANCHOR_SCOPE: &str = "self_issued_local_root_only";
/// REQUIRED: a chain issued by this machine's own root proves key possession. It
/// is never a verified creator, owner, or rights holder, and this field says so
/// inside the signed record rather than leaving a reader to infer it.
const SIGNER_IDENTITY: &str = "not_established";

/// Signs a detected export into a real C2PA claim and reads the claim back.
///
/// IMPORTANT: the export is never rewritten. `export.sha256` is committed before
/// this runs, so an in-place rewrite would leave the manifest describing a file
/// that no longer exists; the signed copy goes to the request's
/// `signed_asset_path`.
pub struct C2paClaimIssuer {
    provider: Arc<dyn ProvenanceProvider>,
}

impl C2paClaimIssuer {
    pub fn new(provider: Arc<dyn ProvenanceProvider>) -> Self {
        C2paClaimIssuer { provider }
    }
}

impl ClaimIssuer for C2paClaimIssuer {
    fn issue(&self, request: &ClaimRequest) -> Value {
        match self.try_issue(request) {
            Ok(claim) => claim,
            Err(error) => {
                log::warn!(
                    "Could not produce a C2PA claim for {}: {error}",
                    file_name(request.export_path)
                );
                apw_core::unavailable_c2pa_claim(&one_line(&error))
            }
        }
    }
}

struct IngredientNodes {
    ingredients: Vec<Ingredient>,
    nodes: Vec<Value>,
    unresolved: Vec<Value>,
}

impl C2paClaimIssuer {
    fn try_issue(&self, request: &ClaimRequest) -> Result<Value> {
        let asset = detect_format(request.export_path)?;
        let collected = collect_ingredients(request)?;

        let material = self.provider.issue_signing_material()?;
        let signer = C2paSigner::from_pem(
            &material.certificate_chain_pem,
            &material.private_key_handle,
            SigningAlgorithm::parse(&material.algorithm)?,
        )?;

        let mut spec = ManifestSpec::new(file_name(request.export_path));
        spec.ingredients = collected.ingredients;
        spec.mime = asset.mime.to_string();
        let c2pa_manifest = build_manifest(&spec)?;

        let signing = sign_asset(
            request.export_path,
            &request.signed_asset_path,
            &c2pa_manifest,
            &signer,
            Some(&request.sidecar_path),
        )?;
        let verification = verify_signed(&signing, &material)?;
        let identity = serde_json::to_value(self.provider.identity()?)?;
        let signed_hash = sha256_file(&signing.asset_path)?;
        let trust_anchor_pem = String::from_utf8(material.trust_anchor_pem.clone())
            .map_err(|_| CliError::usage("the trust anchor PEM is not valid UTF-8"))?;

        let mut signer_record = Map::new();
        if let Value::Object(fields) = identity {
            signer_record.extend(fields);
        }
        signer_record.insert("signer_identity".to_owned(), json!(SIGNER_IDENTITY));
        signer_record.insert("trust_anchor_pem".to_owned(), json!(trust_anchor_pem));

        Ok(json!({
            "status": signing.mode.as_str(),
            "mime": signing.mime,
            "claim_generator": c2pa_manifest.get("claim_generator_info").cloned(),
            "signed_asset": {
                "file_name": file_name(&signing.asset_path),
                "file_path": resolved_string(&signing.asset_path),
                "relative_path": relative_artifact(&signing.asset_path, &request.manifest_dir),
                "sha256": signed_hash,
                PROOF_LEVEL_KEY: ProofLevel::DirectlyObserved.as_str(),
            },
            "sidecar_manifest": match (signing.mode, &signing.manifest_path) {
                (SigningMode::Sidecar, Some(path)) => {
                    json!(relative_artifact(path, &request.manifest_dir))
                }
                _ => Value::Null,
            },
            "source_export_sha256": request.export_hash,
            "source_sha256_matches_export": signing.binding.source_sha256 == request.export_hash,
            "hard_binding": signing.binding.to_value()?,
            "validation": {
                "state": verification.state.as_str(),
                "library_validation_state": verification.validation_state,
                "failure_codes": verification.failure_codes,
                "assertion_labels": verification.assertion_labels,
                "trust_evaluated": verification.trust_evaluated,
                "trust_anchor_scope": TRUST_ANCHOR_SCOPE,
                "detail": verification.detail,
            },
            "signer": Value::Object(signer_record),
            "ingredients": collected.nodes,
            "unresolved_ingredient_references": collected.unresolved,
            "scope": C2PA_CLAIM_SCOPE,
            PROOF_LEVEL_KEY: ProofLevel::DirectlyObserved.as_str(),
        }))
    }
}

fn verify_signed(
    signing: &SigningResult,
    material: &SigningMaterial,
) -> Result<apw_c2pa::ClaimVerification> {
    let anchors = core::str::from_utf8(&material.trust_anchor_pem)
        .map_err(|_| CliError::usage("the trust anchor PEM is not valid UTF-8"))?;
    let sidecar = match (signing.mode, &signing.manifest_path) {
        (SigningMode::Sidecar, Some(path)) => Some(
            std::fs::read(path).map_err(|source| CliError::io("read", path, source))?,
        ),
        _ => None,
    };
    Ok(verify_asset(
        &signing.asset_path,
        &signing.mime,
        Some(anchors),
        sidecar.as_deref(),
    )?)
}

fn collect_ingredients(request: &ClaimRequest) -> Result<IngredientNodes> {
    let mut collected = IngredientNodes {
        ingredients: Vec::new(),
        nodes: Vec::new(),
        unresolved: Vec::new(),
    };
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut known_digests: HashSet<String> = request
        .ingredients
        .iter()
        .filter(|item| !item.sha256.is_empty())
        .map(|item| item.sha256.clone())
        .collect();

    for stem in request.stems {
        add_node(
            &mut collected,
            &mut seen_ids,
            &stem.stem_id,
            &stem.hash_chain_root,
            IngredientRelationship::ComponentOf,
            stem.proof_level,
            STEM_DIGEST_MEANING,
        )?;
    }
    for ingredient in request.ingredients {
        add_node(
            &mut collected,
            &mut seen_ids,
            &ingredient.file_name,
            &ingredient.sha256,
            IngredientRelationship::InputTo,
            ingredient.proof_level,
            OBSERVED_SAMPLE_DIGEST_MEANING,
        )?;
    }

    let mut project_nodes: Vec<(String, String)> = Vec::new();
    for reference in request.project_sample_refs {
        if project_nodes.len() >= MAX_PROJECT_INGREDIENTS {
            collected.unresolved.push(unresolvable(
                reference,
                &format!(
                    "more than {MAX_PROJECT_INGREDIENTS} project sample references; the \
                     remainder were not hashed"
                ),
            ));
            break;
        }
        let path = Path::new(reference);
        if !path.is_absolute() {
            collected.unresolved.push(unresolvable(
                reference,
                "project-relative reference was not resolved to a file",
            ));
            continue;
        }
        // Order matters for parity: Python tests `is_file()` first, so a missing
        // reference reports "not present", not an io error string.
        if !path.is_file() {
            collected.unresolved.push(unresolvable(
                reference,
                "referenced file is not present on this machine",
            ));
            continue;
        }
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(source) => {
                collected.unresolved.push(unresolvable(
                    reference,
                    &format!("referenced file could not be read: {source}"),
                ));
                continue;
            }
        };
        if metadata.len() > MAX_INGREDIENT_HASH_BYTES {
            collected.unresolved.push(unresolvable(
                reference,
                "referenced file exceeds the ingredient hashing limit",
            ));
            continue;
        }
        let digest = match sha256_file(path) {
            Ok(digest) => digest,
            Err(source) => {
                collected.unresolved.push(unresolvable(
                    reference,
                    &format!("referenced file could not be read: {source}"),
                ));
                continue;
            }
        };
        if !known_digests.insert(digest.clone()) {
            continue;
        }
        project_nodes.push((file_name(path), digest));
    }

    for (title, digest) in project_nodes {
        add_node(
            &mut collected,
            &mut seen_ids,
            &title,
            &digest,
            IngredientRelationship::InputTo,
            ProofLevel::UnknownUnobserved,
            UNOBSERVED_SAMPLE_DIGEST_MEANING,
        )?;
    }

    Ok(collected)
}

fn add_node(
    collected: &mut IngredientNodes,
    seen_ids: &mut HashSet<String>,
    title: &str,
    digest: &str,
    relationship: IngredientRelationship,
    proof_level: ProofLevel,
    digest_meaning: &str,
) -> Result<()> {
    if digest.is_empty() {
        collected.unresolved.push(unresolvable(
            title,
            "no digest was available, so the node cannot be recorded as an ingredient",
        ));
        return Ok(());
    }
    let ingredient_id = ingredient_id(title, digest);
    if !seen_ids.insert(ingredient_id.clone()) {
        return Ok(());
    }
    collected.ingredients.push(
        Ingredient::declared(title, relationship, proof_level, digest)?
            .with_id(ingredient_id.clone())?,
    );
    collected.nodes.push(json!({
        "title": title,
        "ingredient_id": ingredient_id,
        "relationship": relationship.as_str(),
        "sha256": digest,
        "digest_meaning": digest_meaning,
        PROOF_LEVEL_KEY: proof_level.as_str(),
    }));
    Ok(())
}

/// Stable, collision-resistant node id.
///
/// IMPORTANT: titles arrive from wire and project data. `build_manifest` rejects a
/// duplicate id and a title with no usable characters, and a rejection inside the
/// export watcher would cost the whole manifest, so the digest suffix keeps two
/// different files with the same name apart.
fn ingredient_id(title: &str, digest: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(|ch| ch == '-' || ch == '.');
    let slug = if trimmed.is_empty() {
        "ingredient"
    } else {
        trimmed
    };
    let head: String = slug.chars().take(48).collect();
    let tail: String = digest.chars().take(8).collect();
    format!("{head}-{tail}")
}

fn unresolvable(reference: &str, reason: &str) -> Value {
    json!({
        "reference": reference,
        "reason": reason,
        PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
    })
}

fn relative_artifact(path: &Path, manifest_dir: &Path) -> String {
    match path.strip_prefix(manifest_dir) {
        Ok(relative) => relative.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

fn resolved_string(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| PathBuf::from(path))
        .to_string_lossy()
        .into_owned()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
