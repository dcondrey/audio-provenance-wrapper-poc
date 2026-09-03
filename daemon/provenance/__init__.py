from __future__ import annotations

import logging
import os
from pathlib import Path

from daemon.provenance.local_reference import DEFAULT_STORE, LocalReferenceProvider
from daemon.provenance.provider import (
    NOTHING_FOUND_NORMATIVE_NOTE,
    ProvenanceProvider,
    RevokedKeyError,
    SigningIdentity,
    SigningMaterial,
    VerificationState,
)
from daemon.provenance.remote_adapter import RemoteProvenanceProvider

log = logging.getLogger(__name__)

PROVIDER_ENV_VAR = "APW_PROVENANCE_PROVIDER"

__all__ = [
    "NOTHING_FOUND_NORMATIVE_NOTE",
    "LocalReferenceProvider",
    "ProvenanceProvider",
    "RemoteProvenanceProvider",
    "RevokedKeyError",
    "SigningIdentity",
    "SigningMaterial",
    "VerificationState",
    "detect_provider",
]


def detect_provider(
    store_dir: Path = DEFAULT_STORE,
    provider_name: str | None = None,
) -> ProvenanceProvider:
    """Select a provenance provider.

    The local reference provider is the default. A remote provider is used only
    when explicitly requested, and its methods raise NotImplementedError until a
    remote service exists.
    """
    requested = (provider_name or os.environ.get(PROVIDER_ENV_VAR) or "local").strip().lower()

    if requested == "remote":
        log.warning(
            "Provenance provider 'remote' requested; no remote service is implemented, "
            "so every call will raise NotImplementedError with its API requirement"
        )
        return RemoteProvenanceProvider()

    if requested != "local":
        log.warning(
            "Unknown provenance provider %r requested; falling back to the local reference "
            "provider", requested,
        )

    log.info("Using the local reference provenance provider at %s", Path(store_dir).expanduser())
    return LocalReferenceProvider(store_dir=store_dir)
