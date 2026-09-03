"""The sweep and deconvolution mathematics, against synthetic impulse responses with known answers.

This is the part of the rig that is fully verifiable with no hardware present, so it is verified
here rather than assumed to work when the campaign runs.
"""

from __future__ import annotations

import numpy as np
import pytest
from scipy.signal import fftconvolve

from capture_rig.acoustics import drr_db, rt60_t30
from capture_rig.signals import SweepError, exponential_sweep, sweep_stimulus
from capture_rig.sweep import deconvolve, extract_response, measure_response

from .synthetic import decaying_ir, tap_ir

SAMPLE_RATE = 48000


def test_inverse_filter_deconvolves_the_sweep_to_a_unit_impulse():
    f_start = 20.0
    pair = exponential_sweep(SAMPLE_RATE, 2.0, f_start, 20000.0)
    impulse = fftconvolve(pair.sweep, pair.inverse, mode="full")
    assert np.argmax(np.abs(impulse)) == pair.peak_index
    assert impulse[pair.peak_index] == pytest.approx(1.0, abs=1e-9)

    # The deconvolution returns a BAND-LIMITED impulse, so its skirt rings for about 1/f_start
    # seconds. Concentration is asserted over that support, not over a delta's.
    support = int(round(SAMPLE_RATE / f_start))
    total = float(np.dot(impulse, impulse))
    inside = impulse[pair.peak_index - support : pair.peak_index + support]
    assert float(np.dot(inside, inside)) / total > 0.90
    outside = np.abs(np.delete(impulse, slice(pair.peak_index - support, pair.peak_index + support)))
    assert outside.max() < 0.15


def test_deconvolution_recovers_discrete_taps_and_the_bulk_delay():
    """The extracted response must equal the true response BAND-LIMITED BY THE SWEEP, not a delta train.

    Comparing against the true taps alone would fail for a correct deconvolution, because a swept-sine
    measurement can only ever recover the response within the sweep's own band. The reference here is
    the true impulse response convolved with the sweep pair's own impulse, which isolates
    deconvolution correctness from band limiting.
    """
    pair = exponential_sweep(SAMPLE_RATE, 4.0, 100.0, 20000.0)
    taps = [(0.0, 1.0), (0.020, 0.50), (0.050, -0.35), (0.090, 0.25)]
    ir = tap_ir(SAMPLE_RATE, taps)
    latency = 1234
    scale = 10.0 ** (-6.0 / 20.0)
    recording = np.concatenate([np.zeros(latency), fftconvolve(sweep_stimulus(pair, 1.0, -6.0), ir)])

    response = measure_response(recording, pair, ir_seconds=0.20, pre_seconds=0.005)

    assert response.bulk_delay_seconds == pytest.approx(latency / SAMPLE_RATE, abs=2.0 / SAMPLE_RATE)

    reference = fftconvolve(ir, fftconvolve(pair.sweep, pair.inverse, mode="full")) * scale
    reference_peak = int(np.argmax(np.abs(reference)))
    length = response.ir.size - response.direct_index
    expected = reference[reference_peak : reference_peak + length]
    actual = response.ir[response.direct_index : response.direct_index + expected.size]
    assert np.max(np.abs(actual - expected)) < 1e-6

    for delay, amplitude in taps:
        index = response.direct_index + int(round(delay * SAMPLE_RATE))
        assert np.sign(response.ir[index]) == np.sign(amplitude)
        assert abs(response.ir[index]) == pytest.approx(abs(amplitude) * scale, rel=0.15)


def test_deconvolution_recovers_a_designed_rt60_and_drr():
    pair = exponential_sweep(SAMPLE_RATE, 6.0, 30.0, 20000.0)
    designed_rt60 = 0.45
    ir = decaying_ir(SAMPLE_RATE, designed_rt60, length_seconds=1.2)
    recording = fftconvolve(sweep_stimulus(pair, 2.0, -6.0), ir)

    response = measure_response(recording, pair, ir_seconds=1.0, pre_seconds=0.005)
    measured = rt60_t30(response.ir, SAMPLE_RATE)

    assert measured.seconds is not None, measured.reason
    assert measured.seconds == pytest.approx(designed_rt60, rel=0.05)
    assert measured.fit_r_squared is not None and measured.fit_r_squared > 0.95

    # DRR is compared against the response BAND-LIMITED BY THE SWEEP, for the same reason the tap
    # test is: an ideal single-sample direct arrival spreads over roughly 1/f_start seconds once
    # band-limited, so several dB of its energy leaves the +/- 2.5 ms direct window. A real
    # transducer band-limits the direct arrival too; an ideal delta is the unrealistic case.
    band_limited = fftconvolve(ir, fftconvolve(pair.sweep, pair.inverse, mode="full"))
    reference_drr = drr_db(band_limited[int(np.argmax(np.abs(band_limited))) - 240 :], SAMPLE_RATE)
    assert reference_drr is not None
    assert drr_db(response.ir, SAMPLE_RATE) == pytest.approx(reference_drr, abs=1.0)


def test_harmonic_packets_land_at_farina_pre_arrival_times():
    """A memoryless odd nonlinearity puts its k-th harmonic at T*ln(k)/ln(f2/f1) BEFORE the linear IR.

    This is the property that lets one sweep measure both the room and the loudspeaker's distortion,
    which is what parameterises distortion-layer stage D4 with a measured figure. It also proves the
    deconvolution is linear rather than circular: a circular one would wrap these packets, which
    arrive before time zero, into the reverberant tail.
    """
    pair = exponential_sweep(SAMPLE_RATE, 6.0, 50.0, 18000.0)
    stimulus = sweep_stimulus(pair, 1.0, 0.0) * 0.9
    distorted = stimulus - 0.25 * stimulus**3
    deconvolved = deconvolve(distorted, pair)
    response = extract_response(deconvolved, pair, ir_seconds=0.2, pre_seconds=0.005)

    linear_peak = int(np.argmax(np.abs(deconvolved)))
    third = pair.harmonic_arrival_offset_seconds(3)
    expected = linear_peak - int(round(third * SAMPLE_RATE))
    window = int(round(0.02 * SAMPLE_RATE))
    found = expected - window + int(np.argmax(np.abs(deconvolved[expected - window : expected + window])))
    assert abs(found - expected) < int(round(0.002 * SAMPLE_RATE))

    packets = {p.order: p for p in response.harmonics}
    assert packets[3].peak_abs > 10.0 * packets[2].peak_abs
    assert response.thd_percent is not None and response.thd_percent > 1.0


def test_a_clean_linear_path_reports_negligible_harmonic_distortion():
    pair = exponential_sweep(SAMPLE_RATE, 4.0, 50.0, 18000.0)
    recording = fftconvolve(sweep_stimulus(pair, 1.0, -6.0), tap_ir(SAMPLE_RATE, [(0.0, 1.0), (0.005, 0.4)]))
    response = measure_response(recording, pair, ir_seconds=0.15, pre_seconds=0.005)
    assert response.thd_percent is not None and response.thd_percent < 1.0


@pytest.mark.parametrize(
    "kwargs",
    [
        {"duration_seconds": 0.0},
        {"f_start_hz": 20000.0, "f_end_hz": 20.0},
        {"f_end_hz": 40000.0},
    ],
)
def test_unrealisable_sweeps_are_refused(kwargs):
    base = {"sample_rate": SAMPLE_RATE, "duration_seconds": 2.0, "f_start_hz": 20.0, "f_end_hz": 20000.0}
    with pytest.raises(SweepError):
        exponential_sweep(**{**base, **kwargs})
