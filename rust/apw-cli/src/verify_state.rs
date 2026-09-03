use std::path::Path;

use apw_c2pa::{detect_format, verify_asset};
use apw_core::{VerificationState, NOTHING_FOUND_NORMATIVE_NOTE};
use serde_json::{json, Value};

use crate::error::Result;

/// REQUIRED: this value stays `not_established` for every state, including
/// `verified`. A trusted signing credential proves the claim chains to an anchor
/// the caller supplied; it says nothing about who holds the key.
pub const SIGNER_IDENTITY: &str = "not_established";

pub struct VerifiedFile {
    pub state: VerificationState,
    pub record: Value,
}

/// Grade one file into one of the four provenance states.
///
/// Callable from the CLI and from the FFI surface a plug-in links, so the two
/// can never disagree about what a file carries.
pub fn state_of_file(
    path: &Path,
    trust_anchors_pem: Option<&str>,
    trust_anchor_scope: &str,
    sidecar_manifest: Option<&[u8]>,
) -> Result<VerifiedFile> {
    let asset = detect_format(path)?;
    // A container with no embedded-manifest handler carries no place for a claim
    // to live, so with no sidecar supplied there is nothing to look for. Asking
    // c2pa-rs anyway returns "type is unsupported", which is a statement about
    // the library, not about the file.
    if !asset.embeddable && sidecar_manifest.is_none() {
        return Ok(VerifiedFile {
            state: VerificationState::NothingFound,
            record: json!({
                "file": path.to_string_lossy(),
                "mime": asset.mime,
                "container": asset.container.as_str(),
                "state": VerificationState::NothingFound.as_str(),
                "library_validation_state": Value::Null,
                "failure_codes": Vec::<String>::new(),
                "assertion_labels": Vec::<String>::new(),
                "ingredient_count": 0,
                "trust_evaluated": false,
                "trust_anchor_scope": trust_anchor_scope,
                "detail": format!(
                    "{} cannot carry an embedded manifest and no detached manifest was \
                     supplied; pass the .c2pa sidecar with --sidecar",
                    asset.container.as_str()
                ),
                "signer_identity": SIGNER_IDENTITY,
                "normative_note": NOTHING_FOUND_NORMATIVE_NOTE,
                "manifest": Value::Null,
            }),
        });
    }
    let verification = verify_asset(path, asset.mime, trust_anchors_pem, sidecar_manifest)?;

    Ok(VerifiedFile {
        state: verification.state,
        record: json!({
            "file": path.to_string_lossy(),
            "mime": asset.mime,
            "container": asset.container.as_str(),
            "state": verification.state.as_str(),
            "library_validation_state": verification.validation_state,
            "failure_codes": verification.failure_codes,
            "assertion_labels": verification.assertion_labels,
            "ingredient_count": verification.ingredient_count,
            "trust_evaluated": verification.trust_evaluated,
            "trust_anchor_scope": trust_anchor_scope,
            "detail": verification.detail,
            "signer_identity": SIGNER_IDENTITY,
            "normative_note": NOTHING_FOUND_NORMATIVE_NOTE,
            "manifest": verification.manifest,
        }),
    })
}
