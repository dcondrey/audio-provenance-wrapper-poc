#!/usr/bin/env python3
"""Regenerate ots_upstream.json with the reference python-opentimestamps package.

The package is deliberately NOT a dependency of this repository. Run this once in a
scratch environment:

    uv venv /tmp/otsvenv && uv pip install --python /tmp/otsvenv/bin/python opentimestamps
    /tmp/otsvenv/bin/python tests/fixtures/parity/generate_ots_upstream.py

For every real proof in ots/ it records what upstream makes of it: whether it parses,
its attestations, every node message, and the SHA-256 of upstream's own re-serialization.
tests/test_ots.py then checks daemon/time_anchor/ots.py against this file, so agreement
with the reference implementation is asserted without needing it installed.
"""
from __future__ import annotations

import hashlib
import importlib.metadata
import json
from pathlib import Path

from opentimestamps.core.notary import BitcoinBlockHeaderAttestation, PendingAttestation
from opentimestamps.core.serialize import BytesDeserializationContext, BytesSerializationContext
from opentimestamps.core.timestamp import DetachedTimestampFile

HERE = Path(__file__).resolve().parent
OTS = HERE / "ots"


def messages(stamp) -> list[str]:
    out = {stamp.msg.hex()}
    for child in stamp.ops.values():
        out |= set(messages(child))
    return sorted(out)


def describe(path: Path) -> dict:
    data = path.read_bytes()
    try:
        proof = DetachedTimestampFile.deserialize(BytesDeserializationContext(data))
    except Exception as error:  # noqa: BLE001 - recording upstream's verdict is the point
        return {"name": path.name, "parses": False, "error_type": type(error).__name__}
    ctx = BytesSerializationContext()
    proof.serialize(ctx)
    attestations = []
    for _msg, attestation in proof.timestamp.all_attestations():
        if isinstance(attestation, PendingAttestation):
            attestations.append({"type": "pending", "uri": attestation.uri})
        elif isinstance(attestation, BitcoinBlockHeaderAttestation):
            attestations.append({"type": "bitcoin", "height": attestation.height})
        else:
            attestations.append({"type": type(attestation).__name__})
    return {
        "name": path.name,
        "parses": True,
        "file_hash_op": proof.file_hash_op.TAG_NAME,
        "digest": proof.file_digest.hex(),
        "attestations": attestations,
        "messages": messages(proof.timestamp),
        "reserialized_sha256": hashlib.sha256(ctx.getbytes()).hexdigest(),
        "reserialized_equals_input": ctx.getbytes() == data,
    }


def main() -> None:
    files = sorted(OTS.glob("*.ots"))
    payload = {
        "produced_by": f"python-opentimestamps {importlib.metadata.version('opentimestamps')}",
        "proofs": [describe(path) for path in files],
    }
    (HERE / "ots_upstream.json").write_text(json.dumps(payload, indent=2) + "\n")
    print(f"wrote {HERE / 'ots_upstream.json'} ({len(files)} proofs)")


if __name__ == "__main__":
    main()
