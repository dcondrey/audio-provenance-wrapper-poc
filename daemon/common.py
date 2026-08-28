from __future__ import annotations

import hashlib
import json
import logging
import time
from datetime import datetime, timezone
from pathlib import Path

log = logging.getLogger(__name__)

DEFAULT_EVIDENCE_MAX_BYTES = 64 * 1024 * 1024
DEFAULT_EVIDENCE_BACKUPS = 3


def utc_timestamp(timestamp: float | None = None) -> str:
    """ISO 8601 UTC timestamp. Uses current time if *timestamp* is None."""
    if timestamp is None:
        timestamp = time.time()
    return datetime.fromtimestamp(timestamp, timezone.utc).isoformat().replace("+00:00", "Z")


def sha256_file(path: Path) -> str:
    """SHA-256 hex digest of a file, read in 1 MB chunks."""
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_prefix(path: Path, byte_length: int) -> str:
    """Hash exactly *byte_length* bytes without loading the file into memory."""
    if byte_length < 0:
        raise ValueError("byte_length must be non-negative")
    digest = hashlib.sha256()
    remaining = byte_length
    with path.open("rb") as f:
        while remaining:
            chunk = f.read(min(1024 * 1024, remaining))
            if not chunk:
                raise EOFError(f"{path} is shorter than the bound prefix")
            digest.update(chunk)
            remaining -= len(chunk)
    return digest.hexdigest()


def canonical_json_bytes(value: object) -> bytes:
    """Stable UTF-8 JSON used as the signing input for this POC."""
    return json.dumps(
        value,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
        allow_nan=False,
    ).encode("utf-8")


def rotated_evidence_paths(path: Path) -> list[Path]:
    """Return existing rotations oldest-first, followed by the active file."""
    rotations = sorted(
        path.parent.glob(f"{path.stem}.*{path.suffix}"),
        key=lambda candidate: candidate.name,
        reverse=True,
    )
    if path.exists():
        rotations.append(path)
    return rotations


def _rotate_jsonl(path: Path, backup_count: int, max_bytes: int) -> None:
    oldest = path.with_name(f"{path.stem}.{backup_count}{path.suffix}")
    if oldest.exists():
        oldest.unlink()
    for index in range(backup_count - 1, 0, -1):
        source = path.with_name(f"{path.stem}.{index}{path.suffix}")
        if source.exists():
            source.replace(path.with_name(f"{path.stem}.{index + 1}{path.suffix}"))
    if path.exists():
        path.replace(path.with_name(f"{path.stem}.1{path.suffix}"))
    log.warning(
        "Evidence limit reached; rotated %s (max=%d bytes, backups=%d)",
        path,
        max_bytes,
        backup_count,
    )


def append_jsonl(
    path: Path,
    obj: dict[str, object],
    *,
    max_bytes: int = DEFAULT_EVIDENCE_MAX_BYTES,
    backup_count: int = DEFAULT_EVIDENCE_BACKUPS,
) -> None:
    """Append one JSONL record with bounded demo-appropriate rotation."""
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = canonical_json_bytes(obj) + b"\n"
    if len(encoded) > max_bytes:
        log.error("Dropped oversize evidence event (%d bytes) for %s", len(encoded), path)
        return
    try:
        current_size = path.stat().st_size
    except FileNotFoundError:
        current_size = 0
    if current_size and current_size + len(encoded) > max_bytes:
        _rotate_jsonl(path, max(1, backup_count), max_bytes)
    with path.open("ab") as f:
        f.write(encoded)
