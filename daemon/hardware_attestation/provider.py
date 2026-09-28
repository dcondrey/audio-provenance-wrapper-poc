from __future__ import annotations

import abc
import hashlib
import hmac
import json
import logging
import os
import threading
import time
from dataclasses import dataclass
from pathlib import Path

log = logging.getLogger(__name__)


@dataclass(frozen=True)
class DeviceIdentity:
    """Unique hardware-bound identity for this machine."""

    device_id: str
    public_key_hex: str
    algorithm: str
    created_at_ms: int


@dataclass(frozen=True)
class HardwareBinding:
    """A hash chain root sealed to hardware."""

    chain_root_hash: str
    device_id: str
    monotonic_counter: int
    clock_ms: int
    signature_hex: str
    public_key_hex: str


@dataclass(frozen=True)
class HardwareCosignature:
    """Self-entangled hardware cosignature following the CPoE pattern.

    Each cosignature chains the previous one, making forgery of checkpoint N
    require valid signatures for all preceding checkpoints.

    entangled_hash = SHA256(
        domain_separator
        || content_hash
        || software_signature
        || hardware_clock_ms
        || monotonic_counter
        || device_id
        || previous_cosignature
    )
    """

    entangled_hash: str
    content_hash: str
    hardware_clock_ms: int
    monotonic_counter: int
    device_id: str
    signature_hex: str
    previous_cosignature_hash: str

    DOMAIN_SEPARATOR = b"apw-hw-cosign-v1"


class HardwareProvider(abc.ABC):
    """Abstract interface for hardware security modules.

    Implementations:
        SecureEnclaveProvider - macOS Secure Enclave via Security.framework
        TpmProvider           - Linux TPM 2.0 via tpm2-tools or tss2
        SoftwareProvider      - Fallback using filesystem keys (NOT attestable)
    """

    @abc.abstractmethod
    def device_identity(self) -> DeviceIdentity:
        """Return the hardware-bound device identity."""

    @abc.abstractmethod
    def sign(self, data: bytes) -> bytes:
        """Sign data with the hardware-bound private key.

        The private key never leaves the hardware module.
        """

    @abc.abstractmethod
    def verify(self, data: bytes, signature: bytes) -> bool:
        """Verify a signature against the hardware public key."""

    @abc.abstractmethod
    def seal(self, plaintext: bytes) -> bytes:
        """Encrypt data such that only this hardware can decrypt it.

        Sealed data is bound to the current device and platform state.
        """

    @abc.abstractmethod
    def unseal(self, sealed: bytes) -> bytes:
        """Decrypt hardware-sealed data."""

    @abc.abstractmethod
    def monotonic_counter(self) -> int:
        """Return a hardware-backed monotonic counter value.

        This counter increments on each call and cannot be rolled back.
        Used to detect replay attacks on the hash chain.
        """

    @abc.abstractmethod
    def clock_ms(self) -> int:
        """Return the hardware-attested clock value in milliseconds.

        On TPM this is the TPM clock; on Secure Enclave this is the
        monotonic system clock attested by the SE.
        """

    def last_cosignature_hash(self) -> str:
        """Head of this device's cosignature chain, or 'genesis' if it has none."""
        return "genesis"

    def counter_scope(self) -> str:
        """State what the monotonic counter is actually backed by."""
        return "provider_defined"

    def _record_cosignature(self, entangled_hash: str) -> None:
        """Persist the new chain head. Providers without durable state keep none."""

    def bind_chain_root(self, chain_root_hash: str) -> HardwareBinding:
        """Bind a hash chain root to this hardware device.

        Creates a signed attestation that this specific hash chain root
        was produced on this specific device at this specific counter value.
        """
        identity = self.device_identity()
        counter = self.monotonic_counter()
        clock = self.clock_ms()

        payload = (
            chain_root_hash.encode()
            + identity.device_id.encode()
            + counter.to_bytes(8, "big")
            + clock.to_bytes(8, "big")
        )
        signature = self.sign(payload)

        return HardwareBinding(
            chain_root_hash=chain_root_hash,
            device_id=identity.device_id,
            monotonic_counter=counter,
            clock_ms=clock,
            signature_hex=signature.hex(),
            public_key_hex=identity.public_key_hex,
        )

    def cosign_checkpoint(
        self,
        content_hash: str,
        software_signature: str,
        previous_cosignature_hash: str,
    ) -> HardwareCosignature:
        """Create a self-entangled hardware cosignature for a checkpoint.

        Each cosignature includes the hash of the previous one, creating
        a chain that is bound to both software evidence and hardware state.
        """
        identity = self.device_identity()
        counter = self.monotonic_counter()
        clock = self.clock_ms()

        entangle_input = (
            HardwareCosignature.DOMAIN_SEPARATOR
            + content_hash.encode()
            + software_signature.encode()
            + clock.to_bytes(8, "big")
            + counter.to_bytes(8, "big")
            + identity.device_id.encode()
            + previous_cosignature_hash.encode()
        )
        entangled_hash = hashlib.sha256(entangle_input).hexdigest()
        signature = self.sign(entangled_hash.encode())
        self._record_cosignature(entangled_hash)

        return HardwareCosignature(
            entangled_hash=entangled_hash,
            content_hash=content_hash,
            hardware_clock_ms=clock,
            monotonic_counter=counter,
            device_id=identity.device_id,
            signature_hex=signature.hex(),
            previous_cosignature_hash=previous_cosignature_hash,
        )


class SecureEnclaveProvider(HardwareProvider):
    """macOS Secure Enclave via Security.framework.

    Requires pyobjc-framework-Security or ctypes bindings to:
        SecKeyCreateRandomKey (kSecAttrTokenIDSecureEnclave)
        SecKeyCreateSignature (kSecKeyAlgorithmECDSASignatureMessageX962SHA256)
        SecKeyVerifySignature

    The private key is created with kSecAttrIsPermanent=True and stored
    in the Secure Enclave. It never leaves the hardware.

    Stub: all methods raise NotImplementedError.
    """

    def device_identity(self) -> DeviceIdentity:
        raise NotImplementedError("Secure Enclave integration not yet implemented")

    def sign(self, data: bytes) -> bytes:
        raise NotImplementedError("Secure Enclave integration not yet implemented")

    def verify(self, data: bytes, signature: bytes) -> bool:
        raise NotImplementedError("Secure Enclave integration not yet implemented")

    def seal(self, plaintext: bytes) -> bytes:
        raise NotImplementedError("Secure Enclave integration not yet implemented")

    def unseal(self, sealed: bytes) -> bytes:
        raise NotImplementedError("Secure Enclave integration not yet implemented")

    def monotonic_counter(self) -> int:
        raise NotImplementedError("Secure Enclave integration not yet implemented")

    def clock_ms(self) -> int:
        raise NotImplementedError("Secure Enclave integration not yet implemented")


class TpmProvider(HardwareProvider):
    """Linux TPM 2.0 via tpm2-tools CLI or tss2 Python bindings.

    Uses the TPM endorsement key hierarchy:
        EK (Endorsement Key)  - device identity, not directly usable
        SRK (Storage Root Key) - parent for sealing
        AK (Attestation Key)  - signing for quotes and attestations

    Monotonic counter: TPM2_NV_Increment on a reserved NV index.
    Clock: TPM2_ReadClock for attested time.

    Stub: all methods raise NotImplementedError.
    """

    def device_identity(self) -> DeviceIdentity:
        raise NotImplementedError("TPM 2.0 integration not yet implemented")

    def sign(self, data: bytes) -> bytes:
        raise NotImplementedError("TPM 2.0 integration not yet implemented")

    def verify(self, data: bytes, signature: bytes) -> bool:
        raise NotImplementedError("TPM 2.0 integration not yet implemented")

    def seal(self, plaintext: bytes) -> bytes:
        raise NotImplementedError("TPM 2.0 integration not yet implemented")

    def unseal(self, sealed: bytes) -> bytes:
        raise NotImplementedError("TPM 2.0 integration not yet implemented")

    def monotonic_counter(self) -> int:
        raise NotImplementedError("TPM 2.0 integration not yet implemented")

    def clock_ms(self) -> int:
        raise NotImplementedError("TPM 2.0 integration not yet implemented")


class SoftwareProvider(HardwareProvider):
    """Fallback provider using a filesystem-stored HMAC key.

    NOT attestable. Evidence produced with this provider carries proof level
    'directly_observed' for the hash chain but 'unknown_unobserved' for
    hardware binding. An auditor can verify chain integrity but not that
    the chain was produced on a specific device.

    Uses HMAC-SHA256 for signing (stdlib only, no external dependencies).
    The signing key is derived from a random seed stored on disk. This is
    NOT equivalent to real Ed25519 but provides a functional signing flow
    for development and testing.
    """

    def __init__(self, key_path: Path = Path("~/.apw/device_key.bin")) -> None:
        self.key_path = key_path.expanduser()
        self._state_path = self.key_path.with_name(self.key_path.name + ".state.json")
        self._state_lock = threading.Lock()
        self._seed = self._load_or_create_key()
        self._device_id = hashlib.sha256(self._seed).hexdigest()[:16]

    @staticmethod
    def _restrict(path: Path) -> None:
        """Keep the signing seed unreadable by other local users.

        IMPORTANT: repaired on every load, not only on creation. Anyone who can
        read the seed derives the device_id and forges a manifest_signature the
        verifier accepts, and machines provisioned before this check existed
        still carry the world-readable file.
        """
        if os.name == "nt":
            # POSIX mode bits do not express Windows ACLs; nothing is enforced here.
            log.warning("Signing seed %s: file permissions are not managed on Windows", path)
            return
        try:
            mode = path.stat().st_mode & 0o777
            if mode & 0o077:
                os.chmod(path, 0o600)
                log.warning("Tightened permissions on %s from %o to 600", path, mode)
        except OSError:
            log.warning("Could not restrict permissions on %s", path, exc_info=True)

    def _load_or_create_key(self) -> bytes:
        if self.key_path.exists():
            self._restrict(self.key_path)
            return self.key_path.read_bytes()
        seed = os.urandom(32)
        self.key_path.parent.mkdir(parents=True, exist_ok=True)
        self.key_path.write_bytes(seed)
        self._restrict(self.key_path)
        log.info("Created software signing key at %s", self.key_path)
        return seed

    def _read_state(self) -> dict[str, object]:
        try:
            state = json.loads(self._state_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError, UnicodeDecodeError):
            return {}
        return state if isinstance(state, dict) else {}

    def _write_state(self, state: dict[str, object]) -> None:
        temporary = self._state_path.with_name(self._state_path.name + f".tmp-{os.getpid()}")
        try:
            temporary.write_text(json.dumps(state, sort_keys=True), encoding="utf-8")
            temporary.replace(self._state_path)
            self._restrict(self._state_path)
        except OSError:
            temporary.unlink(missing_ok=True)
            log.warning("Could not persist signer state to %s", self._state_path, exc_info=True)

    def last_cosignature_hash(self) -> str:
        value = self._read_state().get("last_cosignature_hash")
        return value if isinstance(value, str) and value else "genesis"

    def counter_scope(self) -> str:
        return (
            "persisted local file counter, not hardware-backed; rollback is prevented only "
            "by filesystem permissions on the signer state file"
        )

    def _record_cosignature(self, entangled_hash: str) -> None:
        with self._state_lock:
            state = self._read_state()
            state["last_cosignature_hash"] = entangled_hash
            self._write_state(state)

    def device_identity(self) -> DeviceIdentity:
        return DeviceIdentity(
            device_id=self._device_id,
            public_key_hex="",
            algorithm="hmac-sha256-local",
            created_at_ms=int(self.key_path.stat().st_mtime * 1000),
        )

    def sign(self, data: bytes) -> bytes:
        return hmac.new(self._seed, data, hashlib.sha256).digest()

    def verify(self, data: bytes, signature: bytes) -> bool:
        expected = hmac.new(self._seed, data, hashlib.sha256).digest()
        return hmac.compare_digest(expected, signature)

    @staticmethod
    def _keystream(key: bytes, nonce: bytes, length: int) -> bytes:
        """Counter-varied keystream: HMAC(key, nonce || counter) per block.

        A repeated fixed-key block (key * n) leaks identical ciphertext for
        identical aligned plaintext blocks (ECB-style). Mixing in a
        big-endian block counter makes every block's keystream distinct.
        """
        blocks = []
        produced = 0
        counter = 0
        while produced < length:
            block = hmac.new(key, nonce + counter.to_bytes(8, "big"), hashlib.sha256).digest()
            blocks.append(block)
            produced += len(block)
            counter += 1
        return b"".join(blocks)[:length]

    def seal(self, plaintext: bytes) -> bytes:
        nonce = os.urandom(16)
        key = hmac.new(self._seed, b"apw-seal-" + nonce, hashlib.sha256).digest()
        keystream = self._keystream(key, nonce, len(plaintext))
        sealed = bytes(a ^ b for a, b in zip(plaintext, keystream))
        tag = hmac.new(key, sealed, hashlib.sha256).digest()[:16]
        return nonce + tag + sealed

    def unseal(self, sealed_data: bytes) -> bytes:
        if len(sealed_data) < 32:
            raise ValueError("Sealed data too short")
        nonce = sealed_data[:16]
        tag = sealed_data[16:32]
        ciphertext = sealed_data[32:]
        key = hmac.new(self._seed, b"apw-seal-" + nonce, hashlib.sha256).digest()
        expected_tag = hmac.new(key, ciphertext, hashlib.sha256).digest()[:16]
        if not hmac.compare_digest(tag, expected_tag):
            raise ValueError("Sealed data integrity check failed")
        keystream = self._keystream(key, nonce, len(ciphertext))
        return bytes(a ^ b for a, b in zip(ciphertext, keystream))

    def monotonic_counter(self) -> int:
        """Persisted across processes.

        A per-process integer restarted at 1 on every launch, which made the
        signed counter useless for the replay, reorder and drop detection the
        HardwareProvider contract promises it is for.
        """
        with self._state_lock:
            state = self._read_state()
            raw = state.get("monotonic_counter")
            current = raw if isinstance(raw, int) and not isinstance(raw, bool) and raw >= 0 else 0
            current += 1
            state["monotonic_counter"] = current
            self._write_state(state)
            return current

    def clock_ms(self) -> int:
        return int(time.time() * 1000)


def attestation_status(system: str | None = None) -> dict[str, object]:
    """What this platform's provider proves. Never claims hardware it does not use.

    Every platform currently maps to SoftwareProvider, so the answer is the
    same on all of them: a local key file, not hardware-attested.
    """
    import platform

    system = system or platform.system()
    if system == "Darwin":
        candidate = "Secure Enclave (not integrated)"
    elif system == "Linux":
        present = Path("/dev/tpm0").exists() or Path("/dev/tpmrm0").exists()
        candidate = "TPM 2.0 device present (not integrated)" if present else "no TPM device node found"
    elif system == "Windows":
        candidate = "TPM via CNG/Platform Crypto Provider (not integrated)"
    else:
        candidate = "no hardware provider known for this platform"
    return {
        "platform": system or "unknown",
        "provider": "SoftwareProvider",
        "hardware_attested": False,
        "proof_level": "unknown_unobserved",
        "hardware_candidate": candidate,
    }


def detect_provider(
    software_key_path: Path = Path("~/.apw/demo_signing_key.bin"),
) -> HardwareProvider:
    """Return the provider for this platform: always SoftwareProvider today.

    No Secure Enclave, TPM or Windows platform-crypto provider is integrated on
    any OS. The result is a local software integrity key and is logged as not
    hardware-attested; see attestation_status() for the per-platform detail.
    """
    status = attestation_status()
    log.warning(
        "Hardware attestation unavailable on %s (%s); using a local software "
        "integrity key that is not hardware-attested",
        status["platform"],
        status["hardware_candidate"],
    )
    return SoftwareProvider(key_path=software_key_path)
