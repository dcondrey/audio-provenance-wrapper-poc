"""Room acoustics measured from an impulse response: RT60, DRR, and A-weighted level.

Kill criterion K0 in docs/WATERMARK_N_SPEC.md section 12.3 is defined on MEASURED RT60 and MEASURED
direct-to-reverberant ratio, so the conventions below are the ones K0 is evaluated under and they are
named rather than implied.

RT60: Schroeder backward integration, T30 fitted between -5 dB and -35 dB and extrapolated to 60 dB,
with a Lundeby-style noise truncation before integration. Matches the T30 convention of
`training//apw-watermark-neural/rir/image_source.py::measure_rt60`, with the truncation added because a
measured response has a noise floor that a synthesised one does not.

DRR: the ACE Challenge convention, a +/- 2.5 ms window around the direct peak against everything
after it. IMPORTANT: this is NOT the same split as `RirPair.measured_drr_db()` in the training tree,
which separates image-source order 0 from orders >= 1. An order-based split is unavailable on a
measured response because the image orders are not observable. The two conventions differ; the
divergence is measured by `tests/test_acoustics.py::test_ace_and_order_split_drr_are_named_and_close`
and reported in every measurement record as `drr_convention`.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np
from scipy.signal import butter, sosfiltfilt, welch

DRR_CONVENTION = "ace_direct_window_2p5ms"
DIRECT_WINDOW_SECONDS = 0.0025
RT60_CONVENTION = "schroeder_t30_minus5_to_minus35_lundeby_truncated"
ANALYSIS_BAND_HZ = (100.0, 8000.0)
MID_BAND_KEYS = ("500", "1000")
OCTAVE_BAND_CENTRES_HZ = (125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0)
MIN_DECAY_FIT_R_SQUARED = 0.95


class AcousticsError(ValueError):
    """An acoustic quantity was requested from a signal that cannot support it."""


@dataclass(frozen=True)
class Rt60Result:
    seconds: float | None
    fit_r_squared: float | None
    truncation_seconds: float | None
    decay_range_db: float
    reason: str | None
    band_hz: tuple[float, float] | None = None
    noise_limited: bool = False
    poor_fit: bool = False

    @property
    def trustworthy(self) -> bool:
        return self.seconds is not None and not self.noise_limited and not self.poor_fit

    def to_dict(self) -> dict:
        return {
            "seconds": self.seconds,
            "noise_limited": self.noise_limited,
            "poor_fit": self.poor_fit,
            "trustworthy": self.trustworthy,
            "fit_r_squared": self.fit_r_squared,
            "truncation_seconds": self.truncation_seconds,
            "decay_range_db": self.decay_range_db,
            "convention": RT60_CONVENTION,
            "band_hz": list(self.band_hz) if self.band_hz else None,
            "reason": self.reason,
        }


def bandlimit(signal: np.ndarray, sample_rate: int, band_hz: tuple[float, float] | None) -> np.ndarray:
    """Fourth-order zero-phase band-pass.

    IMPORTANT: a swept-sine deconvolution returns a BAND-LIMITED impulse, not a delta, and the
    residual skirt below the sweep's start frequency rings for 1/f_start seconds. Left in, it
    dominates the Schroeder tail: on a synthetic response designed for RT60 0.45 s, broadband T30 on
    the raw deconvolution reads 1.39 s while the same response band-limited to 100 Hz - 8 kHz reads
    0.43 s. Every RT60 and DRR this module reports is therefore band-limited, and the band travels
    with the number.
    """
    if band_hz is None:
        return np.asarray(signal, dtype=np.float64)
    low, high = band_hz
    nyquist = sample_rate / 2.0
    if not 0.0 < low < high:
        raise AcousticsError(f"band {band_hz} is not an increasing positive pair")
    high = min(high, nyquist * 0.99)
    if low >= high:
        raise AcousticsError(f"band {band_hz} does not fit below Nyquist {nyquist}")
    sos = butter(4, [low, high], btype="bandpass", fs=sample_rate, output="sos")
    return sosfiltfilt(sos, np.asarray(signal, dtype=np.float64))


def _as_mono(signal: np.ndarray) -> np.ndarray:
    array = np.asarray(signal, dtype=np.float64)
    if array.ndim == 2:
        array = array.mean(axis=1)
    if array.ndim != 1:
        raise AcousticsError("expected a mono or (frames, channels) signal")
    if array.size == 0:
        raise AcousticsError("empty signal")
    return array


def _lundeby_truncation(energy: np.ndarray, sample_rate: int) -> tuple[int, float]:
    """Index at which the response has decayed into its own noise floor, and that floor's power."""
    block = max(int(round(0.01 * sample_rate)), 1)
    tail_start = max(energy.size - max(energy.size // 10, block), 0)
    noise_power = float(np.mean(energy[tail_start:])) if energy.size > tail_start else 0.0
    if noise_power <= 0.0:
        return energy.size, 0.0
    usable = (energy.size // block) * block
    if usable < block:
        return energy.size, noise_power
    blocks = energy[:usable].reshape(-1, block).mean(axis=1)
    above = np.nonzero(blocks > noise_power * 10.0)[0]
    if above.size == 0:
        return energy.size, noise_power
    return int((above[-1] + 1) * block), noise_power


def rt60_t30(
    ir: np.ndarray,
    sample_rate: int,
    band_hz: tuple[float, float] | None = ANALYSIS_BAND_HZ,
) -> Rt60Result:
    """Schroeder backward integration with noise truncation; T30 extrapolated to 60 dB.

    Band-limited to `band_hz` first. Pass None only for a response that is already band-limited, such
    as one octave band of a filter bank.
    """
    if sample_rate <= 0:
        raise AcousticsError("sample_rate must be positive")
    signal = bandlimit(_as_mono(ir), sample_rate, band_hz)
    onset = int(np.argmax(np.abs(signal)))
    energy = signal[onset:] ** 2
    if energy.size < sample_rate // 100 or float(energy.sum()) <= 0.0:
        return Rt60Result(None, None, None, 0.0, "impulse response carries no energy after its peak", band_hz)

    cut, noise_power = _lundeby_truncation(energy, sample_rate)
    truncated = np.maximum(energy[:cut] - noise_power, 0.0)
    if float(truncated.sum()) <= 0.0:
        return Rt60Result(None, None, cut / sample_rate, 0.0, "response is entirely below its noise floor", band_hz)

    integral = truncated[::-1].cumsum()[::-1]
    curve = 10.0 * np.log10(np.maximum(integral / integral[0], 1e-30))
    decay_range = float(curve[0] - curve[-1])

    start_hits = np.nonzero(curve <= -5.0)[0]
    stop_hits = np.nonzero(curve <= -35.0)[0]
    if start_hits.size == 0 or stop_hits.size == 0:
        return Rt60Result(
            None,
            None,
            cut / sample_rate,
            decay_range,
            f"decay never reached -35 dB (reached {decay_range:.1f} dB); "
            "lengthen the silent tail after the sweep or raise playback level",
        )
    start, stop = int(start_hits[0]), int(stop_hits[0])
    if stop - start < 8:
        return Rt60Result(None, None, cut / sample_rate, decay_range, "decay between -5 and -35 dB is too short to fit", band_hz)

    times = np.arange(start, stop, dtype=np.float64) / sample_rate
    values = curve[start:stop]
    slope, intercept = np.polyfit(times, values, 1)
    if slope >= 0.0:
        return Rt60Result(None, None, cut / sample_rate, decay_range, "fitted decay slope is not negative", band_hz)
    residual = values - (slope * times + intercept)
    variance = float(np.var(values))
    r_squared = 1.0 - float(np.var(residual)) / variance if variance > 0.0 else None
    seconds = float(-60.0 / slope)
    truncation = cut / sample_rate
    concerns: list[str] = []
    # A response whose usable decay ends well before one RT60 has elapsed was measured into its own
    # noise floor. The number is still the best fit available, but it is biased and the record says so
    # rather than letting a short silent tail or a low playback level pass as a room property.
    noise_limited = truncation < seconds * 0.5
    if noise_limited:
        concerns.append(
            f"noise-limited: usable decay ends at {truncation:.3f} s, under half the fitted RT60 "
            f"{seconds:.3f} s. Lengthen the sweep's silent tail or raise playback level."
        )
    # A curved decay is not a reverberation time. The commonest cause in this rig is a sweep too
    # short or too narrow to resolve the analysis band: a 3 s 50 Hz-20 kHz sweep reads 0.63 s on a
    # response designed for 0.30 s, at r^2 0.84, where a 10 s 20 Hz-20 kHz sweep reads 0.30 s.
    poor_fit = r_squared is not None and r_squared < MIN_DECAY_FIT_R_SQUARED
    if poor_fit:
        concerns.append(
            f"decay fit r^2 is {r_squared:.3f}, below {MIN_DECAY_FIT_R_SQUARED}. The decay is not "
            "log-linear over -5 to -35 dB; lengthen the sweep or widen its frequency range."
        )
    return Rt60Result(
        seconds, r_squared, truncation, decay_range, "; ".join(concerns) or None, band_hz, noise_limited, poor_fit
    )


def octave_band_rt60(ir: np.ndarray, sample_rate: int) -> dict[str, dict]:
    """T30 per octave band. Bands whose upper edge exceeds Nyquist are skipped, not fabricated."""
    signal = _as_mono(ir)
    results: dict[str, dict] = {}
    for centre in OCTAVE_BAND_CENTRES_HZ:
        low = centre / math.sqrt(2.0)
        high = centre * math.sqrt(2.0)
        if high >= sample_rate / 2.0:
            continue
        band = bandlimit(signal, sample_rate, (low, high))
        results[f"{int(centre)}"] = rt60_t30(band, sample_rate, band_hz=None).to_dict()
    return results


def mid_frequency_rt60(octave_bands: dict[str, dict]) -> float | None:
    """Tmid, the mean of the 500 Hz and 1 kHz octave-band T30 values. None when either is missing."""
    values = [octave_bands.get(key, {}).get("seconds") for key in MID_BAND_KEYS]
    if any(v is None for v in values):
        return None
    return float(np.mean(values))


def drr_db(
    ir: np.ndarray,
    sample_rate: int,
    direct_window_seconds: float = DIRECT_WINDOW_SECONDS,
    band_hz: tuple[float, float] | None = ANALYSIS_BAND_HZ,
) -> float | None:
    """Direct-to-reverberant ratio, ACE convention. None when there is no reverberant energy.

    Band-limited for the same reason RT60 is: the sub-100 Hz skirt of a swept-sine deconvolution
    spreads the direct arrival far past the +/- 2.5 ms window and biases the ratio downward.
    """
    signal = bandlimit(_as_mono(ir), sample_rate, band_hz)
    peak = int(np.argmax(np.abs(signal)))
    half = max(int(round(direct_window_seconds * sample_rate)), 1)
    lo, hi = max(peak - half, 0), min(peak + half + 1, signal.size)
    direct = float(np.dot(signal[lo:hi], signal[lo:hi]))
    late_slice = signal[hi:]
    late = float(np.dot(late_slice, late_slice))
    if direct <= 0.0 or late <= 0.0:
        return None
    return 10.0 * math.log10(direct / late)


A_WEIGHTING_POLES_HZ = (20.598997, 107.65265, 737.86223, 12194.217)


def a_weighting_response(frequencies_hz: np.ndarray) -> np.ndarray:
    """Exact IEC 61672 A-weighting magnitude, normalised to unity at 1 kHz.

    KISS: the analytic magnitude is evaluated per frequency rather than realised as a digital filter.
    A bilinear-transformed A-weighting is 1.2 dB low at 10 kHz at 48 kHz sample rate, which is
    outside IEC class 1 tolerance, and every use here is a power measurement over a spectrum.
    """
    f1, f2, f3, f4 = A_WEIGHTING_POLES_HZ

    def magnitude(freq: np.ndarray) -> np.ndarray:
        s = 2j * np.pi * np.asarray(freq, dtype=np.float64)
        numerator = s**4
        denominator = (
            (s + 2 * np.pi * f1) ** 2
            * (s + 2 * np.pi * f2)
            * (s + 2 * np.pi * f3)
            * (s + 2 * np.pi * f4) ** 2
        )
        return np.abs(numerator / denominator)

    return magnitude(frequencies_hz) / float(magnitude(np.array([1000.0]))[0])


def a_weighted_rms_dbfs(signal: np.ndarray, sample_rate: int) -> float:
    """A-weighted RMS in dB relative to a full-scale sine. Not an SPL; see `dba_from_dbfs`."""
    mono = _as_mono(signal)
    if sample_rate <= 2000:
        raise AcousticsError("A-weighting needs a sample rate above 2 kHz to be meaningful")
    segment = min(mono.size, max(int(round(0.5 * sample_rate)), 256))
    freqs, density = welch(mono, fs=sample_rate, nperseg=segment, scaling="density")
    bin_width = float(freqs[1] - freqs[0]) if freqs.size > 1 else float(sample_rate)
    weighted = float(np.sum(density * a_weighting_response(freqs) ** 2) * bin_width)
    if weighted <= 0.0:
        return -math.inf
    return 10.0 * math.log10(weighted)


def rms_dbfs(signal: np.ndarray) -> float:
    mono = _as_mono(signal)
    rms = float(np.sqrt(np.mean(mono**2)))
    if rms <= 0.0:
        return -math.inf
    return 20.0 * math.log10(rms)


def dba_from_dbfs(measured_dbfs: float, reference_dbfs: float, reference_dba: float) -> float:
    """Convert a capture-side dBFS reading into dB SPL(A) using one operator meter reading.

    IMPORTANT: without a meter reading there is no absolute reference and the caller MUST report the
    level as absent rather than inventing one. Spec section 10's whole indictment of the published
    field is that SPL and background level go unreported; a fabricated figure is worse than a missing
    one.
    """
    if not math.isfinite(measured_dbfs) or not math.isfinite(reference_dbfs):
        raise AcousticsError("dBFS readings must be finite to be referenced to a meter")
    return measured_dbfs - reference_dbfs + reference_dba
