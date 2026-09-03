"""Farina deconvolution: recorded sweep -> linear impulse response plus separated harmonic packets.

Deconvolving an exponential sine sweep pushes every harmonic distortion product to a NEGATIVE time
offset dt_k = T*ln(k)/ln(f2/f1) relative to the linear response. That is what lets one measurement
yield both the room response (spec 10.2(a)) and the loudspeaker's harmonic distortion (spec 10.2(c),
which parameterises distortion-layer stage D4 with a measured figure instead of a guessed one).
"""

from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np
from scipy.signal import fftconvolve

from .signals import SweepPair, SweepError

DEFAULT_PRE_SECONDS = 0.01
DEFAULT_IR_SECONDS = 2.0
MAX_HARMONIC_ORDER = 8


@dataclass(frozen=True)
class HarmonicPacket:
    order: int
    offset_seconds: float
    peak_abs: float
    energy: float


@dataclass(frozen=True)
class ExtractedResponse:
    """A linear impulse response and everything the deconvolution learned around it."""

    ir: np.ndarray
    sample_rate: int
    direct_index: int
    bulk_delay_seconds: float
    linear_peak_abs: float
    linear_energy: float
    noise_rms: float
    harmonics: tuple[HarmonicPacket, ...] = field(default=())

    @property
    def thd_percent(self) -> float | None:
        """sqrt(sum_{k>=2} E_k / E_1) as a percentage. None when the linear response has no energy."""
        if self.linear_energy <= 0.0:
            return None
        harmonic_energy = sum(p.energy for p in self.harmonics if p.order >= 2)
        return 100.0 * float(np.sqrt(harmonic_energy / self.linear_energy))

    @property
    def peak_to_noise_db(self) -> float | None:
        if self.noise_rms <= 0.0 or self.linear_peak_abs <= 0.0:
            return None
        return float(20.0 * np.log10(self.linear_peak_abs / self.noise_rms))

    def to_dict(self) -> dict:
        return {
            "sample_rate": self.sample_rate,
            "ir_samples": int(self.ir.size),
            "direct_index": self.direct_index,
            "bulk_delay_seconds": self.bulk_delay_seconds,
            "linear_peak_abs": self.linear_peak_abs,
            "peak_to_noise_db": self.peak_to_noise_db,
            "thd_percent": self.thd_percent,
            "harmonics": [
                {
                    "order": p.order,
                    "offset_seconds": p.offset_seconds,
                    "peak_abs": p.peak_abs,
                    "relative_db": (
                        float(20.0 * np.log10(p.peak_abs / self.linear_peak_abs))
                        if p.peak_abs > 0.0 and self.linear_peak_abs > 0.0
                        else None
                    ),
                }
                for p in self.harmonics
            ],
        }


def deconvolve(recording: np.ndarray, pair: SweepPair) -> np.ndarray:
    """Full LINEAR convolution of the recording with the inverse filter.

    Linear, never circular: a circular deconvolution wraps the harmonic packets, which arrive before
    time zero, around into the reverberant tail and corrupts both RT60 and THD.
    """
    signal = np.asarray(recording, dtype=np.float64)
    if signal.ndim != 1:
        raise SweepError("deconvolve operates on one channel at a time")
    if signal.size == 0:
        raise SweepError("empty recording")
    return fftconvolve(signal, pair.inverse, mode="full")


def _packet_window(centre: int, half_width: int, limit: int) -> tuple[int, int]:
    start = max(centre - half_width, 0)
    stop = min(centre + half_width + 1, limit)
    return start, stop


def extract_response(
    deconvolved: np.ndarray,
    pair: SweepPair,
    pre_seconds: float = DEFAULT_PRE_SECONDS,
    ir_seconds: float = DEFAULT_IR_SECONDS,
    max_harmonic_order: int = MAX_HARMONIC_ORDER,
) -> ExtractedResponse:
    """Locate the linear impulse in a deconvolved sweep and cut the response and harmonics out."""
    dec = np.asarray(deconvolved, dtype=np.float64)
    if dec.ndim != 1:
        raise SweepError("extract_response operates on one channel at a time")
    fs = pair.sample_rate
    guard = min(int(round(0.001 * fs)), pair.peak_index)
    search_from = pair.peak_index - guard
    if search_from >= dec.size:
        raise SweepError(
            "deconvolved signal is shorter than the sweep's own impulse position; the recording is "
            "shorter than the stimulus"
        )
    linear_peak = search_from + int(np.argmax(np.abs(dec[search_from:])))
    linear_peak_abs = float(abs(dec[linear_peak]))
    bulk_delay_seconds = (linear_peak - pair.peak_index) / fs

    pre = int(round(pre_seconds * fs))
    start = max(linear_peak - pre, 0)
    stop = min(start + int(round((pre_seconds + ir_seconds) * fs)), dec.size)
    ir = dec[start:stop].copy()
    direct_index = linear_peak - start
    linear_energy = float(np.dot(ir, ir))

    harmonics: list[HarmonicPacket] = []
    for order in range(2, max_harmonic_order + 1):
        offset = pair.harmonic_arrival_offset_seconds(order)
        centre = linear_peak - int(round(offset * fs))
        if centre <= 0:
            break
        previous = pair.harmonic_arrival_offset_seconds(order - 1)
        following = pair.harmonic_arrival_offset_seconds(order + 1)
        half_width = max(int(round(0.5 * min(offset - previous, following - offset) * fs)), 1)
        half_width = min(half_width, int(round(0.05 * fs)))
        lo, hi = _packet_window(centre, half_width, dec.size)
        if hi <= lo:
            break
        segment = dec[lo:hi]
        harmonics.append(
            HarmonicPacket(
                order=order,
                offset_seconds=offset,
                peak_abs=float(np.max(np.abs(segment))),
                energy=float(np.dot(segment, segment)),
            )
        )

    noise_from = min(stop + int(round(0.1 * fs)), dec.size)
    tail = dec[noise_from:]
    noise_rms = float(np.sqrt(np.mean(tail**2))) if tail.size >= fs // 10 else 0.0

    return ExtractedResponse(
        ir=ir,
        sample_rate=fs,
        direct_index=direct_index,
        bulk_delay_seconds=bulk_delay_seconds,
        linear_peak_abs=linear_peak_abs,
        linear_energy=linear_energy,
        noise_rms=noise_rms,
        harmonics=tuple(harmonics),
    )


def measure_response(recording: np.ndarray, pair: SweepPair, **kwargs) -> ExtractedResponse:
    return extract_response(deconvolve(recording, pair), pair, **kwargs)
