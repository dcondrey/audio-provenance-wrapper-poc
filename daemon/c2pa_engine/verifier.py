from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path

from c2pa import C2paError, ContextBuilder, Reader, Settings

VERIFIED = "verified"
REGISTERED_BUT_CHANGED = "registered_but_changed"
MARK_FOUND_CLAIM_NOT_TRUSTED = "mark_found_claim_not_trusted"
NOTHING_FOUND = "nothing_found"

STATES = (VERIFIED, REGISTERED_BUT_CHANGED, MARK_FOUND_CLAIM_NOT_TRUSTED, NOTHING_FOUND)

_HASH_MISMATCH_CODES = frozenset(
    {
        "assertion.dataHash.mismatch",
        "assertion.bmffHash.mismatch",
        "assertion.boxesHash.mismatch",
    }
)
_TRUSTED_CODE = "signingCredential.trusted"


class VerificationError(RuntimeError):
    pass


@dataclass(frozen=True)
class VerificationResult:
    state: str
    validation_state: str | None
    failure_codes: tuple[str, ...] = ()
    trust_evaluated: bool = False
    manifest: dict[str, object] | None = None
    detail: str = ""
    assertion_labels: tuple[str, ...] = field(default_factory=tuple)


def _context(trust_anchors_pem: bytes | str | None):
    if trust_anchors_pem is None:
        return None
    anchors = (
        trust_anchors_pem.decode("ascii")
        if isinstance(trust_anchors_pem, (bytes, bytearray))
        else trust_anchors_pem
    )
    if "BEGIN CERTIFICATE" not in anchors:
        raise VerificationError("Trust anchors must be PEM-encoded certificates")
    settings = Settings.from_json(
        json.dumps({"trust": {"trust_anchors": anchors}, "verify": {"verify_trust": True}})
    )
    return ContextBuilder().with_settings(settings).build()


def _classify(
    state: str | None,
    failures: tuple[str, ...],
    trust_evaluated: bool,
    credential_trusted: bool,
) -> tuple[str, str]:
    """Grade the four product states.

    IMPORTANT: "registered but changed" asserts that the claim WAS trusted and only
    the bytes moved. An untrusted signer therefore outranks a hash mismatch: anyone
    can self-sign a file and alter a byte, and reporting that as a registration this
    system recognises is strictly stronger than the truth.
    """
    hash_failures = [code for code in failures if code in _HASH_MISMATCH_CODES]
    other_failures = [code for code in failures if code not in _HASH_MISMATCH_CODES]
    binding_note = (
        f"; the hard binding is also broken ({', '.join(hash_failures)})" if hash_failures else ""
    )
    if not trust_evaluated:
        return (
            MARK_FOUND_CLAIM_NOT_TRUSTED,
            "manifest present but no trust anchors were supplied, so the signer was never evaluated"
            + binding_note,
        )
    if not credential_trusted:
        return (
            MARK_FOUND_CLAIM_NOT_TRUSTED,
            "manifest present but the signing credential did not chain to a supplied anchor"
            + (f" ({', '.join(failures)})" if failures else ""),
        )
    if hash_failures:
        return (
            REGISTERED_BUT_CHANGED,
            f"the signing credential is trusted but the hard binding is broken: "
            f"{', '.join(hash_failures)}",
        )
    if other_failures or state != "Trusted":
        return (
            MARK_FOUND_CLAIM_NOT_TRUSTED,
            f"the signing credential is trusted but validation reported {state}"
            + (f" ({', '.join(other_failures)})" if other_failures else ""),
        )
    return VERIFIED, "signing credential trusted and hard binding intact"


def verify_asset(
    path: Path | str,
    mime: str,
    trust_anchors_pem: bytes | str | None = None,
    sidecar_manifest: bytes | Path | str | None = None,
) -> VerificationResult:
    path = Path(path)
    manifest_data: bytes | None = None
    if sidecar_manifest is not None:
        manifest_data = (
            bytes(sidecar_manifest)
            if isinstance(sidecar_manifest, (bytes, bytearray))
            else Path(sidecar_manifest).read_bytes()
        )
    context = _context(trust_anchors_pem)
    try:
        with path.open("rb") as handle:
            reader = (
                Reader(mime, handle, manifest_data=manifest_data, context=context)
                if context is not None
                else Reader(mime, handle, manifest_data=manifest_data)
            )
            validation_state = reader.get_validation_state()
            report = json.loads(reader.json())
    except C2paError.ManifestNotFound:
        return VerificationResult(
            state=NOTHING_FOUND,
            validation_state=None,
            detail="no C2PA manifest is embedded in the asset and no sidecar was supplied",
        )
    except C2paError as exc:
        raise VerificationError(f"Reading provenance from {path.name} failed: {exc}") from exc

    active = report.get("manifests", {}).get(report.get("active_manifest"), {})
    results = report.get("validation_results", {}).get("activeManifest", {})
    failures = tuple(
        entry["code"] for entry in results.get("failure", []) if isinstance(entry, dict) and "code" in entry
    )
    successes = {
        entry.get("code") for entry in results.get("success", []) if isinstance(entry, dict)
    }
    credential_trusted = _TRUSTED_CODE in successes
    trust_evaluated = trust_anchors_pem is not None or credential_trusted
    state, detail = _classify(validation_state, failures, trust_evaluated, credential_trusted)
    labels = tuple(
        a["label"]
        for a in active.get("assertions", [])
        if isinstance(a, dict) and isinstance(a.get("label"), str)
    )
    return VerificationResult(
        state=state,
        validation_state=validation_state,
        failure_codes=failures,
        trust_evaluated=trust_evaluated,
        manifest=active or None,
        detail=detail,
        assertion_labels=labels,
    )
