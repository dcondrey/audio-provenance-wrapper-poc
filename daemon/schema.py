from __future__ import annotations

from typing import Any

PROOF_LEVELS = {
    "directly_observed",
    "inferred",
    "user_declared",
    "externally_verified",
    "unknown_unobserved",
}
COVERAGE_STATUSES = {
    "complete_observed_path",
    "partial_observed_path",
    "unknown_coverage",
}


def validate_manifest_invariants(data: object) -> list[str]:
    errors: list[str] = []
    if not isinstance(data, dict):
        return ["manifest must be a JSON object"]
    for key in (
        "apw_version", "schema", "session_id", "capture_session", "created_at",
        "observed_stems", "claim_summary", "stem_export_association",
        "observation_coverage", "daemon_receipt_acknowledgement",
        "apw:unobserved", "c2pa_mapping",
    ):
        if key not in data:
            errors.append(f"required field is missing: {key}")

    _validate_proof_values(data, "$", errors)

    stems = data.get("observed_stems", [])
    if not isinstance(stems, list):
        errors.append("observed_stems must be a list")
        stems = []
    for index, stem in enumerate(stems):
        if not isinstance(stem, dict):
            errors.append(f"observed_stems[{index}] must be an object")
            continue
        _require_proof(stem, f"observed_stems[{index}]", errors)
        if "source_category_proof_level" not in stem:
            errors.append(f"observed_stems[{index}] missing source_category_proof_level")

    export = data.get("export")
    if export is not None and isinstance(export, dict):
        _require_proof(export, "export", errors)
        if data.get("portable_signature") is not None and len(str(export.get("sha256", ""))) != 64:
            errors.append("export.sha256 must be a 64-character SHA-256 hex digest")

    claims = data.get("claim_summary", [])
    if isinstance(claims, list):
        for index, claim in enumerate(claims):
            if isinstance(claim, dict):
                _require_proof(claim, f"claim_summary[{index}]", errors)

    association = data.get("stem_export_association")
    if isinstance(association, dict):
        _require_proof(association, "stem_export_association", errors)
        status = association.get("status")
        proof = association.get("apw:proof_level")
        if status == "inferred_match" and proof != "inferred":
            errors.append("established stem_export_association must remain inferred")
        if status in {"not_established", "unavailable"} and proof != "unknown_unobserved":
            errors.append("unavailable association must be unknown_unobserved")
        if status not in {"inferred_match", "not_established", "unavailable"}:
            errors.append(f"invalid stem_export_association status: {status}")

    coverage = data.get("observation_coverage")
    if isinstance(coverage, dict):
        _require_proof(coverage, "observation_coverage", errors)
        status = coverage.get("status")
        if status not in COVERAGE_STATUSES:
            errors.append(f"invalid observation coverage status: {status}")
        if status == "complete_observed_path":
            counters = coverage.get("counters")
            if not isinstance(counters, dict):
                errors.append("complete_observed_path requires counters")
            else:
                for key in (
                    "windows_hashed", "buffer_hash_events_received", "fifo_samples_dropped",
                    "fifo_windows_dropped", "udp_sends_failed", "sequence_gaps",
                    "hash_chain_breaks", "events_prepared", "events_received",
                    "daemon_acknowledgements_sent", "daemon_acknowledgements_failed",
                ):
                    if key not in counters:
                        errors.append(f"complete_observed_path missing counter: {key}")
                if counters.get("windows_hashed") != counters.get("buffer_hash_events_received"):
                    errors.append("complete_observed_path requires all hashed windows to be received")
                if counters.get("events_prepared") != counters.get("events_received"):
                    errors.append("complete_observed_path requires the prepared/received event prefix to agree")
                for key in (
                    "fifo_samples_dropped", "fifo_windows_dropped", "midi_events_dropped",
                    "udp_sends_failed", "sequence_gaps", "hash_chain_breaks",
                    "stream_evictions", "daemon_acknowledgements_failed",
                ):
                    if counters.get(key, 0) != 0:
                        errors.append(f"complete_observed_path requires {key}=0")
                if counters.get("daemon_acknowledgements_sent") != counters.get("events_received"):
                    errors.append("complete_observed_path requires one daemon ACK dispatch per received event")

    receipt = data.get("daemon_receipt_acknowledgement")
    if isinstance(receipt, dict):
        _require_proof(receipt, "daemon_receipt_acknowledgement", errors)
        if receipt.get("status") not in {"issued", "degraded", "unknown"}:
            errors.append("invalid daemon receipt acknowledgement status")

    portable = data.get("portable_signature")
    if isinstance(portable, dict):
        if portable.get("signer_identity_proof_level") != "unknown_unobserved":
            errors.append("self-generated portable signer identity must remain unknown_unobserved")
        if portable.get("trust_scope") != "self_generated_demo_key_integrity":
            errors.append("portable signature trust scope is invalid")

    return errors


# Real manifests nest ~6 levels; a crafted deeply-nested one must produce a
# finding, not a RecursionError out of the verifier (untrusted verifier input).
_MAX_PROOF_VALUE_DEPTH = 64


def _validate_proof_values(value: Any, path: str, errors: list[str], depth: int = 0) -> None:
    if depth >= _MAX_PROOF_VALUE_DEPTH:
        errors.append(f"manifest nesting exceeds {_MAX_PROOF_VALUE_DEPTH} levels at {path}")
        return
    if isinstance(value, dict):
        for key, child in value.items():
            child_path = f"{path}.{key}"
            if (key == "apw:proof_level" or key.endswith("_proof_level")) and child not in PROOF_LEVELS:
                errors.append(f"invalid proof level at {child_path}: {child}")
            _validate_proof_values(child, child_path, errors, depth + 1)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            _validate_proof_values(child, f"{path}[{index}]", errors, depth + 1)


def _require_proof(value: dict[str, object], path: str, errors: list[str]) -> None:
    if value.get("apw:proof_level") not in PROOF_LEVELS:
        errors.append(f"{path} must have a valid apw:proof_level")
