use serde_json::{Map, Value};

use crate::canonical::MAX_PROOF_VALUE_DEPTH;
use crate::proof::{ProofLevel, PROOF_LEVEL_KEY};
use crate::pyvalue::{get, is_string_equal, is_truthy, python_eq, python_str};

/// The keys `validate_manifest_invariants` requires. IMPORTANT: `c2pa_claim` is
/// deliberately absent. `daemon/schema.py::_validate_c2pa_claim` treats absence
/// as a non-error and `daemon/verify.py` raises the `c2pa_claim_missing`
/// warning instead, so requiring it here would reject manifests Python accepts.
pub const REQUIRED_MANIFEST_KEYS: [&str; 12] = [
    "apw_version",
    "schema",
    "session_id",
    "capture_session",
    "created_at",
    "observed_stems",
    "claim_summary",
    "stem_export_association",
    "observation_coverage",
    "daemon_receipt_acknowledgement",
    "apw:unobserved",
    "c2pa_mapping",
];

const COVERAGE_STATUSES: [&str; 3] = [
    "complete_observed_path",
    "partial_observed_path",
    "unknown_coverage",
];
const C2PA_CLAIM_STATUSES: [&str; 3] = ["embedded", "sidecar", "unavailable"];
const HOST_ENVIRONMENT_STATUSES: [&str; 4] = [
    "observed",
    "host_unrecognised",
    "conflicting_observations",
    "unobserved",
];
const ASSOCIATION_STATUSES: [&str; 3] = ["inferred_match", "not_established", "unavailable"];
const RECEIPT_STATUSES: [&str; 3] = ["issued", "degraded", "unknown"];
const REQUIRED_COVERAGE_COUNTERS: [&str; 13] = [
    "windows_hashed",
    "buffer_hash_events_received",
    "fifo_samples_dropped",
    "fifo_windows_dropped",
    "udp_sends_failed",
    "sequence_gaps",
    "hash_chain_breaks",
    "events_prepared",
    "events_received",
    "packets_received",
    "daemon_acknowledgements_sent",
    "daemon_acknowledgements_failed",
    "bypassed_buffers",
];
const ZERO_COVERAGE_COUNTERS: [&str; 10] = [
    "fifo_samples_dropped",
    "fifo_windows_dropped",
    "midi_events_dropped",
    "bypassed_buffers",
    "udp_sends_failed",
    "sequence_gaps",
    "hash_chain_breaks",
    "stream_evictions",
    "daemon_acknowledgements_failed",
    "plugin_telemetry_regressions",
];

/// Returns the enforced-invariant violations in the same order and wording the
/// Python validator produces; each one becomes a `schema_invalid` error finding.
/// Bounded recursion: nesting beyond [`MAX_PROOF_VALUE_DEPTH`] is reported as a
/// violation, never a stack overflow (untrusted verifier input).
pub fn validate_manifest_invariants(value: &Value) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    let Value::Object(data) = value else {
        return vec!["manifest must be a JSON object".to_owned()];
    };
    for key in REQUIRED_MANIFEST_KEYS {
        if !data.contains_key(key) {
            errors.push(format!("required field is missing: {key}"));
        }
    }

    validate_proof_values(value, "$", &mut errors, 0);

    match data.get("observed_stems") {
        Some(Value::Array(stems)) => validate_stems(stems, &mut errors),
        None => {}
        Some(_) => errors.push("observed_stems must be a list".to_owned()),
    }

    if let Some(Value::Object(export)) = data.get("export") {
        require_proof(export, "export", &mut errors);
        let has_portable = !matches!(get(data, "portable_signature"), Value::Null);
        if has_portable {
            let digest = match export.get("sha256") {
                Some(value) => python_str(value),
                None => String::new(),
            };
            if digest.chars().count() != 64 {
                errors
                    .push("export.sha256 must be a 64-character SHA-256 hex digest".to_owned());
            }
        }
    }

    if let Some(Value::Array(claims)) = data.get("claim_summary") {
        for (index, claim) in claims.iter().enumerate() {
            if let Value::Object(claim) = claim {
                require_proof(claim, &format!("claim_summary[{index}]"), &mut errors);
            }
        }
    }

    if let Some(Value::Object(association)) = data.get("stem_export_association") {
        validate_association(association, &mut errors);
    }

    if let Some(Value::Object(coverage)) = data.get("observation_coverage") {
        validate_coverage(coverage, &mut errors);
    }

    if let Some(Value::Object(host)) = data.get("host_environment") {
        validate_host_environment(host, &mut errors);
    }

    if let Some(Value::Object(receipt)) = data.get("daemon_receipt_acknowledgement") {
        require_proof(receipt, "daemon_receipt_acknowledgement", &mut errors);
        if !status_in(receipt, &RECEIPT_STATUSES) {
            errors.push("invalid daemon receipt acknowledgement status".to_owned());
        }
    }

    validate_c2pa_claim(data.get("c2pa_claim"), &mut errors);

    if let Some(Value::Object(portable)) = data.get("portable_signature") {
        if !is_string_equal(
            get(portable, "signer_identity_proof_level"),
            "unknown_unobserved",
        ) {
            errors.push(
                "self-generated portable signer identity must remain unknown_unobserved".to_owned(),
            );
        }
        if !is_string_equal(get(portable, "trust_scope"), "self_generated_demo_key_integrity") {
            errors.push("portable signature trust scope is invalid".to_owned());
        }
    }

    errors
}

fn validate_stems(stems: &[Value], errors: &mut Vec<String>) {
    for (index, stem) in stems.iter().enumerate() {
        let Value::Object(stem) = stem else {
            errors.push(format!("observed_stems[{index}] must be an object"));
            continue;
        };
        require_proof(stem, &format!("observed_stems[{index}]"), errors);
        if !stem.contains_key("source_category_proof_level") {
            errors.push(format!(
                "observed_stems[{index}] missing source_category_proof_level"
            ));
        }
    }
}

fn validate_association(association: &Map<String, Value>, errors: &mut Vec<String>) {
    require_proof(association, "stem_export_association", errors);
    let status = get(association, "status");
    let proof = get(association, PROOF_LEVEL_KEY);
    if is_string_equal(status, "inferred_match") && !is_string_equal(proof, "inferred") {
        errors.push("established stem_export_association must remain inferred".to_owned());
    }
    let unavailable =
        is_string_equal(status, "not_established") || is_string_equal(status, "unavailable");
    if unavailable && !is_string_equal(proof, "unknown_unobserved") {
        errors.push("unavailable association must be unknown_unobserved".to_owned());
    }
    if !ASSOCIATION_STATUSES
        .iter()
        .any(|candidate| is_string_equal(status, candidate))
    {
        errors.push(format!(
            "invalid stem_export_association status: {}",
            python_str(status)
        ));
    }
}

fn validate_host_environment(host: &Map<String, Value>, errors: &mut Vec<String>) {
    require_proof(host, "host_environment", errors);
    if !status_in(host, &HOST_ENVIRONMENT_STATUSES) {
        errors.push(format!(
            "invalid host_environment status: {}",
            python_str(get(host, "status"))
        ));
        return;
    }
    if is_string_equal(get(host, "status"), "observed") {
        if !is_string_equal(get(host, PROOF_LEVEL_KEY), ProofLevel::DirectlyObserved.as_str()) {
            errors.push("an observed host_environment records a directly observed host".to_owned());
        }
        if !is_truthy(get(host, "host_name")) {
            errors.push("an observed host_environment must name the host".to_owned());
        }
        return;
    }
    // IMPORTANT: honesty constraint 1. The wrapper reports "Unknown" for any host
    // outside JUCE's table, so a name surviving here would sign an absence as an
    // observation.
    if !is_string_equal(get(host, PROOF_LEVEL_KEY), ProofLevel::UnknownUnobserved.as_str()) {
        errors.push("an unidentified host_environment must remain unknown_unobserved".to_owned());
    }
    if !matches!(get(host, "host_name"), Value::Null) {
        errors.push("an unidentified host_environment must not name a host".to_owned());
    }
    if !matches!(get(host, "host_recognised"), Value::Bool(false)) {
        errors.push(
            "an unidentified host_environment must not report the host as recognised".to_owned(),
        );
    }
}

fn validate_coverage(coverage: &Map<String, Value>, errors: &mut Vec<String>) {
    require_proof(coverage, "observation_coverage", errors);
    let status = get(coverage, "status");
    if !COVERAGE_STATUSES
        .iter()
        .any(|candidate| is_string_equal(status, candidate))
    {
        errors.push(format!(
            "invalid observation coverage status: {}",
            python_str(status)
        ));
    }
    if !is_string_equal(status, "complete_observed_path") {
        return;
    }
    let Some(Value::Object(counters)) = coverage.get("counters") else {
        errors.push("complete_observed_path requires counters".to_owned());
        return;
    };
    for key in REQUIRED_COVERAGE_COUNTERS {
        if !counters.contains_key(key) {
            errors.push(format!("complete_observed_path missing counter: {key}"));
        }
    }
    if !python_eq(
        get(counters, "windows_hashed"),
        get(counters, "buffer_hash_events_received"),
    ) {
        errors.push("complete_observed_path requires all hashed windows to be received".to_owned());
    }
    if !python_eq(
        get(counters, "events_prepared"),
        get(counters, "events_received"),
    ) {
        errors.push(
            "complete_observed_path requires the prepared/received event prefix to agree"
                .to_owned(),
        );
    }
    let zero = Value::from(0u8);
    for key in ZERO_COVERAGE_COUNTERS {
        let observed = counters.get(key).unwrap_or(&zero);
        if !python_eq(observed, &zero) {
            errors.push(format!("complete_observed_path requires {key}=0"));
        }
    }
    // IMPORTANT: per received PACKET, not per accepted event. The daemon
    // acknowledges every datagram including rejections, so requiring parity with
    // events_received made one stray packet produce a manifest the daemon's own
    // verifier rejected as schema_invalid.
    if !python_eq(
        get(counters, "daemon_acknowledgements_sent"),
        get(counters, "packets_received"),
    ) {
        errors.push(
            "complete_observed_path requires one daemon ACK dispatch per received packet"
                .to_owned(),
        );
    }
}

/// A real signed claim may never read as externally verified identity. The chain
/// is issued by a root this machine generated, so a `verified` validation state
/// means the claim chains to our own anchor and nothing more.
fn validate_c2pa_claim(claim: Option<&Value>, errors: &mut Vec<String>) {
    let claim = match claim {
        None | Some(Value::Null) => return,
        Some(Value::Object(claim)) => claim,
        Some(_) => {
            errors.push("c2pa_claim must be an object".to_owned());
            return;
        }
    };
    require_proof(claim, "c2pa_claim", errors);
    let status = get(claim, "status");
    if !C2PA_CLAIM_STATUSES
        .iter()
        .any(|candidate| is_string_equal(status, candidate))
    {
        errors.push(format!("invalid c2pa_claim status: {}", python_str(status)));
        return;
    }
    if is_string_equal(status, "unavailable") {
        if !is_string_equal(get(claim, PROOF_LEVEL_KEY), "unknown_unobserved") {
            errors.push("an unavailable c2pa_claim must be unknown_unobserved".to_owned());
        }
        if !is_truthy(get(claim, "reason")) {
            errors.push("an unavailable c2pa_claim must state a reason".to_owned());
        }
        return;
    }

    if !is_string_equal(get(claim, PROOF_LEVEL_KEY), "directly_observed") {
        errors.push("a signed c2pa_claim records a directly observed signing act".to_owned());
    }
    if !matches!(claim.get("hard_binding"), Some(Value::Object(_))) {
        errors.push("a signed c2pa_claim must record its hard binding".to_owned());
    }
    if !matches!(claim.get("source_sha256_matches_export"), Some(Value::Bool(true))) {
        errors.push("the c2pa_claim hard binding must cover the hashed export bytes".to_owned());
    }

    match claim.get("validation") {
        Some(Value::Object(validation)) => {
            let state = get(validation, "state");
            if !crate::state::VerificationState::ALL
                .iter()
                .any(|candidate| is_string_equal(state, candidate.as_str()))
            {
                errors.push(format!(
                    "invalid c2pa_claim validation state: {}",
                    python_str(state)
                ));
            } else if !is_string_equal(
                get(validation, "trust_anchor_scope"),
                "self_issued_local_root_only",
            ) {
                errors
                    .push("c2pa_claim trust must stay scoped to the self-issued local root".to_owned());
            }
        }
        _ => errors.push("a signed c2pa_claim must record its validation result".to_owned()),
    }

    match claim.get("signer") {
        Some(Value::Object(signer)) => {
            require_proof(signer, "c2pa_claim.signer", errors);
            if is_string_equal(get(signer, PROOF_LEVEL_KEY), "externally_verified") {
                errors
                    .push("a self-issued c2pa signer identity must not be externally_verified".to_owned());
            }
            if !is_string_equal(get(signer, "signer_identity"), "not_established") {
                errors.push("c2pa_claim signer identity must remain not_established".to_owned());
            }
        }
        _ => errors.push("a signed c2pa_claim must record its signer".to_owned()),
    }

    match claim.get("ingredients") {
        Some(Value::Array(ingredients)) => {
            for (index, ingredient) in ingredients.iter().enumerate() {
                let Value::Object(ingredient) = ingredient else {
                    errors.push(format!(
                        "c2pa_claim.ingredients[{index}] must be an object"
                    ));
                    continue;
                };
                require_proof(ingredient, &format!("c2pa_claim.ingredients[{index}]"), errors);
                if !is_truthy(get(ingredient, "sha256")) {
                    errors.push(format!(
                        "c2pa_claim.ingredients[{index}] must record a digest even when \
                         it has no provenance of its own"
                    ));
                }
            }
        }
        _ => errors.push("c2pa_claim ingredients must be a list".to_owned()),
    }
}

/// Real manifests nest ~6 levels; a crafted deeply-nested one must produce a
/// finding, not a stack overflow out of the verifier.
fn validate_proof_values(value: &Value, path: &str, errors: &mut Vec<String>, depth: usize) {
    if depth >= MAX_PROOF_VALUE_DEPTH {
        errors.push(format!(
            "manifest nesting exceeds {MAX_PROOF_VALUE_DEPTH} levels at {path}"
        ));
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = format!("{path}.{key}");
                let is_proof_key = key == PROOF_LEVEL_KEY || key.ends_with("_proof_level");
                if is_proof_key && !is_valid_proof_level(child) {
                    errors.push(format!(
                        "invalid proof level at {child_path}: {}",
                        python_str(child)
                    ));
                }
                validate_proof_values(child, &child_path, errors, depth + 1);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                validate_proof_values(child, &format!("{path}[{index}]"), errors, depth + 1);
            }
        }
        _ => {}
    }
}

fn is_valid_proof_level(value: &Value) -> bool {
    match value {
        Value::String(text) => ProofLevel::parse(text).is_ok(),
        _ => false,
    }
}

fn require_proof(value: &Map<String, Value>, path: &str, errors: &mut Vec<String>) {
    if !is_valid_proof_level(get(value, PROOF_LEVEL_KEY)) {
        errors.push(format!("{path} must have a valid apw:proof_level"));
    }
}

fn status_in(map: &Map<String, Value>, allowed: &[&str]) -> bool {
    let status = get(map, "status");
    allowed
        .iter()
        .any(|candidate| is_string_equal(status, candidate))
}
