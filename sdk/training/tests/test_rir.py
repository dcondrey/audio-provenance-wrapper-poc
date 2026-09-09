"""The three ways an image-source synthesizer silently lies about the room it built."""

import numpy as np
import pytest

from apw_watermark_neural.rir.image_source import (
    ImageSourceRoom,
    eyring_absorption,
    measure_rt60,
    synthesise_rir,
)

SMALL = (2.8, 2.4, 2.3)


def _surface(dims):
    return 2.0 * (dims[0] * dims[1] + dims[1] * dims[2] + dims[0] * dims[2])


def test_eyring_stays_physical_where_sabine_does_not():
    volume = SMALL[0] * SMALL[1] * SMALL[2]
    surface = _surface(SMALL)
    for rt60 in (0.12, 0.18, 0.25):
        sabine = 0.161 * volume / (surface * rt60)
        eyring = eyring_absorption(volume, surface, rt60)
        assert 0.0 < eyring < 1.0
        if sabine >= 1.0:
            assert eyring < 1.0


@pytest.mark.parametrize("target", [0.25, 0.40, 0.60])
def test_measured_rt60_matches_the_requested_rt60(target):
    room = ImageSourceRoom(
        room_dims_m=(6.0, 5.0, 3.0), source_m=(3.0, 2.0, 1.5), receiver_m=(3.0, 3.0, 1.5),
        rt60_seconds=target, max_order=10,
    )
    pair = synthesise_rir(room, 48_000, 1.5, np.random.default_rng(3))
    assert pair.measured_rt60_seconds is not None
    assert pair.measured_rt60_seconds == pytest.approx(target, rel=0.05)


def test_direct_and_late_are_separable_so_drr_is_controllable():
    room = ImageSourceRoom(
        room_dims_m=(5.0, 4.0, 2.8), source_m=(2.5, 1.5, 1.4), receiver_m=(2.5, 2.5, 1.4),
        rt60_seconds=0.35, max_order=10,
    )
    pair = synthesise_rir(room, 48_000, 1.0, np.random.default_rng(5))
    assert pair.direct.any() and pair.late.any()
    for target in (-6.0, 0.0, 8.0, 18.0):
        combined = pair.at_drr(target)
        assert np.isfinite(combined).all()
        assert float((combined**2).sum()) == pytest.approx(1.0, rel=1e-4)


def test_fractional_delay_is_not_sample_rounded():
    """A nearest-sample synthesizer puts the direct tap on an integer index with nothing beside it.
    Windowed-sinc placement leaves the neighbouring taps non-zero, which is what removes the comb
    artifact a learned encoder would otherwise key on."""
    room = ImageSourceRoom(
        room_dims_m=(5.0, 4.0, 2.8), source_m=(2.5, 1.5, 1.4), receiver_m=(2.5, 2.5, 1.4),
        rt60_seconds=0.3, max_order=2,
    )
    pair = synthesise_rir(room, 48_000, 0.5, np.random.default_rng(1))
    peak = int(np.argmax(np.abs(pair.direct)))
    assert abs(pair.direct[peak - 1]) > 0.0
    assert abs(pair.direct[peak + 1]) > 0.0
