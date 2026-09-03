"""Python orchestration for the Rust capture-to-SDK adapter.

There is deliberately no second Python mapper. The Python path executes the public Rust CLI's
``capture-adapt`` command and consumes its JSON receipt, so canonicalization, signing, embedding,
record ids and verification are byte-for-byte the same implementation as a direct CLI run.
"""

from __future__ import annotations

import json
import os
import secrets
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

MAX_ADAPTER_OUTPUT_BYTES = 4 * 1024 * 1024
DEVELOPMENT_KEY_BYTES = 32


@dataclass(frozen=True)
class CaptureAdapterInvocation:
    export: Path
    capture_manifest: Path
    handoff: Path
    evidence_bundle: Path
    key: Path
    receipt: Path
    embedded_output: Path | None = None
    sidecar: Path | None = None
    evidence_bundle_sha256: str | None = None
    registry: str | None = None

    def __post_init__(self) -> None:
        if self.embedded_output is not None and self.sidecar is not None:
            raise ValueError("embedded_output and sidecar are mutually exclusive")


def ensure_development_key(path: Path) -> Path:
    """Create an ephemeral-development key once, then reuse it for crash-safe retries."""
    path = path.expanduser()
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        data = path.read_bytes()
        if len(data) != DEVELOPMENT_KEY_BYTES:
            raise ValueError(f"development key {path} must contain exactly 32 raw bytes")
        return path
    try:
        data = secrets.token_bytes(DEVELOPMENT_KEY_BYTES)
        written = os.write(descriptor, data)
        if written != len(data):
            raise OSError(f"short development-key write: {written}/{len(data)} bytes")
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    return path


def run_capture_adapter(
    invocation: CaptureAdapterInvocation,
    *,
    cli: str | Path = "audio-provenance",
    environment: Mapping[str, str] | None = None,
    timeout_seconds: float | None = None,
) -> dict[str, object]:
    """Run the Rust adapter once and return its public SDK result."""
    command = [
        str(cli),
        "--json",
    ]
    if invocation.registry:
        command.extend(["--registry", invocation.registry])
    command.extend([
        "capture-adapt",
        str(invocation.export),
        "--capture-manifest",
        str(invocation.capture_manifest),
        "--handoff",
        str(invocation.handoff),
        "--evidence-bundle",
        str(invocation.evidence_bundle),
        "--key",
        str(invocation.key),
        "--receipt",
        str(invocation.receipt),
    ])
    if invocation.embedded_output:
        command.extend(["--out", str(invocation.embedded_output)])
    if invocation.sidecar:
        command.extend(["--sidecar", str(invocation.sidecar)])
    if invocation.evidence_bundle_sha256:
        command.extend([
            "--evidence-bundle-sha256",
            invocation.evidence_bundle_sha256,
        ])

    completed = subprocess.run(
        command,
        check=False,
        capture_output=True,
        env=dict(environment) if environment is not None else None,
        timeout=timeout_seconds,
    )
    if len(completed.stdout) > MAX_ADAPTER_OUTPUT_BYTES:
        raise RuntimeError("capture adapter JSON exceeded the 4 MiB orchestration bound")
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(
            f"capture adapter exited {completed.returncode}: {detail or 'no diagnostic'}"
        )
    try:
        result = json.loads(completed.stdout)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise RuntimeError(f"capture adapter returned malformed JSON: {exc}") from exc
    if not isinstance(result, dict):
        raise RuntimeError("capture adapter JSON result must be an object")
    if result.get("development_only") is not True or result.get("identity") != "not_established":
        raise RuntimeError("capture adapter omitted its development-only/identity boundary")
    return result
