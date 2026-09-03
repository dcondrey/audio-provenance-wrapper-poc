from __future__ import annotations

import abc
import enum
from dataclasses import dataclass
from pathlib import Path


class VerificationState(str, enum.Enum):
    """The four verification outcomes a provenance provider may report.

    IMPORTANT: NOTHING_FOUND means only that this provider found neither a
    recoverable mark nor a registry record for the asset. It is NEVER evidence
    that the asset is synthetic, machine-generated, or untrustworthy. Any caller
    that renders NOTHING_FOUND as a synthetic-origin claim is misusing it.
    """

    VERIFIED = "verified"
    REGISTERED_BUT_CHANGED = "registered_but_changed"
    MARK_FOUND_CLAIM_NOT_TRUSTED = "mark_found_claim_not_trusted"
    NOTHING_FOUND = "nothing_found"


NOTHING_FOUND_NORMATIVE_NOTE = (
    "A missing mark is not proof of synthetic origin. NOTHING_FOUND records the "
    "absence of provenance data, not the presence of a generation signal."
)


@dataclass(frozen=True)
class SigningIdentity:
    """The signing identity and how strongly the provider established it."""

    key_id: str
    subject_common_name: str
    algorithm: str
    identity_evidence: str
    revoked: bool
    created_at_ms: int
    proof_level: str


@dataclass(frozen=True)
class SigningMaterial:
    """X.509 material used to sign a claim.

    certificate_chain_pem is leaf-then-issuer concatenated PEM, the ordering the
    C2PA signer requires. private_key_handle is opaque to callers: a local
    provider returns key bytes, a remote provider returns a service handle whose
    private key never leaves the vault.
    """

    key_id: str
    algorithm: str
    certificate_chain_pem: bytes
    private_key_handle: bytes
    trust_anchor_pem: bytes
    proof_level: str


class RevokedKeyError(RuntimeError):
    """Raised when signing is attempted with a revoked key."""


class ProvenanceProvider(abc.ABC):
    """Abstract interface for identity, soft binding, registry, and verification.

    Implementations:
        LocalReferenceProvider   - operational reference backed by local files
        RemoteProvenanceProvider - seam for a hosted vault/mark/registry service

    The four capability groups map onto the product surface this seam models:
    a key-custody vault (identity, issue_signing_material, sign_claim, revoke,
    signing_history), a soft binding (embed_mark, recover_mark), a manifest
    registry (register), and a verifier (verify).

    Nothing here may run on the audio thread.
    """

    @abc.abstractmethod
    def identity(self) -> dict:
        """Describe the active signing identity and how strongly it is established.

        REQUIRED: the returned mapping carries apw:proof_level. A locally
        generated identity is never stronger than 'user_declared'.
        """

    @abc.abstractmethod
    def issue_signing_material(self) -> SigningMaterial:
        """Return the X.509 chain and key handle used to sign a claim."""

    @abc.abstractmethod
    def sign_claim(self, payload: bytes) -> bytes:
        """Sign claim bytes with the active key and record the act in history."""

    @abc.abstractmethod
    def embed_mark(self, audio_path: Path, payload: dict) -> dict:
        """Attach a soft binding to an asset and return what was attached.

        REQUIRED: the returned mapping states the binding's actual mechanism and
        its measured limits. A provider that has not measured perceptual
        transparency or transcode survival MUST NOT claim either.
        """

    @abc.abstractmethod
    def recover_mark(self, audio_path: Path) -> dict | None:
        """Recover a previously attached soft binding, or None if absent."""

    @abc.abstractmethod
    def register(self, manifest: dict) -> dict:
        """Submit a manifest to the registry and return the registry receipt."""

    @abc.abstractmethod
    def verify(self, asset_path: Path, manifest_bytes: bytes | None) -> dict:
        """Verify an asset, returning one of the four VerificationState values."""

    @abc.abstractmethod
    def revoke(self, key_id: str) -> dict:
        """Revoke a key.

        REQUIRED: revocation marks the key unusable for new signatures and MUST
        retain every prior signing record. Erasing what a creator signed is a
        different operation and is not offered by this interface.
        """

    @abc.abstractmethod
    def signing_history(self, key_id: str) -> list[dict]:
        """Return every signing record for a key, revoked or not."""
