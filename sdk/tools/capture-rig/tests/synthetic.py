"""Synthetic impulse responses with KNOWN answers, so the sweep mathematics is checked against a
designed truth rather than against itself."""

from __future__ import annotations

import numpy as np

LN_10_POW_6 = 6.907755278982137


def tap_ir(sample_rate: int, taps: list[tuple[float, float]], length_seconds: float = 0.2) -> np.ndarray:
    """An impulse response made only of discrete taps at (delay in seconds, amplitude)."""
    ir = np.zeros(int(round(length_seconds * sample_rate)))
    for delay, amplitude in taps:
        ir[int(round(delay * sample_rate))] = amplitude
    return ir


def decaying_ir(
    sample_rate: int,
    rt60_seconds: float,
    direct_amplitude: float = 1.0,
    tail_amplitude: float = 0.25,
    tail_start_seconds: float = 0.004,
    length_seconds: float | None = None,
    seed: int = 20260901,
) -> np.ndarray:
    """A direct tap plus an exponentially decaying noise tail with a DESIGNED RT60.

    The tail envelope is exp(-t * ln(10^6) / RT60), which is exactly a 60 dB decay over RT60 seconds,
    so `rt60_t30` has a known number to recover.
    """
    length = int(round((length_seconds or rt60_seconds * 2.0) * sample_rate))
    rng = np.random.default_rng(seed)
    t = np.arange(length, dtype=np.float64) / sample_rate
    tail = rng.standard_normal(length) * np.exp(-t * LN_10_POW_6 / rt60_seconds) * tail_amplitude
    tail[: int(round(tail_start_seconds * sample_rate))] = 0.0
    tail[0] = direct_amplitude
    return tail
