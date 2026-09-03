"""The 56-bit message, CRC-24, and the FIXED flip search of spec 3.2.

IMPORTANT: nothing in this module takes the true payload as an argument. The decision to accept a
window is made from the CRC alone. Truth enters only in the scorer, after acceptance, which is what
makes the oracle bug of spec 4.1 unrepresentable here rather than merely avoided.
"""

import itertools
from dataclasses import dataclass

import numpy as np

from .config import PayloadConfig

CRC24_INIT = 0x00B7_04CE
CRC24_MASK = 0x00FF_FFFF


def crc24(data: bytes, poly: int = 0x864CFB) -> int:
    """CRC-24/OPENPGP. No reflection, no final xor, init 0xB704CE."""
    register = CRC24_INIT
    for byte in data:
        register ^= byte << 16
        for _ in range(8):
            register <<= 1
            if register & 0x0100_0000:
                register ^= poly
    return register & CRC24_MASK


def bits_to_bytes(bits) -> bytes:
    bits = np.asarray(bits, dtype=np.uint8)
    if bits.size % 8 != 0:
        bits = np.concatenate([bits, np.zeros(8 - bits.size % 8, dtype=np.uint8)])
    return bytes(np.packbits(bits, bitorder="big").tolist())


def int_to_bits(value: int, width: int) -> np.ndarray:
    if value < 0 or value >= (1 << width):
        raise ValueError(f"{value} does not fit in {width} bits")
    return np.array([(value >> (width - 1 - i)) & 1 for i in range(width)], dtype=np.uint8)


def bits_to_int(bits) -> int:
    value = 0
    for bit in np.asarray(bits, dtype=np.uint8):
        value = (value << 1) | int(bit)
    return value


@dataclass(frozen=True)
class Message:
    version: int
    namespace: int
    locator_prefix: int


@dataclass(frozen=True)
class Decoded:
    """What a blind decode returns. `bits_corrected` is 0..2 per spec 3.2."""

    bits: np.ndarray
    message: Message
    bits_corrected: int
    crc_trials: int
    mean_abs_logit: float


class MessageCodec:
    def __init__(self, config: PayloadConfig | None = None) -> None:
        self.config = config or PayloadConfig()
        self.config.validate()
        self._patterns = self._flip_patterns()

    def _flip_patterns(self) -> tuple[tuple[int, ...], ...]:
        """Ordered-statistics flip patterns over positions 0..3 of the least-confident ordering.

        A fixed constant: 1 + 4 + 6 = 11, independent of the logit values and of any payload.
        """
        positions = range(self.config.flip_candidates)
        patterns: list[tuple[int, ...]] = []
        for weight in range(self.config.flip_max_weight + 1):
            patterns.extend(itertools.combinations(positions, weight))
        return tuple(patterns)

    @property
    def patterns(self) -> tuple[tuple[int, ...], ...]:
        return self._patterns

    @property
    def crc_trials(self) -> int:
        return len(self._patterns)

    def encode(self, message: Message) -> np.ndarray:
        cfg = self.config
        payload = np.concatenate(
            [
                int_to_bits(message.version, cfg.version_bits),
                int_to_bits(message.namespace, cfg.namespace_bits),
                int_to_bits(message.locator_prefix, cfg.locator_bits),
            ]
        )
        checksum = crc24(bits_to_bytes(payload), cfg.crc_poly)
        return np.concatenate([payload, int_to_bits(checksum, cfg.crc_bits)]).astype(np.uint8)

    def check(self, bits: np.ndarray) -> Message | None:
        cfg = self.config
        covered = bits[: cfg.covered_bits]
        claimed = bits_to_int(bits[cfg.covered_bits :])
        if crc24(bits_to_bytes(covered), cfg.crc_poly) != claimed:
            return None
        version = bits_to_int(covered[: cfg.version_bits])
        if version != cfg.version_value:
            return None
        namespace = bits_to_int(covered[cfg.version_bits : cfg.version_bits + cfg.namespace_bits])
        locator = bits_to_int(covered[cfg.version_bits + cfg.namespace_bits :])
        return Message(version=version, namespace=namespace, locator_prefix=locator)

    def decode(self, logits) -> Decoded | None:
        """Blind decode. `logits` are the 56 pooled bit logits; positive means bit 1.

        Returns None when no flip pattern passes the CRC. The caller cannot influence which pattern
        is taken, and no ground truth is consulted.
        """
        logits = np.asarray(logits, dtype=np.float64).reshape(-1)
        if logits.size != self.config.message_bits:
            raise ValueError(f"expected {self.config.message_bits} logits, got {logits.size}")
        hard = (logits > 0.0).astype(np.uint8)
        order = np.argsort(np.abs(logits), kind="stable")[: self.config.flip_candidates]
        for pattern in self._patterns:
            trial = hard.copy()
            for position in pattern:
                trial[order[position]] ^= 1
            message = self.check(trial)
            if message is not None:
                return Decoded(
                    bits=trial,
                    message=message,
                    bits_corrected=len(pattern),
                    crc_trials=self.crc_trials,
                    mean_abs_logit=float(np.abs(logits).mean()),
                )
        return None

    def random_message(self, rng: np.random.Generator) -> Message:
        cfg = self.config
        return Message(
            version=cfg.version_value,
            namespace=int(rng.integers(0, 1 << cfg.namespace_bits)),
            locator_prefix=int(rng.integers(0, 1 << cfg.locator_bits)),
        )


def false_accept_bound(windows: int, codec: MessageCodec) -> float:
    """Spec 3.4. Per-file CRC-gated false-accept rate for a stated window count."""
    return windows * codec.crc_trials * 2.0 ** (-codec.config.crc_bits)
