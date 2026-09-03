from __future__ import annotations

import hashlib
import os
from dataclasses import dataclass
from pathlib import Path

from daemon.common import canonical_json_bytes

DEFAULT_PRIVATE_KEY = Path("~/.apw/demo_ed25519_private.key")
DEFAULT_PUBLIC_KEY = Path("~/.apw/demo_ed25519_public.key")


@dataclass(frozen=True)
class Ed25519Signer:
    private_key_path: Path = DEFAULT_PRIVATE_KEY
    public_key_path: Path = DEFAULT_PUBLIC_KEY

    def __post_init__(self) -> None:
        object.__setattr__(self, "private_key_path", self.private_key_path.expanduser())
        object.__setattr__(self, "public_key_path", self.public_key_path.expanduser())

    def _load_or_create_private_key(self):
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

        if self.private_key_path.is_file():
            raw = self.private_key_path.read_bytes()
            if len(raw) != 32:
                raise ValueError("Ed25519 private key file must contain exactly 32 raw bytes")
            private_key = Ed25519PrivateKey.from_private_bytes(raw)
        else:
            from cryptography.hazmat.primitives import serialization

            private_key = Ed25519PrivateKey.generate()
            raw = private_key.private_bytes(
                serialization.Encoding.Raw,
                serialization.PrivateFormat.Raw,
                serialization.NoEncryption(),
            )
            self.private_key_path.parent.mkdir(parents=True, exist_ok=True)
            self.private_key_path.write_bytes(raw)
            os.chmod(self.private_key_path, 0o600)
        self._write_public_key(private_key.public_key())
        return private_key

    def _write_public_key(self, public_key) -> None:
        from cryptography.hazmat.primitives import serialization

        raw = public_key.public_bytes(
            serialization.Encoding.Raw,
            serialization.PublicFormat.Raw,
        )
        self.public_key_path.parent.mkdir(parents=True, exist_ok=True)
        if not self.public_key_path.is_file() or self.public_key_path.read_bytes() != raw:
            self.public_key_path.write_bytes(raw)
            os.chmod(self.public_key_path, 0o644)

    def sign_manifest(self, unsigned_manifest: dict[str, object]) -> dict[str, object]:
        from cryptography.hazmat.primitives import serialization

        private_key = self._load_or_create_private_key()
        public_key = private_key.public_key()
        public_raw = public_key.public_bytes(
            serialization.Encoding.Raw,
            serialization.PublicFormat.Raw,
        )
        content = canonical_json_bytes(unsigned_manifest)
        return {
            "algorithm": "Ed25519",
            "canonicalization": "apw-json-sort-v1",
            "public_key_hex": public_raw.hex(),
            "public_key_file": str(self.public_key_path),
            "signer_id": hashlib.sha256(public_raw).hexdigest()[:16],
            "signature_hex": private_key.sign(content).hex(),
            "signed_content_hash": hashlib.sha256(content).hexdigest(),
            "trust_scope": "self_generated_demo_key_integrity",
            "signer_identity": "not_established",
            "signer_identity_proof_level": "unknown_unobserved",
            "apw:proof_level": "directly_observed",
            "notes": (
                "The public key independently verifies canonical manifest integrity. "
                "A valid self-generated signature proves key possession, not creator identity, authorship, or external trust."
            ),
        }

    def public_key_hex(self) -> str:
        from cryptography.hazmat.primitives import serialization

        private_key = self._load_or_create_private_key()
        return private_key.public_key().public_bytes(
            serialization.Encoding.Raw,
            serialization.PublicFormat.Raw,
        ).hex()


def pinned_public_key(public_key_path: Path | None = None) -> tuple[bytes | None, Path]:
    """Load the verifying key this machine pins, with the path it looked in.

    A caller that cannot find a pin must say so rather than report a failed
    signature: on a recipient's machine the two are completely different facts.
    """
    pinned = (public_key_path or DEFAULT_PUBLIC_KEY).expanduser()
    try:
        raw = pinned.read_bytes()
    except OSError:
        return None, pinned
    return (raw if len(raw) == 32 else None), pinned


def verify_ed25519_signature(
    unsigned_manifest: dict[str, object],
    signature: dict[str, object],
    public_key_path: Path | None = None,
) -> tuple[bool, str]:
    """Verify a portable signature against a key held by this machine.

    IMPORTANT: the verifying key is never taken from ``signature["public_key_hex"]``.
    A blob that supplies its own key proves only that whoever re-signed it owned some
    Ed25519 key, so any attacker could re-sign an edited document and pass. The
    embedded key is used solely to say whether the signer differs from the pinned one.
    """
    from cryptography.exceptions import InvalidSignature
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

    raw_public, pinned = pinned_public_key(public_key_path)
    if raw_public is None:
        return False, (
            f"no usable pinned Ed25519 public key at {pinned}; a portable signature "
            "cannot supply the key that verifies it"
        )
    embedded = str(signature.get("public_key_hex", ""))
    if embedded and embedded.lower() != raw_public.hex():
        return False, (
            "the portable signature was produced by a different key than the pinned "
            f"public key at {pinned}"
        )
    try:
        signature_bytes = bytes.fromhex(str(signature.get("signature_hex", "")))
    except ValueError:
        return False, "portable signature is malformed"
    content = canonical_json_bytes(unsigned_manifest)
    expected_hash = hashlib.sha256(content).hexdigest()
    if expected_hash != signature.get("signed_content_hash"):
        return False, "portable signed-content hash does not match canonical manifest"
    try:
        Ed25519PublicKey.from_public_bytes(raw_public).verify(signature_bytes, content)
    except InvalidSignature:
        return False, "Ed25519 signature is invalid"
    return True, (
        f"Ed25519 signature verified against the pinned public key at {pinned}; "
        "signer identity remains unverified"
    )
