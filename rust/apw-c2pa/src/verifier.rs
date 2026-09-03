use std::fs::File;
use std::path::Path;

use apw_core::VerificationState;
use c2pa::{settings::Settings, Context, Reader, ValidationState};
use serde_json::Value;

use crate::error::{file_name, io_error, C2paError, Result};

pub const HASH_MISMATCH_CODES: [&str; 3] = [
    "assertion.dataHash.mismatch",
    "assertion.bmffHash.mismatch",
    "assertion.boxesHash.mismatch",
];
pub const TRUSTED_CODE: &str = "signingCredential.trusted";

/// The result of reading one asset's C2PA claim.
///
/// IMPORTANT: `state` is [`VerificationState::NothingFound`] when no manifest is present.
/// That records the absence of provenance data. It is never evidence of synthetic origin.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimVerification {
    pub state: VerificationState,
    pub validation_state: Option<String>,
    pub failure_codes: Vec<String>,
    pub trust_evaluated: bool,
    pub detail: String,
    pub assertion_labels: Vec<String>,
    pub ingredient_count: usize,
    pub manifest: Option<Value>,
}

impl ClaimVerification {
    fn nothing_found() -> Self {
        Self {
            state: VerificationState::NothingFound,
            validation_state: None,
            failure_codes: Vec::new(),
            trust_evaluated: false,
            detail: "no C2PA manifest is embedded in the asset and no sidecar was supplied"
                .to_string(),
            assertion_labels: Vec::new(),
            ingredient_count: 0,
            manifest: None,
        }
    }

    pub fn has_assertion(&self, label: &str) -> bool {
        self.assertion_labels.iter().any(|found| found == label)
    }
}

/// IMPORTANT: `RegisteredButChanged` asserts that the claim WAS trusted and only the bytes
/// moved. An untrusted signer therefore outranks a hash mismatch: anyone can mint a claim over
/// altered audio, and reporting that as a recognised-but-changed registration would launder an
/// unknown key into an implied trust relationship. Trust is evaluated before the hard binding.
fn classify(
    state: ValidationState,
    failures: &[String],
    trust_evaluated: bool,
    credential_trusted: bool,
) -> (VerificationState, String) {
    let suffix = |extra: &[String]| {
        if extra.is_empty() {
            String::new()
        } else {
            format!(" ({})", extra.join(", "))
        }
    };

    if !trust_evaluated {
        return (
            VerificationState::MarkFoundClaimNotTrusted,
            "manifest present but no trust anchors were supplied, so the signer was never \
             evaluated"
                .to_string(),
        );
    }
    if !credential_trusted {
        return (
            VerificationState::MarkFoundClaimNotTrusted,
            format!(
                "manifest present but the signing credential did not chain to a supplied \
                 anchor{}",
                suffix(failures)
            ),
        );
    }

    let hash_failures: Vec<&str> = failures
        .iter()
        .filter(|code| HASH_MISMATCH_CODES.contains(&code.as_str()))
        .map(String::as_str)
        .collect();
    if !hash_failures.is_empty() {
        return (
            VerificationState::RegisteredButChanged,
            format!(
                "the signing credential is trusted but the hard binding is broken: {}",
                hash_failures.join(", ")
            ),
        );
    }
    if state == ValidationState::Trusted && failures.is_empty() {
        return (
            VerificationState::Verified,
            "signing credential trusted and hard binding intact".to_string(),
        );
    }
    (
        VerificationState::MarkFoundClaimNotTrusted,
        format!(
            "the signing credential is trusted but validation reported {state:?}{}",
            suffix(failures)
        ),
    )
}

fn context(trust_anchors_pem: Option<&str>) -> Result<Context> {
    let mut settings = Settings::new();
    if let Some(anchors) = trust_anchors_pem {
        if !anchors.contains("BEGIN CERTIFICATE") {
            return Err(C2paError::TrustAnchors);
        }
        settings = settings
            .with_value("trust.trust_anchors", anchors.to_string())
            .map_err(|e| C2paError::Context(Box::new(e)))?;
        settings = settings
            .with_value("verify.verify_trust", true)
            .map_err(|e| C2paError::Context(Box::new(e)))?;
    }
    Context::new()
        .with_settings(settings)
        .map_err(|e| C2paError::Context(Box::new(e)))
}

fn labels_of(active: &Value) -> Vec<String> {
    active
        .get("assertions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("label").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Read the provenance claim on `path`, optionally against a caller-supplied trust anchor
/// and an optional detached sidecar manifest.
pub fn verify_asset(
    path: &Path,
    mime: &str,
    trust_anchors_pem: Option<&str>,
    sidecar_manifest: Option<&[u8]>,
) -> Result<ClaimVerification> {
    let ctx = context(trust_anchors_pem)?;
    let stream = File::open(path).map_err(|e| io_error(path, e))?;
    let reader = match sidecar_manifest {
        Some(bytes) => {
            Reader::from_context(ctx).with_manifest_data_and_stream(bytes, mime, stream)
        }
        None => Reader::from_context(ctx).with_stream(mime, stream),
    };
    let reader = match reader {
        Ok(reader) => reader,
        Err(c2pa::Error::JumbfNotFound) => return Ok(ClaimVerification::nothing_found()),
        Err(source) => {
            return Err(C2paError::Verification {
                name: file_name(path),
                source: Box::new(source),
            })
        }
    };

    let validation_state = reader.validation_state();
    let mut failures: Vec<String> = Vec::new();
    let mut trusted_success = false;
    if let Some(active) = reader.validation_results().and_then(|r| r.active_manifest()) {
        for status in active.failure() {
            failures.push(status.code().to_string());
        }
        trusted_success = active
            .success()
            .iter()
            .any(|status| status.code() == TRUSTED_CODE);
    }
    let trust_evaluated = trust_anchors_pem.is_some() || trusted_success;

    let report: Value = reader
        .json_checked()
        .map_err(|source| C2paError::Verification {
            name: file_name(path),
            source: Box::new(source),
        })
        .and_then(|json| Ok(serde_json::from_str(&json)?))?;
    let active_label = report.get("active_manifest").and_then(Value::as_str);
    let active = active_label
        .and_then(|label| report.get("manifests").and_then(|m| m.get(label)))
        .cloned();
    if active.is_none() {
        return Ok(ClaimVerification::nothing_found());
    }

    let (state, detail) = classify(validation_state, &failures, trust_evaluated, trusted_success);
    let assertion_labels = active.as_ref().map(labels_of).unwrap_or_default();
    let ingredient_count = active
        .as_ref()
        .and_then(|m| m.get("ingredients"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

    Ok(ClaimVerification {
        state,
        validation_state: Some(format!("{validation_state:?}")),
        failure_codes: failures,
        trust_evaluated,
        detail,
        assertion_labels,
        ingredient_count,
        manifest: active,
    })
}
