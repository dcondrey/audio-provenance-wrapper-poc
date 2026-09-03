"""Synthesized audio for smoke tests and for the corpus classes the Rust bench already names.

This is NOT training data. Spec 9.1 fixes the real corpora; these signals exist so every script in
the harness runs end to end with no download, and so the eval harness has the same content classes
(`harmonic_pad`, `near_silence`, `transient_pattern`) the Rust bench reports separately.
"""

import math
from dataclasses import dataclass

import numpy as np

CONTENT_CLASSES = (
    "music_like",
    "harmonic_pad",
    "transient_pattern",
    "speech_like",
    "near_silence",
)


def _adsr(length: int, attack: int, release: int) -> np.ndarray:
    envelope = np.ones(length, dtype=np.float64)
    attack = min(attack, length)
    release = min(release, length - attack) if length > attack else 0
    if attack:
        envelope[:attack] = np.linspace(0.0, 1.0, attack)
    if release:
        envelope[-release:] = np.linspace(1.0, 0.0, release)
    return envelope


def _music_like(rng: np.random.Generator, samples: int, sample_rate: int) -> np.ndarray:
    time = np.arange(samples, dtype=np.float64) / sample_rate
    out = np.zeros(samples, dtype=np.float64)
    root = float(rng.uniform(80.0, 220.0))
    for degree in (1.0, 1.25, 1.5, 2.0, 3.0):
        partials = rng.integers(3, 7)
        for harmonic in range(1, int(partials) + 1):
            frequency = root * degree * harmonic
            if frequency > sample_rate * 0.42:
                break
            vibrato = 1.0 + 0.002 * np.sin(2.0 * math.pi * float(rng.uniform(3.0, 7.0)) * time)
            phase = float(rng.uniform(0.0, 2.0 * math.pi))
            out += (0.6 / (harmonic**1.3)) * np.sin(2.0 * math.pi * frequency * vibrato * time + phase)
    beat = int(sample_rate * float(rng.uniform(0.35, 0.6)))
    for start in range(0, samples - beat, beat):
        length = min(int(sample_rate * 0.09), samples - start)
        noise = rng.normal(size=length) * np.exp(-np.arange(length) / (sample_rate * 0.012))
        out[start : start + length] += 1.4 * noise
    out += rng.normal(size=samples) * 0.004
    return out


def _harmonic_pad(rng: np.random.Generator, samples: int, sample_rate: int) -> np.ndarray:
    time = np.arange(samples, dtype=np.float64) / sample_rate
    root = float(rng.uniform(110.0, 160.0))
    out = np.zeros(samples, dtype=np.float64)
    for harmonic in range(1, 9):
        out += (1.0 / harmonic) * np.sin(2.0 * math.pi * root * harmonic * time + harmonic)
    return out * _adsr(samples, int(sample_rate * 0.4), int(sample_rate * 0.4))


def _transient_pattern(rng: np.random.Generator, samples: int, sample_rate: int) -> np.ndarray:
    out = np.zeros(samples, dtype=np.float64)
    period = int(sample_rate * 0.25)
    for start in range(0, samples - period, period):
        length = min(int(sample_rate * 0.02), samples - start)
        out[start : start + length] += rng.normal(size=length) * np.exp(
            -np.arange(length) / (sample_rate * 0.003)
        )
    return out


def _speech_like(rng: np.random.Generator, samples: int, sample_rate: int) -> np.ndarray:
    time = np.arange(samples, dtype=np.float64) / sample_rate
    pitch = 120.0 + 40.0 * np.sin(2.0 * math.pi * 1.7 * time)
    excitation = np.sign(np.sin(2.0 * math.pi * np.cumsum(pitch) / sample_rate))
    out = np.zeros(samples, dtype=np.float64)
    for centre, bandwidth, gain in ((650.0, 90.0, 1.0), (1180.0, 110.0, 0.6), (2550.0, 160.0, 0.35)):
        decay = math.exp(-math.pi * bandwidth / sample_rate)
        theta = 2.0 * math.pi * centre / sample_rate
        b = [gain * (1.0 - decay)]
        a = [1.0, -2.0 * decay * math.cos(theta), decay**2]
        filtered = np.zeros(samples, dtype=np.float64)
        for n in range(2, samples):
            filtered[n] = b[0] * excitation[n] - a[1] * filtered[n - 1] - a[2] * filtered[n - 2]
        out += filtered
    gate = (np.sin(2.0 * math.pi * 0.7 * time) > -0.3).astype(np.float64)
    return out * gate + rng.normal(size=samples) * 0.002


def _near_silence(rng: np.random.Generator, samples: int, sample_rate: int) -> np.ndarray:
    return rng.normal(size=samples) * 1e-4


_GENERATORS = {
    "music_like": _music_like,
    "harmonic_pad": _harmonic_pad,
    "transient_pattern": _transient_pattern,
    "speech_like": _speech_like,
    "near_silence": _near_silence,
}


@dataclass(frozen=True)
class SyntheticCorpus:
    """Deterministic pseudo-corpus. Item i is a pure function of (base_seed, i)."""

    items: int
    seconds: float
    sample_rate: int = 48_000
    base_seed: int = 0
    classes: tuple[str, ...] = CONTENT_CLASSES

    def __len__(self) -> int:
        return self.items

    def content_class(self, index: int) -> str:
        return self.classes[index % len(self.classes)]

    def load(self, index: int) -> np.ndarray:
        rng = np.random.default_rng((self.base_seed * 1_000_003 + index) & 0xFFFF_FFFF)
        samples = int(self.seconds * self.sample_rate)
        audio = _GENERATORS[self.content_class(index)](rng, samples, self.sample_rate)
        peak = float(np.abs(audio).max())
        if peak > 0.0 and self.content_class(index) != "near_silence":
            audio = audio / peak * float(rng.uniform(0.2, 0.7))
        return audio.astype(np.float32)

    def provenance(self) -> dict:
        return {
            "kind": "synthetic",
            "items": self.items,
            "seconds": self.seconds,
            "sample_rate": self.sample_rate,
            "classes": list(self.classes),
            "licence": "audio-provenance-owned",
            "note": "Generated signals, not a music corpus. Present so the harness runs with no download.",
        }
