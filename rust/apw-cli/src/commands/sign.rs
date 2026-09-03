use std::path::{Path, PathBuf};

use apw_c2pa::{
    build_manifest, detect_format, sign_asset, verify_asset, C2paSigner, ManifestSpec,
    SigningAlgorithm, SigningMode,
};
use apw_core::{sha256_file, C2PA_CLAIM_SCOPE};
use apw_provenance::{detect_provider, expand_user};
use serde_json::{json, Value};

use crate::cli::SignArgs;
use crate::error::{CliError, Result};

pub fn run(args: &SignArgs) -> Result<i32> {
    let asset = detect_format(&args.asset)?;
    let store = expand_user(&args.provenance_store);
    let provider = detect_provider(&store, args.provenance_provider.as_deref())?;
    let material = provider.issue_signing_material()?;
    let signer = C2paSigner::from_pem(
        &material.certificate_chain_pem,
        &material.private_key_handle,
        SigningAlgorithm::parse(&material.algorithm)?,
    )?;

    let mut spec = ManifestSpec::new(
        args.title
            .clone()
            .unwrap_or_else(|| file_name(&args.asset)),
    );
    spec.mime = asset.mime.to_string();
    let manifest = build_manifest(&spec)?;

    // IMPORTANT: the source is never rewritten. A caller that aimed the signed
    // copy back at the input would destroy the bytes the binding commits to.
    let destination = args.out.clone().unwrap_or_else(|| signed_sibling(&args.asset));
    if same_file(&args.asset, &destination) {
        return Err(CliError::usage(format!(
            "{} would overwrite the source asset; the signed copy must be a different file",
            destination.display()
        )));
    }
    let sidecar = args
        .sidecar
        .clone()
        .unwrap_or_else(|| destination.with_extension("c2pa"));

    let signing = sign_asset(&args.asset, &destination, &manifest, &signer, Some(&sidecar))?;
    let detached = match (signing.mode, &signing.manifest_path) {
        (SigningMode::Sidecar, Some(path)) => {
            Some(std::fs::read(path).map_err(|source| CliError::io("read", path, source))?)
        }
        _ => None,
    };
    let anchors = core::str::from_utf8(&material.trust_anchor_pem)
        .map_err(|_| CliError::usage("the local trust anchor PEM is not valid UTF-8"))?;
    let verification = verify_asset(
        &signing.asset_path,
        &signing.mime,
        Some(anchors),
        detached.as_deref(),
    )?;

    let record = json!({
        "status": signing.mode.as_str(),
        "mime": signing.mime,
        "source": {
            "file_path": args.asset.to_string_lossy(),
            "sha256": sha256_file(&args.asset)?,
        },
        // IMPORTANT: the sidecar path writes no new asset. Reporting the
        // untouched source here would claim a file this command did not produce.
        "signed_asset": match signing.mode {
            SigningMode::Embedded => json!({
                "file_path": signing.asset_path.to_string_lossy(),
                "sha256": sha256_file(&signing.asset_path)?,
            }),
            SigningMode::Sidecar => Value::Null,
        },
        // The embedded path reports the signed asset itself as the manifest
        // location; only a detached manifest is a sidecar.
        "sidecar_manifest": match (signing.mode, &signing.manifest_path) {
            (SigningMode::Sidecar, Some(path)) => json!(path.to_string_lossy()),
            _ => Value::Null,
        },
        "hard_binding": signing.binding.to_value()?,
        "validation": {
            "state": verification.state.as_str(),
            "library_validation_state": verification.validation_state,
            "failure_codes": verification.failure_codes,
            "trust_evaluated": verification.trust_evaluated,
            "trust_anchor_scope": "self_issued_local_root_only",
            "detail": verification.detail,
        },
        "signer_identity": "not_established",
        "scope": C2PA_CLAIM_SCOPE,
    });

    if args.json {
        println!("{}", serde_json::to_string_pretty(&record)?);
    } else {
        println!("MODE:    {}", signing.mode.as_str());
        match (signing.mode, &signing.manifest_path) {
            (SigningMode::Sidecar, Some(path)) => {
                println!("ASSET:   {} (unchanged)", args.asset.display());
                println!("SIDECAR: {}", path.display());
            }
            _ => println!("SIGNED:  {}", signing.asset_path.display()),
        }
        println!("BINDING: {}", signing.binding.covers);
        println!("STATE:   {}", verification.state.as_str());
        println!("SIGNER:  not_established (a signing key is not a verified identity)");
    }
    Ok(0)
}

fn signed_sibling(asset: &Path) -> PathBuf {
    let stem = asset
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "asset".to_owned());
    let extension = asset
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default();
    let name = format!("{stem}_c2pa{extension}");
    match asset.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    canonical(left) == canonical(right)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
