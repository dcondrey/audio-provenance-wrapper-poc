from __future__ import annotations

import argparse
import hashlib
import json
import logging
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path

from daemon.common import canonical_json_bytes, sha256_file, sha256_prefix
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
    except (json.JSONDecodeError, OSError) as e:
        result.error("read_failed", f"Cannot read manifest: {e}")
        return result

    for error in validate_manifest_invariants(data):
        result.error("schema_invalid", error)

    if "apw_version" not in data:
        result.error("missing_version", "Missing apw_version field")
    if "c2pa_mapping" not in data:
        result.error("missing_c2pa", "Missing c2pa_mapping field")
    if "apw:unobserved" not in data:
        result.error("missing_unobserved", "Missing apw:unobserved field (honesty model violation)")

    export = data.get("export")
    if export is None:
        result.error("no_export", "No export evidence in manifest")
    else:
        if not export.get("sha256"):
            result.error("export_no_hash", "Export missing sha256 hash")
        if not export.get("file_name"):
            result.error("export_no_name", "Export missing file_name")
        export_path_value = export_path_override or export.get("file_path")
        if export_path_value:
            export_path = Path(str(export_path_value)).expanduser()
            if export_path.is_file():
                actual_export_hash = sha256_file(export_path)
                if actual_export_hash != export.get("sha256"):
                    result.error("export_hash_mismatch", "Export file SHA-256 does not match manifest")
                else:
                    result.info("export_hash_valid", "Export file SHA-256 matches manifest")
            else:
                result.warn("export_file_unavailable", "Export file is unavailable for hash verification")

    stems = data.get("observed_stems", [])
    if not stems:
        result.info(
            "no_routed_evidence",
            "No routed-audio evidence was received; export integrity can still be checked, but association is unobserved",
        )
    for i, stem in enumerate(stems):
        if not stem.get("hash_chain_root"):
            result.error("stem_no_root", f"Stem {i} missing hash_chain_root")
        if stem.get("hash_chain_length", 0) == 0:
            result.error("stem_empty_chain", f"Stem {i} has zero-length hash chain")
        if not stem.get("source_category_proof_level"):
            result.warn("source_proof_missing", f"Stem {i} source category has no proof level")

    assertions = data.get("c2pa_mapping", {}).get("assertions", [])
    labels = [a.get("label") for a in assertions]
    if "apw.unobserved" not in labels:
        result.error("c2pa_no_unobserved", "C2PA assertions missing apw.unobserved declaration")

    if "evidence_binding" not in data:
        result.warn("no_evidence_binding", "No evidence_binding (cannot trace manifest to evidence files)")
    else:
        binding = data["evidence_binding"]
        if not binding.get("evidence_file_hashes"):
            result.warn("empty_evidence_hashes", "Evidence binding has no file hashes")
        if binding.get("chain_length", 0) == 0:
            result.warn("binding_no_chain", "Evidence binding reports zero chain length")
        last_window_hash = binding.get("last_window_hash")
        for i, stem in enumerate(stems):
            if last_window_hash and stem.get("hash_chain_root") != last_window_hash:
                result.error(
                    "stem_commitment_mismatch",
                    f"Stem {i} chain commitment does not match the last bound window hash",
                )

        evidence_dir_value = binding.get("evidence_directory")
        evidence_files = binding.get("evidence_files", {})
        evidence_hashes = binding.get("evidence_file_hashes", {})
        if evidence_dir_value and isinstance(evidence_files, dict) and evidence_files:
            evidence_dir = Path(str(evidence_dir_value)).expanduser()
            for file_name, file_binding in evidence_files.items():
                evidence_path = evidence_dir / str(file_name)
                if not isinstance(file_binding, dict):
                    result.error("evidence_binding_invalid", f"Invalid evidence binding: {file_name}")
                    continue
                byte_length = int(file_binding.get("byte_length", 0))
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
                matched_path = next((
                    candidate
                    for candidate in candidates
                    if candidate.is_file()
                    and candidate.stat().st_size >= byte_length
                    and sha256_prefix(candidate, byte_length) == expected_hash
                ), None)
                if matched_path is not None:
                    suffix = "" if matched_path == evidence_path else f" (now in rotation {matched_path.name})"
                    result.info("evidence_hash_valid", f"Evidence prefix hash matches: {file_name}{suffix}")
                elif not any(candidate.is_file() for candidate in candidates):
                    result.warn("evidence_file_unavailable", f"Evidence file unavailable: {file_name}")
                elif all(candidate.stat().st_size < byte_length for candidate in candidates if candidate.is_file()):
                    result.error("evidence_truncated", f"Evidence prefix is unavailable or truncated: {file_name}")
                else:
                    result.error("evidence_hash_mismatch", f"Evidence file changed: {file_name}")
        elif evidence_dir_value and isinstance(evidence_hashes, dict):
            evidence_dir = Path(str(evidence_dir_value)).expanduser()
            for file_name, expected_hash in evidence_hashes.items():
                evidence_path = evidence_dir / str(file_name)
                if not evidence_path.is_file():
                    result.warn("evidence_file_unavailable", f"Evidence file unavailable: {file_name}")
                    continue
                actual_hash = sha256_file(evidence_path)
                if actual_hash != expected_hash:
                    result.error("evidence_hash_mismatch", f"Evidence file changed: {file_name}")
                else:
                    result.info("evidence_hash_valid", f"Evidence file hash matches: {file_name}")

    sig = data.get("manifest_signature")
    if sig is None:
        result.warn("unsigned", "Manifest is unsigned (no manifest_signature)")
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

    if "session_facts" in data:
        facts = data["session_facts"]
        track_count = len(facts.get("tracks", []))
        result.info("session_facts", f"Session facts present: {track_count} tracks, BPM {facts.get('bpm')}")
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
        for line_num, line in enumerate(lines, 1):
            if not line.strip():
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                result.error("malformed_json", f"Malformed JSON at line {line_num}")
                continue
            if event.get("event_type") == "buffer_hash":
                analyzer.ingest_buffer_hash(event)
                count += 1

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
