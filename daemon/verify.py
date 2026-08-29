from __future__ import annotations

import argparse
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
from daemon.signing import verify_ed25519_signature

log = logging.getLogger(__name__)
DEFAULT_SIGNING_KEY = Path("~/.apw/demo_signing_key.bin")


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
            "stem_commitment_mismatch",
        }
        if codes & changed_codes:
            return "changed"
        if self.errors or "portable_signature_missing" in codes:
            return "untrusted"
        return "verified"

    def to_dict(self) -> dict[str, object]:
        return {
            "verifier": "local_audio_provenance_poc",
            "outcome": self.outcome,
            "qualified_scope": (
                "Local POC integrity outcome; not a Genotone registry, identity, rights, or authorship result."
            ),
            "passed": self.passed,
            "findings": [
                {"severity": finding.severity.value, "code": finding.code, "message": finding.message}
                for finding in self.findings
            ],
        }


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
        consumed = 0
        try:
            with path.open("rb") as handle:
                for raw_line in handle:
                    if consumed + len(raw_line) > byte_length:
                        break
                    consumed += len(raw_line)
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


def verify_manifest(
    manifest_path: Path,
    signing_key_path: Path | None = DEFAULT_SIGNING_KEY,
    public_key_path: Path | None = None,
    export_path_override: Path | None = None,
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
                    if actual_export_hash != export.get("sha256"):
                        result.error("export_hash_mismatch", "Export file SHA-256 does not match manifest")
                    else:
                        result.info("export_hash_valid", "Export file SHA-256 matches manifest")
            else:
                result.warn("export_file_unavailable", "Export file is unavailable for hash verification")

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
                evidence_path = evidence_dir / str(file_name)
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
                candidates.extend(sorted(
                    evidence_dir.glob(f"{rotation_stem}.*{evidence_path.suffix}")
                ))
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
                elif not candidate_sizes:
                    result.warn("evidence_file_unavailable", f"Evidence file unavailable: {file_name}")
                elif all(size < byte_length for size in candidate_sizes):
                    result.error("evidence_truncated", f"Evidence prefix is unavailable or truncated: {file_name}")
                else:
                    result.error("evidence_hash_mismatch", f"Evidence file changed: {file_name}")
            _check_coverage_against_evidence(data, binding, verified_prefixes, result)
        elif evidence_dir is not None and isinstance(evidence_hashes, dict):
            for file_name, expected_hash in evidence_hashes.items():
                evidence_path = evidence_dir / str(file_name)
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

    sig = data.get("manifest_signature")
    if sig is None:
        result.warn("unsigned", "Manifest is unsigned (no manifest_signature)")
    elif not isinstance(sig, dict):
        result.error("schema_invalid", "manifest_signature must be an object")
    elif sig.get("signed_content_hash"):
        manifest_copy = {k: v for k, v in data.items() if k != "manifest_signature"}
        signed_bytes = json.dumps(
            manifest_copy, sort_keys=True, separators=(",", ":")
        ).encode()
        recomputed = hashlib.sha256(signed_bytes).hexdigest()
        if recomputed != sig["signed_content_hash"]:
            result.error("tampered", "Manifest content hash mismatch (manifest may have been tampered with)")
        else:
            result.info("signed_content_hash_valid", "Manifest content matches its signed-content hash")
        if sig.get("trust_scope") == "local_software_integrity":
            local_key = signing_key_path.expanduser() if signing_key_path is not None else None
            if local_key is None or not local_key.is_file():
                result.warn(
                    "local_signing_key_unavailable",
                    "Local HMAC key is unavailable; signature bytes were not verified",
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
        else:
            result.warn(
                "signature_not_independently_verified",
                "Signature bytes were not independently verified by this local verifier",
            )

    portable = data.get("portable_signature")
    if not isinstance(portable, dict):
        result.warn(
            "portable_signature_missing",
            "No portable Ed25519 signature is present; public-key integrity is untrusted",
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
                result.info("portable_signature_valid", message)
                result.warn(
                    "signer_identity_unverified",
                    "Signature validity does not establish the signer’s externally verified identity",
                )
            else:
                result.error("portable_signature_invalid", message)

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
            )
        elif path.suffix == ".jsonl":
            log.info("Verifying hash chain: %s", path)
            result = verify_hash_chain(path)
        else:
            log.info("Attempting both manifest and hash chain verification")
            result = verify_manifest(path, args.signing_key)
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

    log.info(
        "OUTCOME: %s (local POC verifier; not a registry or identity result)",
        result.outcome,
    )

    if result.outcome == "verified":
        log.info("PASS: %d checks, %d warnings", len(result.findings), len(result.warnings))
        return 0
    else:
        log.error(
            "FAIL: outcome=%s, %d errors, %d warnings",
            result.outcome,
            len(result.errors),
            len(result.warnings),
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
