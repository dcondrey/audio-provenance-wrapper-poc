"""Calibration tone: set playback SPL and capture gain, and VERIFY both before a campaign runs.

The tone is 1 kHz because that is the frequency at which A-weighting is unity, so the operator's
sound-level meter reading and this rig's A-weighted dBFS refer to the same quantity by construction
and the conversion in `acoustics.dba_from_dbfs` introduces no weighting error.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np
from scipy.signal import welch

from .acoustics import a_weighted_rms_dbfs, rms_dbfs
from .audio import peak_dbfs
from .config import SplCalibration
from .record import CLIPPING_PEAK_DBFS, utc_now

TONE_HZ = 1000.0
TONE_BANDWIDTH_HZ = 25.0
TARGET_PEAK_DBFS = (-18.0, -6.0)
MIN_TONE_FRACTION = 0.5


class CalibrationError(ValueError):
    """A calibration capture cannot support the conclusion it is being asked for."""


def calibration_tone(
    sample_rate: int,
    seconds: float = 5.0,
    frequency_hz: float = TONE_HZ,
    amplitude_dbfs: float = -12.0,
    fade_seconds: float = 0.05,
) -> np.ndarray:
    if seconds <= 0.0:
        raise CalibrationError("tone length must be positive")
    if amplitude_dbfs > 0.0:
        raise CalibrationError("tone amplitude must be at or below full scale")
    if not 0.0 < frequency_hz < sample_rate / 2.0:
        raise CalibrationError("tone frequency must lie below Nyquist")
    length = int(round(seconds * sample_rate))
    t = np.arange(length, dtype=np.float64) / sample_rate
    tone = np.sin(2.0 * np.pi * frequency_hz * t) * (10.0 ** (amplitude_dbfs / 20.0))
    fade = min(int(round(fade_seconds * sample_rate)), length // 2)
    if fade:
        ramp = 0.5 - 0.5 * np.cos(np.pi * np.arange(fade, dtype=np.float64) / fade)
        tone[:fade] *= ramp
        tone[-fade:] *= ramp[::-1]
    return tone


@dataclass(frozen=True)
class ToneAnalysis:
    peak_dbfs: float
    rms_dbfs: float
    a_weighted_rms_dbfs: float
    tone_power_fraction: float
    clipped: bool
    ok: bool
    reasons: tuple[str, ...]

    def to_dict(self) -> dict:
        return {
            "peak_dbfs": self.peak_dbfs if math.isfinite(self.peak_dbfs) else None,
            "rms_dbfs": self.rms_dbfs if math.isfinite(self.rms_dbfs) else None,
            "a_weighted_rms_dbfs": (
                self.a_weighted_rms_dbfs if math.isfinite(self.a_weighted_rms_dbfs) else None
            ),
            "tone_power_fraction": self.tone_power_fraction,
            "clipped": self.clipped,
            "ok": self.ok,
            "reasons": list(self.reasons),
        }


def analyse_tone(
    capture: np.ndarray,
    sample_rate: int,
    frequency_hz: float = TONE_HZ,
    target_peak_dbfs: tuple[float, float] = TARGET_PEAK_DBFS,
) -> ToneAnalysis:
    """Level, clipping, and whether the capture is actually the tone rather than the room.

    The tone-fraction test is the one that catches the failure the operator will otherwise not
    notice: a microphone pointed at nothing, a muted output, or a capture of a different device's
    audio all give a plausible-looking level and almost no power at 1 kHz.
    """
    signal = np.asarray(capture, dtype=np.float64)
    if signal.ndim == 2:
        signal = signal.mean(axis=1)
    if signal.size < sample_rate // 10:
        raise CalibrationError("calibration capture is shorter than 100 ms")
    peak = peak_dbfs(signal)
    segment = min(signal.size, max(int(round(0.5 * sample_rate)), 256))
    freqs, density = welch(signal, fs=sample_rate, nperseg=segment, scaling="density")
    total = float(np.sum(density))
    in_band = float(np.sum(density[np.abs(freqs - frequency_hz) <= TONE_BANDWIDTH_HZ]))
    fraction = in_band / total if total > 0.0 else 0.0

    reasons: list[str] = []
    clipped = math.isfinite(peak) and peak > CLIPPING_PEAK_DBFS
    if clipped:
        reasons.append(f"capture peaks at {peak:.1f} dBFS, above the {CLIPPING_PEAK_DBFS} dBFS clipping guard")
    if fraction < MIN_TONE_FRACTION:
        reasons.append(
            f"only {fraction * 100:.1f}% of captured power sits within {TONE_BANDWIDTH_HZ:.0f} Hz of "
            f"{frequency_hz:.0f} Hz; the microphone is not hearing the tone"
        )
    low, high = target_peak_dbfs
    if math.isfinite(peak) and peak < low:
        reasons.append(f"capture peaks at {peak:.1f} dBFS, below the {low} dBFS target; raise capture gain")
    if math.isfinite(peak) and not clipped and peak > high:
        reasons.append(f"capture peaks at {peak:.1f} dBFS, above the {high} dBFS target; lower capture gain")
    return ToneAnalysis(
        peak_dbfs=peak,
        rms_dbfs=rms_dbfs(signal),
        a_weighted_rms_dbfs=a_weighted_rms_dbfs(signal, sample_rate),
        tone_power_fraction=fraction,
        clipped=clipped,
        ok=not reasons,
        reasons=tuple(reasons),
    )


def calibration_from_tone(
    analysis: ToneAnalysis,
    meter_reading_dba: float | None,
    meter: str | None,
) -> tuple[SplCalibration | None, str | None]:
    """Turn a verified tone capture plus an operator meter reading into an SPL anchor.

    Returns (None, reason) when no meter reading was supplied. The rig will then report every SPL
    field as null for that room rather than deriving a number from a reference it does not have.
    """
    if meter_reading_dba is None:
        return None, (
            "no --meter-dba reading supplied; this room's captures will report spl_dba as null. "
            "A dBFS reading has no absolute reference and spec section 10 exists because the "
            "published field reports SPL nowhere."
        )
    if not analysis.ok:
        return None, "calibration tone did not verify: " + "; ".join(analysis.reasons)
    if not math.isfinite(analysis.a_weighted_rms_dbfs):
        return None, "calibration capture has no measurable A-weighted level"
    return (
        SplCalibration(
            meter_reading_dba=float(meter_reading_dba),
            mic_rms_dbfs=float(analysis.a_weighted_rms_dbfs),
            measured_at=utc_now(),
            meter=meter or "unspecified sound level meter",
        ),
        None,
    )
