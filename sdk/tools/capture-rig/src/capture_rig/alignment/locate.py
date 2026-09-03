"""Generalised cross-correlation with phase transform, used to place a known clip in a capture.

PHAT whitening rather than plain cross-correlation because the acoustic path colours the capture
heavily: a room, a consumer speaker and a phone microphone between them impose tens of dB of tilt,
and an unwhitened correlation peaks on whichever band survived rather than on the true delay.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np


class AlignmentError(ValueError):
    """A clip could not be located in a capture with the evidence available."""


def _mono(signal: np.ndarray) -> np.ndarray:
    array = np.asarray(signal, dtype=np.float64)
    if array.ndim == 2:
        array = array.mean(axis=1)
    if array.ndim != 1 or array.size == 0:
        raise AlignmentError("expected a non-empty mono or (frames, channels) signal")
    return array


@dataclass(frozen=True)
class Segment:
    """Where a source clip sits inside a capture, and how much the evidence supports it."""

    clip_id: str
    start_sample: int
    end_sample: int
    lag_seconds: float
    peak_to_sidelobe: float
    accepted: bool
    reason: str | None = None

    def to_dict(self) -> dict:
        return {
            "clip_id": self.clip_id,
            "start_sample": self.start_sample,
            "end_sample": self.end_sample,
            "lag_seconds": self.lag_seconds,
            "peak_to_sidelobe": self.peak_to_sidelobe,
            "accepted": self.accepted,
            "reason": self.reason,
            "provenance": "alignment_bookkeeping_only_never_used_for_detection",
        }


def gcc_phat(capture: np.ndarray, reference: np.ndarray, sample_rate: int) -> tuple[int, float]:
    """Return (lag in samples of `reference` within `capture`, peak-to-sidelobe ratio).

    A positive lag means the reference starts that many samples into the capture. Negative lags are
    representable so a mis-ordered pair fails loudly instead of wrapping into a plausible number.
    """
    x, y = _mono(capture), _mono(reference)
    if sample_rate <= 0:
        raise AlignmentError("sample_rate must be positive")
    size = int(1 << int(np.ceil(np.log2(x.size + y.size))))
    X = np.fft.rfft(x, n=size)
    Y = np.fft.rfft(y, n=size)
    cross = X * np.conj(Y)
    magnitude = np.abs(cross)
    cross = np.divide(cross, magnitude, out=np.zeros_like(cross), where=magnitude > 1e-20)
    correlation = np.fft.irfft(cross, n=size)
    peak_index = int(np.argmax(correlation))
    peak = float(correlation[peak_index])

    guard = max(int(round(0.005 * sample_rate)), 1)
    masked = correlation.copy()
    lo, hi = max(peak_index - guard, 0), min(peak_index + guard + 1, masked.size)
    masked[lo:hi] = 0.0
    sidelobe = float(np.max(np.abs(masked))) if masked.size else 0.0
    ratio = peak / sidelobe if sidelobe > 0.0 else float("inf")

    lag = peak_index if peak_index <= size // 2 else peak_index - size
    return lag, ratio


def locate_clip(
    capture: np.ndarray,
    reference: np.ndarray,
    sample_rate: int,
    clip_id: str,
    min_peak_to_sidelobe: float = 3.0,
) -> Segment:
    """Locate one known clip inside one capture."""
    lag, ratio = gcc_phat(capture, reference, sample_rate)
    reference_length = _mono(reference).size
    reasons: list[str] = []
    if ratio < min_peak_to_sidelobe:
        reasons.append(
            f"correlation peak is only {ratio:.2f}x its strongest sidelobe, below the "
            f"{min_peak_to_sidelobe} threshold; the clip may not be present"
        )
    if lag < 0:
        reasons.append(f"clip appears to start {abs(lag) / sample_rate:.3f} s before the capture does")
    return Segment(
        clip_id=clip_id,
        start_sample=lag,
        end_sample=lag + reference_length,
        lag_seconds=lag / sample_rate,
        peak_to_sidelobe=ratio,
        accepted=not reasons,
        reason="; ".join(reasons) or None,
    )


def segment_capture(
    capture: np.ndarray,
    references: list[tuple[str, np.ndarray]],
    sample_rate: int,
    min_peak_to_sidelobe: float = 3.0,
) -> list[Segment]:
    """Place every known clip in a capture that holds several of them, in the order given.

    Each clip is searched only in the capture remaining after the previous accepted segment, so a
    repeated clip cannot be matched twice to the same region.
    """
    audio = _mono(capture)
    segments: list[Segment] = []
    cursor = 0
    for clip_id, reference in references:
        if cursor >= audio.size:
            segments.append(
                Segment(clip_id, cursor, cursor, cursor / sample_rate, 0.0, False, "capture ended before this clip")
            )
            continue
        found = locate_clip(audio[cursor:], reference, sample_rate, clip_id, min_peak_to_sidelobe)
        absolute = Segment(
            clip_id=found.clip_id,
            start_sample=found.start_sample + cursor,
            end_sample=found.end_sample + cursor,
            lag_seconds=(found.start_sample + cursor) / sample_rate,
            peak_to_sidelobe=found.peak_to_sidelobe,
            accepted=found.accepted,
            reason=found.reason,
        )
        segments.append(absolute)
        if absolute.accepted:
            cursor = max(absolute.end_sample, cursor + 1)
    return segments
