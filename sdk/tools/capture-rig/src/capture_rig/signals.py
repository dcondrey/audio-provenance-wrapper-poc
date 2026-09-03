"""Exponential sine sweep generation and its Farina inverse filter.

The sweep is the measurement stimulus for section 10.2(a) of docs/GENOMARK_N_SPEC.md. Its inverse
filter is built here rather than at deconvolution time so that the normalisation constant, which is
what makes a deconvolved impulse response absolutely scaled rather than arbitrarily scaled, is
computed once from the sweep pair itself.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np
from scipy.signal import fftconvolve

MIN_SWEEP_SECONDS = 0.05
MAX_SWEEP_SECONDS = 300.0


class SweepError(ValueError):
    """A sweep was requested with parameters that do not describe a realisable stimulus."""


def _raised_cosine(length: int, rising: bool) -> np.ndarray:
    if length <= 0:
        return np.ones(0, dtype=np.float64)
    ramp = 0.5 - 0.5 * np.cos(np.pi * np.arange(length, dtype=np.float64) / length)
    return ramp if rising else ramp[::-1]


@dataclass(frozen=True)
class SweepPair:
    """An exponential sine sweep and the inverse filter that deconvolves it to a unit impulse."""

    sweep: np.ndarray
    inverse: np.ndarray
    sample_rate: int
    f_start_hz: float
    f_end_hz: float
    duration_seconds: float
    peak_index: int

    @property
    def rate_constant(self) -> float:
        """k = ln(f2/f1) / T, the exponential rate of the instantaneous frequency in rad/s per s."""
        return math.log(self.f_end_hz / self.f_start_hz) / self.duration_seconds

    def harmonic_arrival_offset_seconds(self, order: int) -> float:
        """How far BEFORE the linear impulse the `order`-th harmonic packet lands.

        Farina's result: dt_k = T * ln(k) / ln(f2/f1). Order 1 is the linear response itself and
        therefore has offset zero.
        """
        if order < 1:
            raise SweepError("harmonic order must be >= 1")
        return self.duration_seconds * math.log(order) / math.log(self.f_end_hz / self.f_start_hz)

    def to_dict(self) -> dict:
        return {
            "f_start_hz": self.f_start_hz,
            "f_end_hz": self.f_end_hz,
            "duration_seconds": self.duration_seconds,
            "sample_rate": self.sample_rate,
            "samples": int(self.sweep.size),
        }


def exponential_sweep(
    sample_rate: int,
    duration_seconds: float,
    f_start_hz: float = 20.0,
    f_end_hz: float = 20000.0,
    fade_in_seconds: float = 0.02,
    fade_out_seconds: float = 0.05,
) -> SweepPair:
    """Build an exponential sine sweep and its amplitude-corrected time-reversed inverse.

    The inverse is scaled so that convolving the sweep with it yields a peak of exactly 1.0, which
    makes a deconvolved room response carry the true linear gain of the acoustic path rather than an
    arbitrary constant.
    """
    if sample_rate <= 0:
        raise SweepError("sample_rate must be positive")
    if not MIN_SWEEP_SECONDS <= duration_seconds <= MAX_SWEEP_SECONDS:
        raise SweepError(
            f"duration_seconds must lie in [{MIN_SWEEP_SECONDS}, {MAX_SWEEP_SECONDS}], got {duration_seconds}"
        )
    if not 0.0 < f_start_hz < f_end_hz:
        raise SweepError("require 0 < f_start_hz < f_end_hz")
    if f_end_hz > sample_rate / 2.0:
        raise SweepError(
            f"f_end_hz {f_end_hz} exceeds Nyquist {sample_rate / 2.0} for sample_rate {sample_rate}"
        )
    if fade_in_seconds < 0.0 or fade_out_seconds < 0.0:
        raise SweepError("fades must be non-negative")

    length = int(round(duration_seconds * sample_rate))
    if length < 8:
        raise SweepError("sweep is shorter than 8 samples")
    duration = length / sample_rate

    t = np.arange(length, dtype=np.float64) / sample_rate
    k = math.log(f_end_hz / f_start_hz) / duration
    phase = (2.0 * math.pi * f_start_hz / k) * (np.exp(k * t) - 1.0)
    sweep = np.sin(phase)

    fade_in = min(int(round(fade_in_seconds * sample_rate)), length // 2)
    fade_out = min(int(round(fade_out_seconds * sample_rate)), length // 2)
    if fade_in:
        sweep[:fade_in] *= _raised_cosine(fade_in, rising=True)
    if fade_out:
        sweep[-fade_out:] *= _raised_cosine(fade_out, rising=False)

    inverse = sweep[::-1] * np.exp(-k * t[::-1])
    impulse = fftconvolve(sweep, inverse, mode="full")
    peak_index = int(np.argmax(np.abs(impulse)))
    peak = float(impulse[peak_index])
    if abs(peak) < 1e-30:
        raise SweepError("degenerate sweep: inverse filter produced no impulse")
    inverse = inverse / peak

    return SweepPair(
        sweep=sweep.astype(np.float64),
        inverse=inverse.astype(np.float64),
        sample_rate=sample_rate,
        f_start_hz=f_start_hz,
        f_end_hz=f_end_hz,
        duration_seconds=duration,
        peak_index=peak_index,
    )


def sweep_stimulus(pair: SweepPair, silence_seconds: float, amplitude_dbfs: float) -> np.ndarray:
    """The sweep at a stated playback amplitude followed by a silent tail.

    IMPORTANT: the tail must exceed the room's RT60 or the decay is truncated before it reaches the
    -35 dB point that T30 is fitted over, and `rt60_t30` will refuse to return a value.
    """
    if silence_seconds < 0.0:
        raise SweepError("silence_seconds must be non-negative")
    if amplitude_dbfs > 0.0:
        raise SweepError("amplitude_dbfs must be at or below full scale")
    tail = np.zeros(int(round(silence_seconds * pair.sample_rate)), dtype=np.float64)
    return np.concatenate([pair.sweep * (10.0 ** (amplitude_dbfs / 20.0)), tail])
