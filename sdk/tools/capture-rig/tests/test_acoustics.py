"""RT60, DRR and A-weighting, against published values and against the training tree's conventions."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import numpy as np
import pytest

from capture_rig.acoustics import (
    ANALYSIS_BAND_HZ,
    a_weighted_rms_dbfs,
    a_weighting_response,
    dba_from_dbfs,
    drr_db,
    mid_frequency_rt60,
    octave_band_rt60,
    rms_dbfs,
    rt60_t30,
)

from capture_rig.signals import exponential_sweep, sweep_stimulus
from capture_rig.sweep import measure_response
from scipy.signal import fftconvolve

from .synthetic import decaying_ir

SAMPLE_RATE = 48000
TRAINING_IMAGE_SOURCE = Path("/Volumes/A/audio-provenance/sdk/training//apw-watermark-neural/rir/image_source.py")

# IEC 61672-1 A-weighting, dB relative to 1 kHz.
IEC_61672_A_WEIGHTING_DB = {31.5: -39.4, 100.0: -19.1, 1000.0: 0.0, 10000.0: -2.5, 20000.0: -9.3}


def _load_training_image_source():
    """Load the training tree's image-source module by path.

    By path rather than by import because `apw_watermark_neural.rir.__init__` pulls in the dataset module,
    which needs torch, and this tool must not carry a torch dependency to cross-check twenty lines of
    Schroeder integration.
    """
    if not TRAINING_IMAGE_SOURCE.exists():
        return None
    spec = importlib.util.spec_from_file_location("apw_watermark_neural_image_source", TRAINING_IMAGE_SOURCE)
    if spec is None or spec.loader is None:
        return None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize("frequency,expected_db", sorted(IEC_61672_A_WEIGHTING_DB.items()))
def test_a_weighting_matches_the_iec_61672_table(frequency, expected_db):
    measured = 20.0 * np.log10(a_weighting_response(np.array([frequency]))[0])
    assert measured == pytest.approx(expected_db, abs=0.15)


def test_a_weighted_level_of_a_tone_equals_its_table_weighting():
    t = np.arange(SAMPLE_RATE * 2, dtype=np.float64) / SAMPLE_RATE
    for frequency, expected_db in IEC_61672_A_WEIGHTING_DB.items():
        if frequency >= SAMPLE_RATE / 2:
            continue
        tone = np.sin(2.0 * np.pi * frequency * t)
        assert a_weighted_rms_dbfs(tone, SAMPLE_RATE) - rms_dbfs(tone) == pytest.approx(expected_db, abs=0.15)


def test_dba_conversion_is_a_pure_offset_from_the_meter_reading():
    assert dba_from_dbfs(-30.0, -20.0, 74.0) == pytest.approx(64.0)
    assert dba_from_dbfs(-20.0, -20.0, 74.0) == pytest.approx(74.0)


def test_rt60_recovers_a_designed_decay_and_its_octave_bands():
    ir = decaying_ir(SAMPLE_RATE, 0.35, length_seconds=1.0)
    result = rt60_t30(ir, SAMPLE_RATE)
    assert result.seconds == pytest.approx(0.35, rel=0.05)
    assert tuple(result.band_hz) == ANALYSIS_BAND_HZ

    bands = octave_band_rt60(ir, SAMPLE_RATE)
    assert set(bands) == {"125", "250", "500", "1000", "2000", "4000", "8000"}
    for name, band in bands.items():
        assert band["seconds"] == pytest.approx(0.35, rel=0.15), name
    assert mid_frequency_rt60(bands) == pytest.approx(0.35, rel=0.05)


def test_rt60_refuses_a_response_that_carries_no_energy():
    result = rt60_t30(np.zeros(SAMPLE_RATE), SAMPLE_RATE)
    assert result.seconds is None
    assert result.reason is not None


def test_rt60_flags_a_response_measured_into_its_own_noise_floor():
    """A short silent tail or a low playback level truncates the decay and biases RT60 low.

    Schroeder integration over a truncated array ALWAYS produces a full-looking decay curve, so the
    tool cannot detect this by asking whether the curve reached -35 dB. It detects it by comparing
    where the usable decay ends against the RT60 it fitted.
    """
    long_decay = decaying_ir(SAMPLE_RATE, 0.9, length_seconds=2.0)
    noise = np.random.default_rng(11).standard_normal(long_decay.size) * 3e-3
    truncated = (long_decay + noise)[: int(0.25 * SAMPLE_RATE)]

    clean = rt60_t30(long_decay, SAMPLE_RATE)
    assert clean.seconds == pytest.approx(0.9, rel=0.05)
    assert not clean.noise_limited

    limited = rt60_t30(truncated, SAMPLE_RATE)
    assert limited.noise_limited
    assert not limited.trustworthy
    assert limited.reason is not None and "noise-limited" in limited.reason


def test_rt60_flags_a_decay_that_is_not_log_linear():
    """A sweep too short or too narrow for the analysis band gives a curved decay and a wrong number.

    Measured here rather than asserted: a 3 s 50 Hz - 20 kHz sweep reads 0.63 s on a response
    designed for 0.30 s, while a 10 s 20 Hz - 20 kHz sweep on the same response reads 0.30 s.
    """
    ir = decaying_ir(SAMPLE_RATE, 0.30, length_seconds=1.0, seed=100)
    stimulus_short = exponential_sweep(SAMPLE_RATE, 3.0, 50.0, 20000.0)
    stimulus_long = exponential_sweep(SAMPLE_RATE, 10.0, 20.0, 20000.0)

    short = rt60_t30(
        measure_response(fftconvolve(sweep_stimulus(stimulus_short, 2.0, -6.0), ir), stimulus_short,
                         ir_seconds=0.9, pre_seconds=0.005).ir,
        SAMPLE_RATE,
    )
    long = rt60_t30(
        measure_response(fftconvolve(sweep_stimulus(stimulus_long, 2.0, -6.0), ir), stimulus_long,
                         ir_seconds=0.9, pre_seconds=0.005).ir,
        SAMPLE_RATE,
    )

    assert short.poor_fit and not short.trustworthy
    assert short.seconds > 0.5
    assert long.trustworthy
    assert long.seconds == pytest.approx(0.30, rel=0.10)


def test_drr_tracks_the_direct_to_late_energy_split():
    quiet_tail = decaying_ir(SAMPLE_RATE, 0.3, tail_amplitude=0.05, length_seconds=0.8)
    loud_tail = decaying_ir(SAMPLE_RATE, 0.3, tail_amplitude=0.50, length_seconds=0.8)
    assert drr_db(quiet_tail, SAMPLE_RATE) > drr_db(loud_tail, SAMPLE_RATE) + 12.0


def test_rt60_agrees_with_the_training_tree_on_the_same_convention():
    """Both implementations fit T30 between -5 and -35 dB, so unbanded they must agree.

    The training tree owns its own copy and this tool owns this one, deliberately: a path dependency
    on a concurrently-edited package would break kill criterion K0's evaluator whenever that package
    is refactored. Duplication across an ownership boundary, cross-checked rather than assumed.
    """
    module = _load_training_image_source()
    if module is None:
        pytest.skip("training tree not present next to this tool")
    rng = np.random.default_rng(7)
    compared = 0
    for _ in range(6):
        room = module.sample_room(rng, (3.0, 3.0, 2.4), (6.0, 6.0, 3.0), (0.25, 0.7), (0.8, 1.2), 8, 9, 0.85)
        ir = module.synthesise_rir(room, SAMPLE_RATE).at_drr(6.0)
        theirs = module.measure_rt60(ir, SAMPLE_RATE)
        ours = rt60_t30(ir, SAMPLE_RATE, band_hz=None).seconds
        if theirs is None or ours is None:
            continue
        assert ours == pytest.approx(theirs, rel=0.02)
        compared += 1
    assert compared >= 3


def test_ace_and_order_split_drr_conventions_diverge_by_a_stated_offset():
    """The measured DRR convention is NOT the training tree's synthetic one, and this pins the gap.

    `RirPair.at_drr()` splits image-source order 0 from orders >= 1. That split is unobservable on a
    measured response, so this tool uses the ACE +/- 2.5 ms direct window instead. On the training
    tree's own synthesised responses the ACE figure reads roughly 2.4 dB above the order-split target
    and tracks it one-for-one. A DRR-parameterised training distribution therefore maps monotonically
    onto measured DRR, with an offset that must be stated wherever the two are compared.
    """
    module = _load_training_image_source()
    if module is None:
        pytest.skip("training tree not present next to this tool")
    rng = np.random.default_rng(7)
    room = module.sample_room(rng, (3.0, 3.0, 2.4), (6.0, 6.0, 3.0), (0.3, 0.5), (0.8, 1.2), 8, 9, 0.85)
    pair = module.synthesise_rir(room, SAMPLE_RATE)

    targets = [-6.0, 0.0, 6.0, 12.0, 18.0]
    measured = [drr_db(pair.at_drr(t), SAMPLE_RATE) for t in targets]
    assert all(value is not None for value in measured)
    assert all(b > a for a, b in zip(measured, measured[1:]))
    offsets = [m - t for m, t in zip(measured, targets, strict=True)]
    assert max(offsets) - min(offsets) < 2.0
    assert float(np.mean(offsets)) == pytest.approx(2.4, abs=1.5)
