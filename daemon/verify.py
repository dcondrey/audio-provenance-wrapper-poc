from __future__ import annotations

import argparse
import hashlib
import json
import logging
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path

from daemon.forgery_analysis.analyzer import HashChainAnalyzer
from daemon.hardware_attestation.provider import SoftwareProvider

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


def verify_manifest(
    manifest_path: Path,
    signing_key_path: Path | None = DEFAULT_SIGNING_KEY,
) -> VerificationResult:
    result = VerificationResult()
    try:
        data = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError) as e:
        result.error("read_failed", f"Cannot read manifest: {e}")
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
    else:
        if not export.get("sha256"):
            result.error("export_no_hash", "Export missing sha256 hash")
        if not export.get("file_name"):
            result.error("export_no_name", "Export missing file_name")
        export_path_value = export.get("file_path")
        if export_path_value:
            export_path = Path(str(export_path_value)).expanduser()
            if export_path.is_file():
                actual_export_hash = hashlib.sha256(export_path.read_bytes()).hexdigest()
                if actual_export_hash != export.get("sha256"):
                    result.error("export_hash_mismatch", "Export file SHA-256 does not match manifest")
                else:
                    result.info("export_hash_valid", "Export file SHA-256 matches manifest")
            else:
                result.warn("export_file_unavailable", "Export file is unavailable for hash verification")

    stems = data.get("observed_stems", [])
    if not stems:
        result.error("no_stems", "No observed stems in manifest (no audio stream evidence)")
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
                if not evidence_path.is_file():
                    result.warn("evidence_file_unavailable", f"Evidence file unavailable: {file_name}")
                    continue
                if not isinstance(file_binding, dict):
                    result.error("evidence_binding_invalid", f"Invalid evidence binding: {file_name}")
                    continue
                byte_length = int(file_binding.get("byte_length", 0))
                current_bytes = evidence_path.read_bytes()
                if len(current_bytes) < byte_length:
                    result.error("evidence_truncated", f"Evidence file was truncated: {file_name}")
                    continue
                actual_hash = hashlib.sha256(current_bytes[:byte_length]).hexdigest()
                expected_hash = file_binding.get("sha256")
                if actual_hash != expected_hash:
                    result.error("evidence_hash_mismatch", f"Evidence file changed: {file_name}")
                else:
                    result.info("evidence_hash_valid", f"Evidence prefix hash matches: {file_name}")
        elif evidence_dir_value and isinstance(evidence_hashes, dict):
            evidence_dir = Path(str(evidence_dir_value)).expanduser()
            for file_name, expected_hash in evidence_hashes.items():
                evidence_path = evidence_dir / str(file_name)
                if not evidence_path.is_file():
                    result.warn("evidence_file_unavailable", f"Evidence file unavailable: {file_name}")
                    continue
                actual_hash = hashlib.sha256(evidence_path.read_bytes()).hexdigest()
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
        lines = evidence_path.read_text(encoding="utf-8").splitlines()
    except OSError as e:
        result.error("read_failed", f"Cannot read evidence: {e}")
        return result

    count = 0
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
    args = parser.parse_args(argv)

    path = args.target
    if not path.exists():
        log.error("File not found: %s", path)
        return 1

    if path.suffix == ".json":
        log.info("Verifying manifest: %s", path)
        result = verify_manifest(path, args.signing_key)
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

    if result.passed:
        log.info("PASS: %d checks, %d warnings", len(result.findings), len(result.warnings))
        return 0
    else:
        log.error("FAIL: %d errors, %d warnings", len(result.errors), len(result.warnings))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
