//! A port of `daemon/schema.py::validate_manifest_invariants` from the audio-provenance POC.
//!
//! It accumulates: the Python never fails fast, and one malformed section legitimately produces
//! several findings. Every rule here has a one-to-one counterpart there, and the messages are the
//! Python's own so a finding can be traced back to its line.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use audio_provenance_core::{LocatorSalt, ObservationCounters, ProofLevel};
use serde_json::Value;

use crate::finding::Finding;

pub const APW_SCHEMA_ID: &str = "audio-provenance-manifest-v0";

/// Mirrors `_MAX_PROOF_VALUE_DEPTH`. Untrusted input must produce a finding, never a stack
/// overflow.
pub const MAX_PROOF_VALUE_DEPTH: usize = 64;

const REQUIRED_FIELDS: [&str; 12] = [
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

/// IMPORTANT: this is `validate_manifest_invariants`'s list, which is SHORTER than the 17 required
/// fields in `docs/manifest.schema.json`. The JSON Schema additionally demands `apw_version`,
/// `evidence_binding`, `portable_signature`, `manifest_signature`, `presentation` and
/// `downstream_registration_handoff`; enforcing those here would reject manifests the POC's own
/// verifier accepts.
const REQUIRED_COMPLETE_PATH_COUNTERS: [&str; 13] = [
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

const ZERO_COMPLETE_PATH_COUNTERS: [&str; 10] = [
    "fifo_samples_dropped",
    "fifo_windows_dropped",
    "midi_events_dropped",
    "udp_sends_failed",
    "sequence_gaps",
    "hash_chain_breaks",
    "stream_evictions",
    "daemon_acknowledgements_failed",
    "plugin_telemetry_regressions",
    "bypassed_buffers",
];

const C2PA_CLAIM_STATUSES: [&str; 3] = ["embedded", "sidecar", "unavailable"];

/// The POC's four C2PA validation states and Audio Provenance's four verification statuses are the same
/// four outcomes under two names.
///
/// | `c2pa_claim.validation.state`   | [`audio_provenance_core::VerificationStatus`] |
/// |---------------------------------|---------------------------------------|
/// | `verified`                      | `Verified`                            |
/// | `registered_but_changed`        | `Changed`                             |
/// | `mark_found_claim_not_trusted`  | `Untrusted`                           |
/// | `nothing_found`                 | `NotFound`                            |
///
/// Both vocabularies separate "the signature does not chain to an anchor" from "the bytes moved
/// under a signature that does", which is the distinction that keeps a self-signed altered file
/// from reading as a registration the system recognises.
pub const C2PA_VALIDATION_STATES: [&str; 4] = [
    "verified",
    "registered_but_changed",
    "mark_found_claim_not_trusted",
    "nothing_found",
];

pub fn c2pa_validation_state_to_status(
    state: &str,
) -> Option<audio_provenance_core::VerificationStatus> {
    use audio_provenance_core::VerificationStatus as S;
    match state {
        "verified" => Some(S::Verified),
        "registered_but_changed" => Some(S::Changed),
        "mark_found_claim_not_trusted" => Some(S::Untrusted),
        "nothing_found" => Some(S::NotFound),
        _ => None,
    }
}

pub fn validate_apw_invariants(data: &Value) -> Vec<Finding> {
    let mut findings = Vec::new();
    let Value::Object(root) = data else {
        findings.push(Finding::error(
            "manifest_not_an_object",
            "$",
            "manifest must be a JSON object",
        ));
        return findings;
    };

    for field in REQUIRED_FIELDS {
        if !root.contains_key(field) {
            findings.push(Finding::error(
                "required_field_missing",
                format!("$.{field}"),
                format!("required field is missing: {field}"),
            ));
        }
    }

    validate_proof_values(data, "$", 0, &mut findings);
    validate_stems(root.get("observed_stems"), &mut findings);
    validate_export(root, &mut findings);
    validate_claim_summary(root.get("claim_summary"), &mut findings);
    validate_association(root.get("stem_export_association"), &mut findings);
    validate_coverage(root.get("observation_coverage"), &mut findings);
    validate_receipt(root.get("daemon_receipt_acknowledgement"), &mut findings);
    validate_c2pa_claim(root.get("c2pa_claim"), &mut findings);
    validate_portable_signature(root.get("portable_signature"), &mut findings);

    findings
}

fn validate_stems(stems: Option<&Value>, findings: &mut Vec<Finding>) {
    let Some(stems) = stems else { return };
    let Some(items) = stems.as_array() else {
        findings.push(Finding::error(
            "observed_stems_not_a_list",
            "$.observed_stems",
            "observed_stems must be a list",
        ));
        return;
    };
    for (index, stem) in items.iter().enumerate() {
        let path = format!("$.observed_stems[{index}]");
        let Some(stem) = stem.as_object() else {
            findings.push(Finding::error(
                "observed_stem_not_an_object",
                path.clone(),
                format!("observed_stems[{index}] must be an object"),
            ));
            continue;
        };
        require_proof(stem, &path, &format!("observed_stems[{index}]"), findings);
        if !stem.contains_key("source_category_proof_level") {
            findings.push(Finding::error(
                "observed_stem_source_category_proof_level_missing",
                path,
                format!("observed_stems[{index}] missing source_category_proof_level"),
            ));
        }
    }
}

fn validate_export(root: &serde_json::Map<String, Value>, findings: &mut Vec<Finding>) {
    let Some(export) = root.get("export").and_then(Value::as_object) else {
        return;
    };
    require_proof(export, "$.export", "export", findings);
    let signed = root
        .get("portable_signature")
        .is_some_and(|value| !value.is_null());
    if !signed {
        return;
    }
    // The Python stringifies whatever is there and measures the result, so a non-string sha256 is
    // caught by the same length test rather than a type test.
    let length = match export.get("sha256") {
        Some(Value::String(text)) => text.chars().count(),
        Some(other) => python_str_len(other),
        None => 0,
    };
    if length != 64 {
        findings.push(Finding::error(
            "export_sha256_malformed",
            "$.export.sha256",
            "export.sha256 must be a 64-character SHA-256 hex digest",
        ));
    }
}

/// `str(value)` for the shapes that can appear in `export.sha256`. Only its LENGTH is compared
/// against 64, and no real digest is a container, so an approximation that cannot reach 64 for
/// non-string input reproduces the Python's verdict exactly.
fn python_str_len(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::Number(n) => n.to_string().chars().count(),
        Value::String(text) => text.chars().count(),
        Value::Array(_) | Value::Object(_) => 0,
    }
}

fn validate_claim_summary(claims: Option<&Value>, findings: &mut Vec<Finding>) {
    let Some(items) = claims.and_then(Value::as_array) else {
        return;
    };
    for (index, claim) in items.iter().enumerate() {
        if let Some(claim) = claim.as_object() {
            require_proof(
                claim,
                &format!("$.claim_summary[{index}]"),
                &format!("claim_summary[{index}]"),
                findings,
            );
        }
    }
}

fn validate_association(association: Option<&Value>, findings: &mut Vec<Finding>) {
    let Some(association) = association.and_then(Value::as_object) else {
        return;
    };
    require_proof(
        association,
        "$.stem_export_association",
        "stem_export_association",
        findings,
    );
    let status = association.get("status").and_then(Value::as_str);
    let proof = association.get("apw:proof_level").and_then(Value::as_str);
    if status == Some("inferred_match") && proof != Some(ProofLevel::Inferred.as_str()) {
        findings.push(Finding::error(
            "association_proof_level_overstated",
            "$.stem_export_association",
            "established stem_export_association must remain inferred",
        ));
    }
    if matches!(status, Some("not_established" | "unavailable"))
        && proof != Some(ProofLevel::UnknownUnobserved.as_str())
    {
        findings.push(Finding::error(
            "association_proof_level_understated",
            "$.stem_export_association",
            "unavailable association must be unknown_unobserved",
        ));
    }
    if !matches!(
        status,
        Some("inferred_match" | "not_established" | "unavailable")
    ) {
        findings.push(Finding::error(
            "association_status_invalid",
            "$.stem_export_association.status",
            format!(
                "invalid stem_export_association status: {}",
                render_optional(association.get("status"))
            ),
        ));
    }
}

fn validate_coverage(coverage: Option<&Value>, findings: &mut Vec<Finding>) {
    let Some(coverage) = coverage.and_then(Value::as_object) else {
        return;
    };
    require_proof(
        coverage,
        "$.observation_coverage",
        "observation_coverage",
        findings,
    );
    let status = coverage.get("status").and_then(Value::as_str);
    if !matches!(
        status,
        Some("complete_observed_path" | "partial_observed_path" | "unknown_coverage")
    ) {
        findings.push(Finding::error(
            "coverage_status_invalid",
            "$.observation_coverage.status",
            format!(
                "invalid observation coverage status: {}",
                render_optional(coverage.get("status"))
            ),
        ));
    }
    if status != Some("complete_observed_path") {
        return;
    }
    let Some(counters) = coverage.get("counters").and_then(Value::as_object) else {
        findings.push(Finding::error(
            "coverage_counters_missing",
            "$.observation_coverage.counters",
            "complete_observed_path requires counters",
        ));
        return;
    };
    for key in REQUIRED_COMPLETE_PATH_COUNTERS {
        if !counters.contains_key(key) {
            findings.push(Finding::error(
                "coverage_counter_missing",
                format!("$.observation_coverage.counters.{key}"),
                format!("complete_observed_path missing counter: {key}"),
            ));
        }
    }
    if counters.get("windows_hashed") != counters.get("buffer_hash_events_received") {
        findings.push(Finding::error(
            "coverage_windows_not_all_received",
            "$.observation_coverage.counters",
            "complete_observed_path requires all hashed windows to be received",
        ));
    }
    if counters.get("events_prepared") != counters.get("events_received") {
        findings.push(Finding::error(
            "coverage_event_prefix_disagrees",
            "$.observation_coverage.counters",
            "complete_observed_path requires the prepared/received event prefix to agree",
        ));
    }
    for key in ZERO_COMPLETE_PATH_COUNTERS {
        let value = counters.get(key);
        let is_zero = match value {
            None => true,
            Some(Value::Number(n)) => n.as_f64() == Some(0.0),
            Some(_) => false,
        };
        if !is_zero {
            findings.push(Finding::error(
                "coverage_drop_counter_nonzero",
                format!("$.observation_coverage.counters.{key}"),
                format!("complete_observed_path requires {key}=0"),
            ));
        }
    }
    // IMPORTANT: parity is per received PACKET, not per accepted event. The daemon acknowledges
    // every datagram including the ones it rejects.
    if counters.get("daemon_acknowledgements_sent") != counters.get("packets_received") {
        findings.push(Finding::error(
            "coverage_ack_parity_broken",
            "$.observation_coverage.counters",
            "complete_observed_path requires one daemon ACK dispatch per received packet",
        ));
    }

    // The typed vocabulary and the POC invariant intentionally evaluate the
    // same conjunction. Keep this check as a regression alarm if either side
    // changes without the other.
    if let Ok(typed) =
        serde_json::from_value::<ObservationCounters>(Value::Object(counters.clone()))
        && !typed.proves_complete_observed_path()
    {
        findings.push(Finding::warning(
            "coverage_conjunction_divergence",
            "$.observation_coverage.counters",
            "these counters satisfy daemon/schema.py's complete_observed_path conjunction but not \
             audio-provenance-core's; the cross-language coverage rules have diverged",
        ));
    }
}

fn validate_receipt(receipt: Option<&Value>, findings: &mut Vec<Finding>) {
    let Some(receipt) = receipt.and_then(Value::as_object) else {
        return;
    };
    require_proof(
        receipt,
        "$.daemon_receipt_acknowledgement",
        "daemon_receipt_acknowledgement",
        findings,
    );
    if !matches!(
        receipt.get("status").and_then(Value::as_str),
        Some("issued" | "degraded" | "unknown")
    ) {
        findings.push(Finding::error(
            "daemon_receipt_status_invalid",
            "$.daemon_receipt_acknowledgement.status",
            "invalid daemon receipt acknowledgement status",
        ));
    }
}

/// A real signed claim may never read as externally verified identity: the chain is issued by a
/// root the signing machine generated, so `verified` means it chains to our own anchor and nothing
/// more.
///
/// IMPORTANT: absence is not an error, matching the Python. A manifest written before the section
/// existed stays valid; `daemon/verify.py` raises `c2pa_claim_missing` for those instead.
fn validate_c2pa_claim(claim: Option<&Value>, findings: &mut Vec<Finding>) {
    let Some(claim) = claim else { return };
    if claim.is_null() {
        return;
    }
    let Some(claim) = claim.as_object() else {
        findings.push(Finding::error(
            "c2pa_claim_not_an_object",
            "$.c2pa_claim",
            "c2pa_claim must be an object",
        ));
        return;
    };
    require_proof(claim, "$.c2pa_claim", "c2pa_claim", findings);
    let status = claim.get("status").and_then(Value::as_str);
    let Some(status) = status.filter(|s| C2PA_CLAIM_STATUSES.contains(s)) else {
        findings.push(Finding::error(
            "c2pa_claim_status_invalid",
            "$.c2pa_claim.status",
            format!(
                "invalid c2pa_claim status: {}",
                render_optional(claim.get("status"))
            ),
        ));
        return;
    };
    let proof = claim.get("apw:proof_level").and_then(Value::as_str);
    if status == "unavailable" {
        if proof != Some(ProofLevel::UnknownUnobserved.as_str()) {
            findings.push(Finding::error(
                "c2pa_claim_unavailable_proof_level_overstated",
                "$.c2pa_claim",
                "an unavailable c2pa_claim must be unknown_unobserved",
            ));
        }
        if !is_truthy(claim.get("reason")) {
            findings.push(Finding::error(
                "c2pa_claim_unavailable_reason_missing",
                "$.c2pa_claim.reason",
                "an unavailable c2pa_claim must state a reason",
            ));
        }
        return;
    }

    if proof != Some(ProofLevel::DirectlyObserved.as_str()) {
        findings.push(Finding::error(
            "c2pa_claim_signing_act_not_observed",
            "$.c2pa_claim",
            "a signed c2pa_claim records a directly observed signing act",
        ));
    }
    if !claim.get("hard_binding").is_some_and(Value::is_object) {
        findings.push(Finding::error(
            "c2pa_claim_hard_binding_missing",
            "$.c2pa_claim.hard_binding",
            "a signed c2pa_claim must record its hard binding",
        ));
    }
    if claim.get("source_sha256_matches_export") != Some(&Value::Bool(true)) {
        findings.push(Finding::error(
            "c2pa_claim_binding_does_not_cover_export",
            "$.c2pa_claim.source_sha256_matches_export",
            "the c2pa_claim hard binding must cover the hashed export bytes",
        ));
    }

    match claim.get("validation").and_then(Value::as_object) {
        None => findings.push(Finding::error(
            "c2pa_claim_validation_missing",
            "$.c2pa_claim.validation",
            "a signed c2pa_claim must record its validation result",
        )),
        Some(validation) => {
            let state = validation.get("state").and_then(Value::as_str);
            if !state.is_some_and(|s| C2PA_VALIDATION_STATES.contains(&s)) {
                findings.push(Finding::error(
                    "c2pa_claim_validation_state_invalid",
                    "$.c2pa_claim.validation.state",
                    format!(
                        "invalid c2pa_claim validation state: {}",
                        render_optional(validation.get("state"))
                    ),
                ));
            } else if validation.get("trust_anchor_scope").and_then(Value::as_str)
                != Some("self_issued_local_root_only")
            {
                findings.push(Finding::error(
                    "c2pa_claim_trust_anchor_scope_invalid",
                    "$.c2pa_claim.validation.trust_anchor_scope",
                    "c2pa_claim trust must stay scoped to the self-issued local root",
                ));
            }
        }
    }

    match claim.get("signer").and_then(Value::as_object) {
        None => findings.push(Finding::error(
            "c2pa_claim_signer_missing",
            "$.c2pa_claim.signer",
            "a signed c2pa_claim must record its signer",
        )),
        Some(signer) => {
            require_proof(signer, "$.c2pa_claim.signer", "c2pa_claim.signer", findings);
            if signer.get("apw:proof_level").and_then(Value::as_str)
                == Some(ProofLevel::ExternallyVerified.as_str())
            {
                findings.push(Finding::error(
                    "c2pa_claim_signer_identity_overstated",
                    "$.c2pa_claim.signer",
                    "a self-issued c2pa signer identity must not be externally_verified",
                ));
            }
            if signer.get("signer_identity").and_then(Value::as_str) != Some("not_established") {
                findings.push(Finding::error(
                    "c2pa_claim_signer_identity_established",
                    "$.c2pa_claim.signer.signer_identity",
                    "c2pa_claim signer identity must remain not_established",
                ));
            }
        }
    }

    match claim.get("ingredients").and_then(Value::as_array) {
        None => findings.push(Finding::error(
            "c2pa_claim_ingredients_not_a_list",
            "$.c2pa_claim.ingredients",
            "c2pa_claim ingredients must be a list",
        )),
        Some(ingredients) => {
            for (index, ingredient) in ingredients.iter().enumerate() {
                let path = format!("$.c2pa_claim.ingredients[{index}]");
                let Some(ingredient) = ingredient.as_object() else {
                    findings.push(Finding::error(
                        "c2pa_claim_ingredient_not_an_object",
                        path,
                        format!("c2pa_claim.ingredients[{index}] must be an object"),
                    ));
                    continue;
                };
                require_proof(
                    ingredient,
                    &path,
                    &format!("c2pa_claim.ingredients[{index}]"),
                    findings,
                );
                if !is_truthy(ingredient.get("sha256")) {
                    findings.push(Finding::error(
                        "c2pa_claim_ingredient_digest_missing",
                        path,
                        format!(
                            "c2pa_claim.ingredients[{index}] must record a digest even when it \
                             has no provenance of its own"
                        ),
                    ));
                }
            }
        }
    }
}

fn validate_portable_signature(portable: Option<&Value>, findings: &mut Vec<Finding>) {
    let Some(portable) = portable.and_then(Value::as_object) else {
        return;
    };
    if portable
        .get("signer_identity_proof_level")
        .and_then(Value::as_str)
        != Some(ProofLevel::UnknownUnobserved.as_str())
    {
        findings.push(Finding::error(
            "signer_identity_proof_level_overstated",
            "$.portable_signature.signer_identity_proof_level",
            "self-generated portable signer identity must remain unknown_unobserved",
        ));
    }
    if portable.get("trust_scope").and_then(Value::as_str)
        != Some("self_generated_demo_key_integrity")
    {
        findings.push(Finding::error(
            "portable_signature_trust_scope_invalid",
            "$.portable_signature.trust_scope",
            "portable signature trust scope is invalid",
        ));
    }
}

fn validate_proof_values(value: &Value, path: &str, depth: usize, findings: &mut Vec<Finding>) {
    if depth >= MAX_PROOF_VALUE_DEPTH {
        findings.push(Finding::error(
            "manifest_nesting_exceeded",
            path.to_owned(),
            format!("manifest nesting exceeds {MAX_PROOF_VALUE_DEPTH} levels at {path}"),
        ));
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = format!("{path}.{key}");
                if (key == "apw:proof_level" || key.ends_with("_proof_level"))
                    && !is_proof_level(child)
                {
                    findings.push(Finding::error(
                        "proof_level_invalid",
                        child_path.clone(),
                        format!("invalid proof level at {child_path}: {}", render(child)),
                    ));
                }
                validate_proof_values(child, &child_path, depth + 1, findings);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                validate_proof_values(child, &format!("{path}[{index}]"), depth + 1, findings);
            }
        }
        _ => {}
    }
}

fn require_proof(
    value: &serde_json::Map<String, Value>,
    path: &str,
    label: &str,
    findings: &mut Vec<Finding>,
) {
    if !value.get("apw:proof_level").is_some_and(is_proof_level) {
        findings.push(Finding::error(
            "proof_level_missing",
            path.to_owned(),
            format!("{label} must have a valid apw:proof_level"),
        ));
    }
}

fn is_proof_level(value: &Value) -> bool {
    value.as_str().is_some_and(|text| {
        serde_json::from_value::<ProofLevel>(Value::String(text.into())).is_ok()
    })
}

fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(map)) => !map.is_empty(),
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::Bool(true)) => true,
    }
}

fn render(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

fn render_optional(value: Option<&Value>) -> String {
    value.map_or_else(|| "None".into(), render)
}

const AUDIO_PROVENANCE_REQUIRED_FIELDS: [&str; 6] = [
    "schema",
    "signed_at",
    "hard_binding",
    "locator_salt",
    "claims",
    "portable_signature",
];

/// The Audio Provenance record's invariants. It shares the proof-level lattice, the association rule and
/// the portable-signature rules with the POC record, because those encode the honesty constraints
/// rather than the file format.
pub fn validate_audio_provenance_invariants(data: &Value) -> Vec<Finding> {
    let mut findings = Vec::new();
    let Value::Object(root) = data else {
        findings.push(Finding::error(
            "manifest_not_an_object",
            "$",
            "manifest must be a JSON object",
        ));
        return findings;
    };

    for field in AUDIO_PROVENANCE_REQUIRED_FIELDS {
        if !root.contains_key(field) {
            findings.push(Finding::error(
                "required_field_missing",
                format!("$.{field}"),
                format!("required field is missing: {field}"),
            ));
        }
    }

    validate_proof_values(data, "$", 0, &mut findings);
    validate_claims(root.get("claims"), &mut findings);
    validate_association(root.get("stem_export_association"), &mut findings);
    validate_coverage(root.get("observation_coverage"), &mut findings);
    validate_portable_signature(root.get("portable_signature"), &mut findings);
    validate_mark(root.get("mark"), &mut findings);
    validate_locator_salt(root.get("locator_salt"), &mut findings);

    findings
}

/// IMPORTANT: a malformed salt has to fail HERE, as a manifest verdict. The registry rejects one
/// too, but a registry-layer refusal reaches a verifier as `Unavailable(IndexCorrupt)`, which reads
/// as "the registry was down" for a record that is simply invalid.
fn validate_locator_salt(salt: Option<&Value>, findings: &mut Vec<Finding>) {
    let Some(salt) = salt else { return };
    if salt
        .as_str()
        .is_some_and(|text| LocatorSalt::parse_hex(text).is_ok())
    {
        return;
    }
    findings.push(Finding::error(
        "locator_salt_malformed",
        "$.locator_salt",
        format!(
            "locator_salt must be {} lowercase hex characters, found {}",
            audio_provenance_core::LOCATOR_SALT_HEX_LEN,
            render(salt)
        ),
    ));
}

fn validate_claims(claims: Option<&Value>, findings: &mut Vec<Finding>) {
    let Some(claims) = claims else { return };
    let Some(items) = claims.as_array() else {
        findings.push(Finding::error(
            "claims_not_a_list",
            "$.claims",
            "claims must be a list",
        ));
        return;
    };
    for (index, claim) in items.iter().enumerate() {
        let path = format!("$.claims[{index}]");
        let Some(claim) = claim.as_object() else {
            findings.push(Finding::error(
                "claim_not_an_object",
                path,
                format!("claims[{index}] must be an object"),
            ));
            continue;
        };
        require_proof(claim, &path, &format!("claims[{index}]"), findings);
    }
}

/// IMPORTANT: `GENOMARK_SPEC` §5 gives version and namespace four bits each. A wider value would
/// silently overflow into its neighbour when the payload is packed.
fn validate_mark(mark: Option<&Value>, findings: &mut Vec<Finding>) {
    let Some(mark) = mark.and_then(Value::as_object) else {
        return;
    };
    for field in ["version", "namespace"] {
        match mark.get(field).and_then(Value::as_u64) {
            Some(0..=0x0f) => {}
            other => findings.push(Finding::error(
                "mark_field_out_of_range",
                format!("$.mark.{field}"),
                format!(
                    "mark.{field} must be a 4-bit value in 0..=15, found {}",
                    other.map_or_else(|| "None".to_owned(), |v| v.to_string())
                ),
            )),
        }
    }
}
