from __future__ import annotations

import argparse
import glob as _glob
import hashlib
import json
import logging
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path

from daemon.common import sha256_file, sha256_prefix
from daemon.forgery_analysis.analyzer import HashChainAnalyzer
from daemon.hardware_attestation.provider import SoftwareProvider
from daemon.schema import validate_manifest_invariants
from daemon.signing import pinned_public_key, verify_ed25519_signature
from daemon.time_anchor.anchor import RFC3161Provider, TimeProof

log = logging.getLogger(__name__)
DEFAULT_SIGNING_KEY = Path("~/.apw/demo_signing_key.bin")
DEFAULT_TRUST_ANCHOR = Path("~/.apw/provenance/ca/root_cert.pem")

_LOCAL_SEAL_ALGORITHM = "hmac-sha256-local"

# IMPORTANT: "verified" is earned by running these, never by finding nothing
# wrong. Every "I could not check this" path leaves its name here, and the
# outcome degrades to "incomplete" instead of borrowing the strongest word the
# tool has for a document in which nothing was actually checked.
_REQUIRED_CHECKS: tuple[str, ...] = (
    "export_binding",
    "evidence_binding",
    "portable_signature",
    "c2pa_claim",
)

_CHECK_DESCRIPTIONS: dict[str, str] = {
    "export_binding": "the export file was not available, so its hard binding was never checked",
    "evidence_binding": "no bound evidence prefix could be hashed, so the coverage counters were never re-derived",
    "portable_signature": "the portable Ed25519 signature was not verified against a pinned public key",
    "c2pa_claim": "the C2PA claim was not re-verified against a locally held trust anchor",
}

# A crafted evidence file with no newline would otherwise be read to EOF in one
# allocation by `for line in handle`.
_MAX_EVIDENCE_LINE_BYTES = 1 << 20


class Severity(str, Enum):
    ERROR = "error"
    WARNING = "warning"
    INFO = "info"


@dataclass(frozen=True)
class Finding:
    severity: Severity
    code: str
    message: str


@dataclass
class VerificationResult:
    findings: list[Finding] = field(default_factory=list)
    completed_checks: set[str] = field(default_factory=set)

    def complete(self, check: str) -> None:
        self.completed_checks.add(check)

    @property
    def unchecked(self) -> list[str]:
        return [name for name in _REQUIRED_CHECKS if name not in self.completed_checks]

    @property
    def errors(self) -> list[Finding]:
        return [f for f in self.findings if f.severity == Severity.ERROR]

    @property
    def warnings(self) -> list[Finding]:
        return [f for f in self.findings if f.severity == Severity.WARNING]

    @property
    def passed(self) -> bool:
        return len(self.errors) == 0

    def error(self, code: str, message: str) -> None:
        self.findings.append(Finding(Severity.ERROR, code, message))

    def warn(self, code: str, message: str) -> None:
        self.findings.append(Finding(Severity.WARNING, code, message))

    def info(self, code: str, message: str) -> None:
        self.findings.append(Finding(Severity.INFO, code, message))

    def error_messages(self) -> list[str]:
        return [f.message for f in self.errors]

    @property
    def outcome(self) -> str:
        codes = {finding.code for finding in self.findings}
        if "not_found" in codes:
            return "not_found"
        changed_codes = {
            "export_hash_mismatch", "evidence_hash_mismatch", "evidence_truncated",
            "tampered", "signature_invalid", "portable_signature_invalid",
            "stem_commitment_mismatch", "c2pa_asset_hash_mismatch",
            "c2pa_hard_binding_broken",
        }
        if codes & changed_codes:
            return "changed"
        if self.errors or "portable_signature_missing" in codes:
            return "untrusted"
        if self.unchecked:
            return "incomplete"
        return "verified"

    def to_dict(self) -> dict[str, object]:
        return {
            "verifier": "local_audio_provenance_poc",
            "outcome": self.outcome,
            "qualified_scope": (
                "Local POC integrity outcome; not a registry, identity, rights, or "
                "authorship result from any provider."
            ),
            "passed": self.passed,
            "checks_not_run": [
                {"check": name, "reason": _CHECK_DESCRIPTIONS[name]} for name in self.unchecked
            ],
            "findings": [
                {"severity": finding.severity.value, "code": finding.code, "message": finding.message}
                for finding in self.findings
            ],
        }


def _certificate_bodies(pem: str) -> tuple[str, ...]:
    """Base64 bodies of the certificates in a PEM blob, order- and wrap-insensitive."""
    bodies: list[str] = []
    current: list[str] | None = None
    for line in pem.splitlines():
        stripped = line.strip()
        if stripped == "-----BEGIN CERTIFICATE-----":
            current = []
        elif stripped == "-----END CERTIFICATE-----":
            if current:
                bodies.append("".join(current))
            current = None
        elif current is not None:
            current.append(stripped)
    return tuple(sorted(bodies))


def _evidence_member(evidence_dir: Path, file_name: object) -> Path | None:
    """Resolve one bound evidence file inside its directory, or None.

    IMPORTANT: the names come from the untrusted manifest and are hashed before
    any signature on that manifest is checked, so an unconstrained join is an
    attacker-directed filesystem read plus a per-path hash oracle.
    """
    name = str(file_name)
    if not name or name in {".", ".."} or "/" in name or "\\" in name:
        return None
    candidate = evidence_dir / name
    if candidate.name != name:
        return None
    try:
        root = evidence_dir.resolve()
        if candidate.exists() and candidate.resolve().parent != root:
            return None
    except OSError:
        return None
    return candidate


def _bounded_jsonl_lines(handle, byte_length: int):
    """Yield whole JSONL records from the first *byte_length* bytes, line-bounded."""
    consumed = 0
    while consumed < byte_length:
        raw = handle.readline(_MAX_EVIDENCE_LINE_BYTES)
        if not raw:
            return
        consumed += len(raw)
        if consumed > byte_length:
            return
        if raw.endswith(b"\n") or len(raw) < _MAX_EVIDENCE_LINE_BYTES:
            # The second case is the file's final record, unterminated.
            yield raw
            continue
        # An overlong record: discard the rest of it rather than buffering it.
        while consumed < byte_length:
            extra = handle.readline(_MAX_EVIDENCE_LINE_BYTES)
            if not extra:
                return
            consumed += len(extra)
            if extra.endswith(b"\n"):
                break


def _safe_path(value: object) -> Path | None:
    """expanduser() raises on values like '~no_such_user/x'; untrusted manifests
    must degrade instead."""
    try:
        return Path(str(value)).expanduser()
    except (RuntimeError, ValueError):
        return None


def _check_coverage_against_evidence(
    data: dict[str, object],
    binding: dict[str, object],
    verified_prefixes: list[tuple[Path, int]],
    result: VerificationResult,
) -> None:
    """Re-derive buffer_hash counters from the verified bound evidence prefixes
    and compare them with the manifest's self-reported numbers.

    Without this, a genuinely-signed manifest whose counters diverged from its
    own bound evidence would still be certified verified.
    """
    # Nothing bound to check against; the hash checks already ran. An empty list
    # is not the same as zero events in a bound file: the latter must still catch
    # a manifest that claims a chain over evidence containing no windows.
    if not verified_prefixes:
        return

    session_id = data.get("session_id")
    analyzers: dict[tuple[str, str], HashChainAnalyzer] = {}
    recomputed_count = 0
    window_hashes: set[str] = set()
    for path, byte_length in verified_prefixes:
        try:
            with path.open("rb") as handle:
                for raw_line in _bounded_jsonl_lines(handle, byte_length):
                    stripped = raw_line.strip()
                    if not stripped:
                        continue
                    try:
                        event = json.loads(stripped)
                    except (json.JSONDecodeError, UnicodeDecodeError, RecursionError):
                        continue
                    if not isinstance(event, dict) or event.get("event_type") != "buffer_hash":
                        continue
                    event_session = event.get("capture_session_id")
                    if session_id and event_session and event_session != session_id:
                        continue
                    stream = (
                        str(event.get("plugin_instance_id", "")),
                        str(event.get("plugin_capture_session_id", "")),
                    )
                    analyzers.setdefault(stream, HashChainAnalyzer()).ingest_buffer_hash(event)
                    recomputed_count += 1
                    window_hash = event.get("window_hash")
                    if isinstance(window_hash, str):
                        window_hashes.add(window_hash)
        except OSError:
            continue

    for analyzer in analyzers.values():
        for flag in analyzer.analyze().flags:
            reporter = result.error if flag.severity >= 0.8 else result.warn
            reporter(f"evidence_{flag.name}", f"{flag.description} ({flag.evidence})")

    coverage = data.get("observation_coverage")
    counters = coverage.get("counters") if isinstance(coverage, dict) else None
    claimed = counters.get("buffer_hash_events_received") if isinstance(counters, dict) else None
    bound_chain_length = binding.get("chain_length")
    bound_window_hash = binding.get("last_window_hash")
    mismatched = False
    # Over-claiming is the fraud: asserting more coverage than the bound evidence
    # supports. Under-claiming (more events landed in the bound file after the
    # counter was snapshotted, e.g. streaming during export) is conservative and
    # honest, so it is noted, not failed.
    if isinstance(claimed, int) and claimed > recomputed_count:
        mismatched = True
        result.error(
            "coverage_counters_mismatch",
            f"Manifest reports {claimed} buffer_hash events; bound evidence contains only {recomputed_count}",
        )
    if isinstance(bound_chain_length, int) and bound_chain_length > recomputed_count:
        mismatched = True
        result.error(
            "coverage_counters_mismatch",
            f"Evidence binding reports chain_length {bound_chain_length}; bound evidence contains only {recomputed_count}",
        )
    # The committed chain root must be one of the windows actually in the bound
    # evidence, not necessarily the last (later windows may have been appended).
    if (
        isinstance(bound_window_hash, str)
        and bound_window_hash
        and bound_window_hash not in window_hashes
    ):
        mismatched = True
        result.error(
            "evidence_commitment_mismatch",
            "The bound last_window_hash does not appear in the bound evidence",
        )
    if not mismatched:
        detail = (
            f"Recomputed {recomputed_count} buffer_hash events from bound evidence; counters agree"
            if not isinstance(claimed, int) or claimed == recomputed_count
            else f"Manifest conservatively reports {claimed} of {recomputed_count} bound buffer_hash events"
        )
        result.info("coverage_counters_rederived", detail)


def _contained_artifact(manifest_path: Path, relative: object) -> Path | None:
    """Resolve a manifest-relative artifact without leaving the session tree.

    IMPORTANT: sidecar mode legitimately records ``../exports/take.aiff``, so the
    manifest directory alone is too tight a fence; its parent is the session root
    and is the widest an untrusted manifest may steer a read.
    """
    value = str(relative)
    if not value or Path(value).is_absolute():
        return None
    parts = Path(value).parts
    # At most one leading "..", so the fence does not widen with a shallow layout.
    if parts.count("..") > 1 or ".." in parts[1:]:
        return None
    candidate = _safe_path(manifest_path.parent / value)
    if candidate is None:
        return None
    try:
        resolved = candidate.resolve()
        root = manifest_path.resolve().parent.parent
    except OSError:
        return None
    return candidate if resolved == root or root in resolved.parents else None


def _check_c2pa_claim(
    data: dict[str, object],
    manifest_path: Path,
    result: VerificationResult,
    local_anchors_pem: str | None = None,
    asset_override: Path | None = None,
) -> None:
    """Re-verify the recorded C2PA claim against the signed asset on disk.

    IMPORTANT: trust is evaluated against an anchor this machine holds, never
    against ``signer.trust_anchor_pem`` inside the manifest being verified. A
    forger generates their own root, signs a fabricated asset with it, writes
    that root into the manifest, and every layer downstream reports "verified".
    """
    claim = data.get("c2pa_claim")
    if claim is None:
        result.warn("c2pa_claim_missing", "Manifest records no C2PA claim section")
        return
    if not isinstance(claim, dict):
        result.error("schema_invalid", "c2pa_claim must be an object")
        return

    status = claim.get("status")
    if status == "unavailable":
        result.warn(
            "c2pa_claim_unavailable",
            f"No C2PA claim was produced: {claim.get('reason', 'no reason recorded')}",
        )
        return

    signer = claim.get("signer") if isinstance(claim.get("signer"), dict) else {}
    if (
        signer.get("signer_identity") != "not_established"
        or signer.get("apw:proof_level") == "externally_verified"
    ):
        result.error(
            "c2pa_signer_identity_overclaimed",
            "The C2PA claim presents a self-issued signer as an established identity",
        )

    asset = claim.get("signed_asset") if isinstance(claim.get("signed_asset"), dict) else {}
    asset_path = asset_override
    if asset_path is None:
        relative = asset.get("relative_path")
        candidate = _contained_artifact(manifest_path, relative) if relative else None
        if candidate is not None and candidate.is_file():
            asset_path = candidate
    if asset_path is None or not asset_path.is_file():
        result.warn(
            "c2pa_asset_unavailable",
            "The C2PA-signed asset is unavailable, so its claim could not be re-read",
        )
        return

    try:
        actual = sha256_file(asset_path)
    except OSError:
        result.warn("c2pa_asset_unavailable", "The C2PA-signed asset could not be read")
        return
    if actual != asset.get("sha256"):
        # Not a return: the recorded digest and the real hard binding are
        # independent checks, and a tampered asset must exercise both.
        result.error(
            "c2pa_asset_hash_mismatch",
            f"The C2PA-signed asset {asset_path.name} no longer matches the digest "
            "recorded in the manifest",
        )

    recorded_anchors = signer.get("trust_anchor_pem")
    if local_anchors_pem is None:
        result.warn(
            "c2pa_trust_anchor_unavailable",
            "This machine holds no C2PA trust anchor, so the claim signer was not evaluated; "
            "an anchor inside the manifest cannot vouch for the manifest",
        )
        return
    if (
        isinstance(recorded_anchors, str)
        and "BEGIN CERTIFICATE" in recorded_anchors
        and _certificate_bodies(recorded_anchors) != _certificate_bodies(local_anchors_pem)
    ):
        result.error(
            "c2pa_trust_anchor_untrusted",
            "The manifest names a trust root this machine does not hold; the claim was "
            "signed under a certificate authority outside the configured anchor",
        )
        return
    try:
        from daemon.c2pa_engine.verifier import verify_asset
    except ImportError:
        result.warn(
            "c2pa_engine_unavailable",
            "The C2PA engine is not installed here; only the asset digest was re-checked",
        )
        return

    sidecar = claim.get("sidecar_manifest")
    sidecar_path = _contained_artifact(manifest_path, sidecar) if sidecar else None
    try:
        live = verify_asset(
            asset_path,
            str(claim.get("mime") or "audio/wav"),
            trust_anchors_pem=local_anchors_pem,
            sidecar_manifest=sidecar_path if sidecar_path and sidecar_path.is_file() else None,
        )
    except Exception as exc:
        result.error("c2pa_claim_unreadable", f"The recorded C2PA claim could not be re-read: {exc}")
        return

    recorded = claim.get("validation", {})
    recorded_state = recorded.get("state") if isinstance(recorded, dict) else None
    if recorded_state != live.state:
        result.warn(
            "c2pa_claim_state_changed",
            f"The C2PA claim now validates as {live.state}, not {recorded_state} as recorded",
        )

    if live.state == "verified":
        result.complete("c2pa_claim")
        result.info(
            "c2pa_claim_verified",
            "The C2PA claim chains to the trust anchor held by this machine and its "
            "hard binding is intact; this is not an externally verified identity",
        )
    elif live.state == "registered_but_changed":
        result.error(
            "c2pa_hard_binding_broken",
            f"The C2PA hard binding no longer covers the signed asset: {live.detail}",
        )
    elif live.state == "nothing_found":
        result.error(
            "c2pa_claim_absent",
            "The manifest records a C2PA claim, but the signed asset carries none",
        )
    else:
        result.error("c2pa_claim_untrusted", f"The C2PA claim is not trusted: {live.detail}")


def _note_supersession(manifest_path: Path, result: VerificationResult) -> None:
    """Explain an export-hash mismatch caused by a re-export, not a tamper.

    The daemon versions the manifest but the DAW overwrites the audio in place,
    leaving a complete earlier artifact set on disk that now reads as changed.
    """
    marker = manifest_path.with_name(manifest_path.stem + ".superseded.json")
    try:
        record = json.loads(marker.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, UnicodeDecodeError):
        return
    if isinstance(record, dict) and record.get("superseded_by"):
        result.info(
            "manifest_superseded",
            f"The export was re-rendered under the same file name after this manifest was "
            f"sealed; the current bytes are described by {record['superseded_by']}",
        )


def _load_trust_anchors(trust_anchor_path: Path | None) -> str | None:
    if trust_anchor_path is None:
        return None
    try:
        pem = trust_anchor_path.expanduser().read_text(encoding="ascii")
    except (OSError, UnicodeDecodeError, RuntimeError, ValueError):
        return None
    return pem if "BEGIN CERTIFICATE" in pem else None


# IMPORTANT: this daemon relays the TSA's assertion and never authenticates the
# token's CMS signature, so neither label may claim more than "inferred".
_TIME_ANCHOR_LABELS = frozenset({"inferred", "unknown_unobserved"})


def _check_time_anchor(data: dict[str, object], result: VerificationResult) -> None:
    anchor = data.get("time_anchor")
    if anchor is None:
        return
    if not isinstance(anchor, dict):
        result.error("time_anchor_invalid", "time_anchor must be an object")
        return
    status = anchor.get("status")
    if status == "unavailable":
        result.info("time_anchor_unavailable", "No external timestamp was obtained; no external time is asserted")
        return
    if status != "anchored":
        result.error("time_anchor_invalid", "time_anchor status must be anchored or unavailable")
        return
    if anchor.get("apw:proof_level") not in _TIME_ANCHOR_LABELS or anchor.get("cms_signature_verified") is not False:
        result.error(
            "time_anchor_invalid",
            "time_anchor claims stronger evidence than a relayed, unauthenticated TSA token",
        )
        return
    export = data.get("export")
    export_hash = export.get("sha256") if isinstance(export, dict) else None
    if anchor.get("data_hash") != export_hash:
        result.error("time_anchor_invalid", "time_anchor data_hash does not match the export hash")
        return
    timestamp_ms = anchor.get("timestamp_ms")
    source = anchor.get("source")
    nonce_hex = anchor.get("nonce_hex")
    token_hex = anchor.get("response_der_hex")
    if (
        not isinstance(timestamp_ms, int)
        or isinstance(timestamp_ms, bool)
        or not isinstance(source, str)
        or not isinstance(nonce_hex, str)
        or not isinstance(token_hex, str)
    ):
        result.error("time_anchor_invalid", "time_anchor is missing its token fields")
        return
    proof = TimeProof(source, timestamp_ms, nonce_hex, token_hex, None)
    if not RFC3161Provider().verify(proof, str(export_hash)):
        result.error(
            "time_anchor_invalid",
            "time_anchor token does not match its imprint, nonce, or reported time",
        )
        return
    result.info(
        "time_anchor_consistent",
        "TSA token imprint, nonce, and time are consistent; its CMS signature was not verified",
    )


def verify_manifest(
    manifest_path: Path,
    signing_key_path: Path | None = DEFAULT_SIGNING_KEY,
    public_key_path: Path | None = None,
    export_path_override: Path | None = None,
    trust_anchor_path: Path | None = DEFAULT_TRUST_ANCHOR,
    c2pa_asset_override: Path | None = None,
) -> VerificationResult:
    result = VerificationResult()
    if not manifest_path.is_file():
        result.error("not_found", f"Manifest not found: {manifest_path}")
        return result
    try:
        data = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, UnicodeDecodeError, OSError, RecursionError) as e:
        result.error("read_failed", f"Cannot read manifest: {e}")
        return result

    for error in validate_manifest_invariants(data):
        result.error("schema_invalid", error)
    if not isinstance(data, dict):
        return result

    if "apw_version" not in data:
        result.error("missing_version", "Missing apw_version field")
    if "c2pa_mapping" not in data:
        result.error("missing_c2pa", "Missing c2pa_mapping field")
    if "apw:unobserved" not in data:
        result.error("missing_unobserved", "Missing apw:unobserved field (honesty model violation)")

    export = data.get("export")
    if export is None:
        result.error("no_export", "No export evidence in manifest")
    elif not isinstance(export, dict):
        result.error("schema_invalid", "export must be an object")
    else:
        if not export.get("sha256"):
            result.error("export_no_hash", "Export missing sha256 hash")
        if not export.get("file_name"):
            result.error("export_no_name", "Export missing file_name")
        export_path_value = export_path_override or export.get("file_path")
        if export_path_value:
            export_path = _safe_path(export_path_value)
            if export_path is not None and export_path.is_file():
                try:
                    actual_export_hash = sha256_file(export_path)
                except OSError:
                    result.warn("export_file_unavailable", "Export file could not be read for hash verification")
                else:
                    result.complete("export_binding")
                    if actual_export_hash != export.get("sha256"):
                        result.error("export_hash_mismatch", "Export file SHA-256 does not match manifest")
                        _note_supersession(manifest_path, result)
                    else:
                        result.info("export_hash_valid", "Export file SHA-256 matches manifest")
            else:
                result.warn(
                    "export_file_unavailable",
                    f"Export file is unavailable for hash verification: {export_path_value}. "
                    "Pass --export <file> to verify the manifest against a copy you hold.",
                )

    stems = data.get("observed_stems", [])
    if not isinstance(stems, list):
        stems = []
    if not stems:
        result.info(
            "no_routed_evidence",
            "No routed-audio evidence was received; export integrity can still be checked, but association is unobserved",
        )
    for i, stem in enumerate(stems):
        if not isinstance(stem, dict):
            continue
        if not stem.get("hash_chain_root"):
            result.error("stem_no_root", f"Stem {i} missing hash_chain_root")
        if stem.get("hash_chain_length", 0) == 0:
            result.error("stem_empty_chain", f"Stem {i} has zero-length hash chain")
        if not stem.get("source_category_proof_level"):
            result.warn("source_proof_missing", f"Stem {i} source category has no proof level")

    mapping = data.get("c2pa_mapping")
    assertions = mapping.get("assertions") if isinstance(mapping, dict) else []
    if not isinstance(assertions, list):
        assertions = []
    labels = [a.get("label") for a in assertions if isinstance(a, dict)]
    if "apw.unobserved" not in labels:
        result.error("c2pa_no_unobserved", "C2PA assertions missing apw.unobserved declaration")

    if "evidence_binding" not in data:
        result.warn("no_evidence_binding", "No evidence_binding (cannot trace manifest to evidence files)")
    elif not isinstance(data["evidence_binding"], dict):
        result.error("schema_invalid", "evidence_binding must be an object")
    else:
        binding = data["evidence_binding"]
        if not binding.get("evidence_file_hashes"):
            result.warn("empty_evidence_hashes", "Evidence binding has no file hashes")
        if binding.get("chain_length", 0) == 0:
            result.warn("binding_no_chain", "Evidence binding reports zero chain length")
        last_window_hash = binding.get("last_window_hash")
        for i, stem in enumerate(stems):
            if not isinstance(stem, dict):
                continue
            if last_window_hash and stem.get("hash_chain_root") != last_window_hash:
                result.error(
                    "stem_commitment_mismatch",
                    f"Stem {i} chain commitment does not match the last bound window hash",
                )

        evidence_dir_value = binding.get("evidence_directory")
        evidence_files = binding.get("evidence_files", {})
        evidence_hashes = binding.get("evidence_file_hashes", {})
        verified_prefixes: list[tuple[Path, int]] = []
        evidence_dir = _safe_path(evidence_dir_value) if evidence_dir_value else None
        if evidence_dir_value and evidence_dir is None:
            result.warn("path_unresolvable", "Evidence directory path cannot be resolved")
        if evidence_dir is not None and isinstance(evidence_files, dict) and evidence_files:
            for file_name, file_binding in evidence_files.items():
                evidence_path = _evidence_member(evidence_dir, file_name)
                if evidence_path is None:
                    result.error(
                        "evidence_binding_invalid",
                        f"Evidence binding names a file outside its bound directory: {file_name}",
                    )
                    continue
                if not isinstance(file_binding, dict):
                    result.error("evidence_binding_invalid", f"Invalid evidence binding: {file_name}")
                    continue
                try:
                    byte_length = int(file_binding.get("byte_length", 0))
                except (TypeError, ValueError, OverflowError):
                    result.error(
                        "evidence_binding_invalid",
                        f"Invalid evidence binding: {file_name} (byte_length is not an integer)",
                    )
                    continue
                if byte_length < 0:
                    result.error(
                        "evidence_binding_invalid",
                        f"Invalid evidence binding: {file_name} (byte_length is negative)",
                    )
                    continue
                expected_hash = file_binding.get("sha256")
                candidates = [evidence_path]
                stem_parts = evidence_path.stem.rsplit(".", 1)
                rotation_stem = (
                    stem_parts[0] if len(stem_parts) == 2 and stem_parts[1].isdigit()
                    else evidence_path.stem
                )
                candidates.extend(sorted(evidence_dir.glob(
                    f"{_glob.escape(rotation_stem)}.*{_glob.escape(evidence_path.suffix)}"
                )))
                matched_path = None
                candidate_sizes: list[int] = []
                for candidate in candidates:
                    if not candidate.is_file():
                        continue
                    try:
                        size = candidate.stat().st_size
                    except OSError:
                        continue
                    candidate_sizes.append(size)
                    if matched_path is not None or size < byte_length:
                        continue
                    try:
                        if sha256_prefix(candidate, byte_length) == expected_hash:
                            matched_path = candidate
                    except (OSError, EOFError):
                        continue
                if matched_path is not None:
                    suffix = "" if matched_path == evidence_path else f" (now in rotation {matched_path.name})"
                    result.info("evidence_hash_valid", f"Evidence prefix hash matches: {file_name}{suffix}")
                    verified_prefixes.append((matched_path, byte_length))
                    result.complete("evidence_binding")
                elif not candidate_sizes:
                    result.warn("evidence_file_unavailable", f"Evidence file unavailable: {file_name}")
                elif all(size < byte_length for size in candidate_sizes):
                    result.error("evidence_truncated", f"Evidence prefix is unavailable or truncated: {file_name}")
                else:
                    result.error("evidence_hash_mismatch", f"Evidence file changed: {file_name}")
            _check_coverage_against_evidence(data, binding, verified_prefixes, result)
        elif evidence_dir is not None and isinstance(evidence_hashes, dict):
            for file_name, expected_hash in evidence_hashes.items():
                evidence_path = _evidence_member(evidence_dir, file_name)
                if evidence_path is None:
                    result.error(
                        "evidence_binding_invalid",
                        f"Evidence binding names a file outside its bound directory: {file_name}",
                    )
                    continue
                if not evidence_path.is_file():
                    result.warn("evidence_file_unavailable", f"Evidence file unavailable: {file_name}")
                    continue
                try:
                    actual_hash = sha256_file(evidence_path)
                except OSError:
                    result.warn("evidence_file_unavailable", f"Evidence file unreadable: {file_name}")
                    continue
                if actual_hash != expected_hash:
                    result.error("evidence_hash_mismatch", f"Evidence file changed: {file_name}")
                else:
                    result.info("evidence_hash_valid", f"Evidence file hash matches: {file_name}")
                    result.complete("evidence_binding")

    sig = data.get("manifest_signature")
    if sig is None:
        result.warn("unsigned", "Manifest is unsigned (no manifest_signature)")
    elif not isinstance(sig, dict):
        result.error("schema_invalid", "manifest_signature must be an object")
    elif not sig.get("signed_content_hash"):
        # A present-but-hollow seal used to be quieter than a deleted one: the
        # old elif chain simply ran out, so a forger's best move was to keep the
        # key and empty it.
        result.error(
            "signature_incomplete",
            "manifest_signature carries no signed_content_hash, so nothing binds it to this manifest",
        )
    else:
        manifest_copy = {k: v for k, v in data.items() if k != "manifest_signature"}
        signed_bytes = json.dumps(
            manifest_copy, sort_keys=True, separators=(",", ":")
        ).encode()
        recomputed = hashlib.sha256(signed_bytes).hexdigest()
        if recomputed != sig["signed_content_hash"]:
            result.error("tampered", "Manifest content hash mismatch (manifest may have been tampered with)")
        else:
            result.info("signed_content_hash_valid", "Manifest content matches its signed-content hash")
        # IMPORTANT: keyed off the signature algorithm, never off trust_scope.
        # trust_scope is an unconstrained string inside the sealed document, so
        # a forger who edits it picks whether their own seal gets checked.
        if str(sig.get("algorithm")) != _LOCAL_SEAL_ALGORITHM:
            result.error(
                "signature_algorithm_unrecognised",
                f"manifest_signature declares algorithm {sig.get('algorithm')!r}, which this "
                "verifier cannot check; an unverifiable seal is not an accepted one",
            )
        elif signing_key_path is None:
            result.warn(
                "local_signing_key_not_supplied",
                "The local HMAC key was deliberately not supplied (--public-only), so the seal "
                "bytes were not verified; the portable Ed25519 signature still covers this manifest",
            )
        else:
            local_key = signing_key_path.expanduser()
            if not local_key.is_file():
                result.warn(
                    "local_signing_key_unavailable",
                    f"Local HMAC key is unavailable at {local_key}; signature bytes were not verified",
                )
            elif not sig.get("signature_hex"):
                result.error("signature_missing", "Local integrity seal has no signature bytes")
            else:
                provider = SoftwareProvider(local_key)
                identity = provider.device_identity()
                try:
                    signature_bytes = bytes.fromhex(str(sig["signature_hex"]))
                except ValueError:
                    result.error("signature_malformed", "Local integrity signature is not valid hex")
                else:
                    if identity.device_id != sig.get("device_id"):
                        result.error("signer_mismatch", "Local signing key does not match manifest signer")
                    elif not provider.verify(signed_bytes, signature_bytes):
                        result.error("signature_invalid", "Local HMAC integrity seal is invalid")
                    else:
                        result.info("local_signature_valid", "Local HMAC integrity seal verified")
        result.warn(
            "local_integrity_only",
            "Local HMAC integrity is not hardware attestation or external identity",
        )

    portable = data.get("portable_signature")
    if not isinstance(portable, dict):
        result.warn(
            "portable_signature_missing",
            "No portable Ed25519 signature is present; public-key integrity is untrusted",
        )
    elif pinned_public_key(public_key_path)[0] is None:
        # IMPORTANT: unverifiable is not tampered. portable_signature_invalid is a
        # "changed" code, so treating a missing pin as a failure accused a
        # recipient's untouched manifest of having moved.
        result.warn(
            "portable_key_unavailable",
            f"No pinned Ed25519 public key at {pinned_public_key(public_key_path)[1]}, so the "
            "portable signature was not verified. Pass --public-key <the producer's public key "
            "file> to check it.",
        )
    else:
        unsigned = {
            key: value
            for key, value in data.items()
            if key not in {"portable_signature", "manifest_signature"}
        }
        try:
            valid, message = verify_ed25519_signature(unsigned, portable, public_key_path)
        except (OSError, ValueError, ImportError) as exc:
            result.error("portable_signature_invalid", f"Ed25519 verification failed: {exc}")
        else:
            if valid:
                result.complete("portable_signature")
                result.info("portable_signature_valid", message)
                result.warn(
                    "signer_identity_unverified",
                    "Signature validity does not establish the signer’s externally verified identity",
                )
            else:
                result.error("portable_signature_invalid", message)

    _check_c2pa_claim(
        data,
        manifest_path,
        result,
        _load_trust_anchors(trust_anchor_path),
        c2pa_asset_override,
    )

    _check_time_anchor(data, result)

    facts = data.get("session_facts")
    if isinstance(facts, dict):
        tracks = facts.get("tracks")
        track_count = len(tracks) if isinstance(tracks, list) else 0
        result.info("session_facts", f"Session facts present: {track_count} tracks, BPM {facts.get('bpm')}")
    elif facts is not None:
        result.error("schema_invalid", "session_facts must be an object")
    else:
        result.warn("no_session_facts", "No session_facts (no .als project data)")

    presentation = data.get("presentation", {})
    if isinstance(presentation, dict) and presentation.get("html_report"):
        report_path = manifest_path.parent / str(presentation["html_report"])
        if report_path.is_file():
            result.info("html_report_present", f"HTML fight card present: {report_path.name}")
        else:
            result.warn("html_report_missing", f"HTML fight card is missing: {report_path.name}")

    return result


def verify_hash_chain(evidence_path: Path) -> VerificationResult:
    result = VerificationResult()
    analyzer = HashChainAnalyzer()

    try:
        lines = evidence_path.open("r", encoding="utf-8")
    except OSError as e:
        result.error("read_failed", f"Cannot read evidence: {e}")
        return result

    count = 0
    with lines:
        try:
            for line_num, line in enumerate(lines, 1):
                if not line.strip():
                    continue
                try:
                    event = json.loads(line)
                except (json.JSONDecodeError, RecursionError):
                    result.error("malformed_json", f"Malformed JSON at line {line_num}")
                    continue
                if not isinstance(event, dict):
                    result.error("malformed_json", f"Malformed JSON at line {line_num}: not an object")
                    continue
                if event.get("event_type") == "buffer_hash":
                    analyzer.ingest_buffer_hash(event)
                    count += 1
        except UnicodeDecodeError:
            result.error("read_failed", "Evidence file is not valid UTF-8")
            return result

    if count == 0:
        result.error("no_hashes", "No buffer_hash events found in evidence file")
        return result

    report = analyzer.analyze()
    for flag in report.flags:
        if flag.severity >= 0.8:
            result.error(flag.name, f"{flag.description} ({flag.evidence})")
        else:
            result.warn(flag.name, f"{flag.description} ({flag.evidence})")

    if not report.flags:
        result.info("chain_intact", f"Hash chain verified: {count} windows, no breaks")

    return result


def main(argv: list[str] | None = None) -> int:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")
    parser = argparse.ArgumentParser(description="Verify audio provenance manifest and hash chain integrity.")
    parser.add_argument("target", type=Path, help="Path to manifest JSON or evidence JSONL file.")
    parser.add_argument(
        "--signing-key",
        type=Path,
        default=DEFAULT_SIGNING_KEY,
        help="Local HMAC key used for same-machine integrity verification.",
    )
    parser.add_argument(
        "--public-key",
        type=Path,
        default=None,
        help="Optional raw 32-byte Ed25519 public key; embedded public key is used by default.",
    )
    parser.add_argument(
        "--public-only",
        action="store_true",
        help="Do not use the local HMAC compatibility key.",
    )
    parser.add_argument(
        "--export",
        type=Path,
        default=None,
        help="Verify the manifest export hash against this file instead of its recorded path.",
    )
    parser.add_argument(
        "--trust-anchor",
        type=Path,
        default=DEFAULT_TRUST_ANCHOR,
        help=(
            "PEM trust anchor this machine holds. The C2PA claim is evaluated against it, "
            "never against the anchor recorded inside the manifest."
        ),
    )
    parser.add_argument(
        "--c2pa-asset",
        type=Path,
        default=None,
        help="Re-read the C2PA claim from this file instead of the recorded signed asset.",
    )
    args = parser.parse_args(argv)

    path = args.target
    if not path.exists():
        log.error("OUTCOME: not_found (local POC verifier): %s", path)
        return 1

    # IMPORTANT: recipients run this on files from untrusted sources; any input
    # must produce a clean outcome, never a traceback.
    try:
        if path.suffix == ".json":
            log.info("Verifying manifest: %s", path)
            result = verify_manifest(
                path,
                None if args.public_only else args.signing_key,
                args.public_key,
                args.export,
                args.trust_anchor,
                args.c2pa_asset,
            )
        elif path.suffix == ".jsonl":
            log.info("Verifying hash chain: %s", path)
            result = verify_hash_chain(path)
        else:
            log.info("Attempting both manifest and hash chain verification")
            result = verify_manifest(path, args.signing_key, trust_anchor_path=args.trust_anchor)
            chain_result = verify_hash_chain(path)
            result.findings.extend(chain_result.findings)
    except Exception:
        log.exception("Verifier internal error")
        result = VerificationResult()
        result.error(
            "verifier_error",
            "Verifier could not process this input; treating it as untrusted",
        )

    for f in result.findings:
        if f.severity == Severity.ERROR:
            log.error("FAIL: [%s] %s", f.code, f.message)
        elif f.severity == Severity.WARNING:
            log.warning("WARN: [%s] %s", f.code, f.message)
        else:
            log.info("OK:   [%s] %s", f.code, f.message)

    for name in result.unchecked:
        log.warning("NOT CHECKED: [%s] %s", name, _CHECK_DESCRIPTIONS[name])

    log.info(
        "OUTCOME: %s (local POC verifier; not a registry or identity result)",
        result.outcome,
    )

    if result.outcome == "verified":
        log.info("PASS: %d checks, %d warnings", len(result.findings), len(result.warnings))
        return 0
    if result.outcome == "incomplete":
        log.error(
            "INCOMPLETE: nothing contradicts this manifest, but %d of %d load-bearing checks "
            "could not run, so it is not verified",
            len(result.unchecked),
            len(_REQUIRED_CHECKS),
        )
        return 1
    log.error(
        "FAIL: outcome=%s, %d errors, %d warnings",
        result.outcome,
        len(result.errors),
        len(result.warnings),
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
