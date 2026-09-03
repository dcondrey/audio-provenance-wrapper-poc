from __future__ import annotations

import hashlib
import json
import os
import zipfile
from dataclasses import dataclass
from pathlib import Path

from daemon.common import canonical_json_bytes, sha256_prefix
from daemon.signing import Ed25519Signer, pinned_public_key, verify_ed25519_signature


@dataclass(frozen=True)
class BundleMember:
    archive_path: str
    source_path: Path
    byte_length: int
    sha256: str
    source_kind: str


def _member(
    archive_path: str,
    source_path: Path,
    source_kind: str,
    byte_length: int | None = None,
    expected_sha256: str | None = None,
) -> BundleMember:
    size = source_path.stat().st_size if byte_length is None else byte_length
    if source_path.stat().st_size < size:
        raise EOFError(f"{source_path} is shorter than the requested bundle prefix")
    digest = sha256_prefix(source_path, size)
    if expected_sha256 is not None and digest != expected_sha256:
        raise ValueError(f"bundle source hash changed before packaging: {source_path}")
    return BundleMember(archive_path, source_path, size, digest, source_kind)


def _archive_path(path: Path, base: Path, fallback_dir: str) -> str:
    """Mirror the producer's manifest directory inside the archive.

    IMPORTANT: the manifest resolves presentation.html_report, verifier_result,
    downstream_handoff and c2pa_claim.signed_asset.relative_path against its own
    directory, so a member filed under an invented folder is a dead link for the
    recipient the bundle exists to serve.
    """
    try:
        return path.resolve().relative_to(base).as_posix()
    except ValueError:
        return f"{fallback_dir}/{path.name}"


def _bound_evidence_source(evidence_dir: Path, file_name: str, byte_length: int, digest: str) -> Path:
    direct = evidence_dir / file_name
    candidates = [direct]
    candidates.extend(sorted(evidence_dir.glob(f"{direct.stem}.*{direct.suffix}")))
    for candidate in candidates:
        if (
            candidate.is_file()
            and candidate.stat().st_size >= byte_length
            and sha256_prefix(candidate, byte_length) == digest
        ):
            return candidate
    raise FileNotFoundError(f"bound evidence prefix is unavailable for bundle: {file_name}")


def create_evidence_bundle(
    *,
    manifest_path: Path,
    report_path: Path,
    verification_path: Path,
    handoff_path: Path,
    index_path: Path,
    bundle_path: Path,
    signer: Ed25519Signer,
) -> tuple[Path, Path]:
    """Create a deterministic ZIP plus a separately signed canonical payload index."""
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    export = manifest.get("export") if isinstance(manifest.get("export"), dict) else {}
    binding = (
        manifest.get("evidence_binding")
        if isinstance(manifest.get("evidence_binding"), dict)
        else {}
    )
    base = manifest_path.resolve().parent
    members = [
        _member(manifest_path.name, manifest_path, "manifest"),
        _member(_archive_path(report_path, base, "presentation"), report_path, "fight_card"),
        _member(
            _archive_path(verification_path, base, "verification"),
            verification_path,
            "local_poc_verifier_result",
        ),
        _member(_archive_path(handoff_path, base, "handoff"), handoff_path, "downstream_handoff"),
    ]

    export_path = Path(str(export.get("file_path", ""))).expanduser()
    if export_path.is_file():
        members.append(_member(f"export/{export_path.name}", export_path, "export_audio"))

    claim = manifest.get("c2pa_claim") if isinstance(manifest.get("c2pa_claim"), dict) else {}
    signed_asset = claim.get("signed_asset") if isinstance(claim.get("signed_asset"), dict) else {}
    for relative, kind in (
        (signed_asset.get("relative_path"), "c2pa_signed_asset"),
        (claim.get("sidecar_manifest"), "c2pa_sidecar_manifest"),
    ):
        if not isinstance(relative, str) or not relative:
            continue
        source = (base / relative).resolve()
        if not source.is_file():
            continue
        archive_path = _archive_path(source, base, "c2pa")
        if any(member.archive_path == archive_path for member in members):
            continue
        members.append(_member(archive_path, source, kind))

    evidence_dir = Path(str(binding.get("evidence_directory", ""))).expanduser()
    evidence_files = binding.get("evidence_files", {})
    if isinstance(evidence_files, dict):
        for file_name, raw_binding in sorted(evidence_files.items()):
            if not isinstance(raw_binding, dict):
                continue
            byte_length = int(raw_binding.get("byte_length", 0))
            digest = str(raw_binding.get("sha256", ""))
            source = _bound_evidence_source(evidence_dir, str(file_name), byte_length, digest)
            members.append(_member(
                f"evidence/{file_name}",
                source,
                "immutable_evidence_prefix",
                byte_length,
                digest,
            ))

    members = sorted(members, key=lambda item: item.archive_path)
    entries = [
        {
            "archive_path": item.archive_path,
            "sha256": item.sha256,
            "byte_length": item.byte_length,
            "source_kind": item.source_kind,
        }
        for item in members
    ]
    bundle_id = hashlib.sha256(canonical_json_bytes(entries)).hexdigest()[:24]
    index: dict[str, object] = {
        "bundle_format": "apw-evidence-bundle-v1",
        "bundle_id": bundle_id,
        "capture_session_id": manifest.get("session_id"),
        "created_at": manifest.get("created_at"),
        "archive_order": "lexicographic_by_archive_path",
        "archive_metadata": "fixed_1980_utc_timestamp_and_0644_mode",
        "self_reference_policy": (
            "bundle-index.json is the signed trust root inside the archive and is intentionally not self-hashed; "
            "every other archive entry is enumerated below."
        ),
        "entries": entries,
        "signature_scope": "canonical index fields excluding portable_signature",
        "signer_identity": "not_established",
        "signer_identity_proof_level": "unknown_unobserved",
        "apw:proof_level": "directly_observed",
    }
    index["portable_signature"] = signer.sign_manifest(index)
    index_bytes = json.dumps(index, indent=2, ensure_ascii=False, sort_keys=True).encode("utf-8") + b"\n"

    # IMPORTANT: publish atomically. Writing the ZIP in place would truncate a
    # previously good bundle, and a failure while writing the adjacent index
    # would orphan a signed index no archive backs. Stage both to .part files,
    # then os.replace the bundle first and the index second, so a crash between
    # the two renames leaves new-bundle + old-index (which the verifier rejects
    # fail-closed), never a signed index without its archive.
    bundle_path.parent.mkdir(parents=True, exist_ok=True)
    index_path.parent.mkdir(parents=True, exist_ok=True)
    bundle_part = bundle_path.with_name(bundle_path.name + ".part")
    index_part = index_path.with_name(index_path.name + ".part")
    try:
        with zipfile.ZipFile(
            bundle_part,
            "w",
            compression=zipfile.ZIP_DEFLATED,
            compresslevel=9,
            strict_timestamps=True,
        ) as archive:
            write_items: list[tuple[str, Path | None, int, str | None, bytes | None]] = [
                (item.archive_path, item.source_path, item.byte_length, item.sha256, None)
                for item in members
            ]
            write_items.append(("bundle-index.json", None, len(index_bytes), None, index_bytes))
            for archive_path, source_path, byte_length, expected_sha256, inline in sorted(write_items):
                info = zipfile.ZipInfo(archive_path, date_time=(1980, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                info.create_system = 3
                info.external_attr = (0o100644 & 0xFFFF) << 16
                with archive.open(info, "w", force_zip64=True) as destination:
                    if inline is not None:
                        destination.write(inline)
                        continue
                    assert source_path is not None
                    # IMPORTANT: constraint 3 (the signed index covers every entry).
                    # Hash the bytes actually archived; a re-export landing between
                    # the index hash and this copy must fail the bundle, not ship a
                    # signed digest that does not match its own archive.
                    digest = hashlib.sha256()
                    remaining = byte_length
                    with source_path.open("rb") as source:
                        while remaining:
                            chunk = source.read(min(1024 * 1024, remaining))
                            if not chunk:
                                raise EOFError(f"{source_path} changed while creating the bundle")
                            digest.update(chunk)
                            destination.write(chunk)
                            remaining -= len(chunk)
                    if digest.hexdigest() != expected_sha256:
                        raise ValueError(
                            f"{source_path} changed while creating the bundle; "
                            "archived bytes do not match the signed index"
                        )
        index_part.write_bytes(index_bytes)
    except BaseException:
        # Discard only our own staging; a previously published good pair is
        # untouched because we never wrote to their real paths.
        bundle_part.unlink(missing_ok=True)
        index_part.unlink(missing_ok=True)
        raise
    os.replace(bundle_part, bundle_path)
    os.replace(index_part, index_path)
    return index_path, bundle_path


def verify_evidence_bundle(
    index_path: Path,
    bundle_path: Path,
    public_key_path: Path | None = None,
) -> list[str]:
    """Return integrity errors for the signed index and deterministic archive payload.

    IMPORTANT: the index signature is checked against a locally pinned Ed25519 public
    key. Without a pin, anyone could re-sign an edited index with a key of their own.
    """
    errors: list[str] = []
    try:
        index_bytes = index_path.read_bytes()
        index = json.loads(index_bytes)
    except (OSError, json.JSONDecodeError) as exc:
        return [f"bundle index could not be read: {exc}"]
    if not isinstance(index, dict):
        return ["bundle index must be a JSON object"]
    signature = index.get("portable_signature")
    pinned, pinned_path = pinned_public_key(public_key_path)
    if not isinstance(signature, dict):
        errors.append("bundle index portable signature is missing")
    elif pinned is None:
        errors.append(
            f"bundle index signature was not checked: no pinned Ed25519 public key at "
            f"{pinned_path}; supply the producer's public key to verify it"
        )
    else:
        unsigned = {key: value for key, value in index.items() if key != "portable_signature"}
        valid, message = verify_ed25519_signature(unsigned, signature, public_key_path)
        if not valid:
            errors.append(message)
    try:
        archive = zipfile.ZipFile(bundle_path, "r")
    except (OSError, zipfile.BadZipFile) as exc:
        return errors + [f"evidence bundle could not be read: {exc}"]
    with archive:
        expected_names = {"bundle-index.json"}
        entries = index.get("entries", [])
        if not isinstance(entries, list):
            return errors + ["bundle index entries must be an array"]
        for entry in entries:
            if not isinstance(entry, dict):
                errors.append("bundle index contains an invalid entry")
                continue
            archive_path = str(entry.get("archive_path", ""))
            expected_names.add(archive_path)
            digest = hashlib.sha256()
            size = 0
            try:
                source = archive.open(archive_path, "r")
            except KeyError:
                errors.append(f"bundle entry is missing: {archive_path}")
                continue
            with source:
                for chunk in iter(lambda: source.read(1024 * 1024), b""):
                    digest.update(chunk)
                    size += len(chunk)
            if digest.hexdigest() != entry.get("sha256"):
                errors.append(f"bundle entry hash mismatch: {archive_path}")
            if size != entry.get("byte_length"):
                errors.append(f"bundle entry size mismatch: {archive_path}")
        if set(archive.namelist()) != expected_names:
            errors.append("bundle archive entries do not exactly match the signed index")
        if archive.read("bundle-index.json") != index_bytes:
            errors.append("adjacent and archived bundle indexes differ")
    return errors
