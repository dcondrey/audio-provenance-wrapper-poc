"""Band and grid arithmetic the Rust side has to match exactly."""

import pytest

from apw_watermark_neural.config import StftConfig, load_config
from apw_watermark_neural.model import NeuralWatermark
from apw_watermark_neural.qmark import QGrid


def test_band_arithmetic():
    stft = StftConfig()
    stft.validate()
    assert stft.band_bins == 320
    assert stft.bin_hz == pytest.approx(23.4375)
    assert stft.band_low_hz == pytest.approx(210.9375)
    assert stft.band_high_hz == pytest.approx(7687.5)
    assert stft.frames_per_second == pytest.approx(93.75)
    assert stft.band_bins % 16 == 0


def test_four_frequency_halvings_land_on_twenty_rows():
    assert StftConfig().band_bins // 16 == 20


def test_decoder_preserves_time_resolution():
    config = load_config()
    model = NeuralWatermark(config.encoder, config.decoder, config.payload, config.stft)
    import torch

    frames = 97
    presence, bit_logit, trace = model.decoder(torch.randn(1, 1, 320, frames))
    assert presence.shape == (1, frames)
    assert bit_logit.shape == (1, config.payload.message_bits)
    assert trace.shape == (1, config.payload.message_bits, frames)


def test_q_grid_matches_apw_watermark_geometry():
    grid = QGrid(load_config().q, 48_000)
    assert len(grid.active_pairs) == 14
    assert grid.edges_hz[0] == pytest.approx(861.3, abs=0.1)
    assert grid.edges_hz[-1] == pytest.approx(4306.6, abs=0.1)
