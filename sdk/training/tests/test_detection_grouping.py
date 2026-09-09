"""What `Detector.detect` reports when windows disagree, and what it reports when nothing decodes.

The branch that picks a locator out of per-window decodes is the only place in the harness where
several CRC-valid candidates are reduced to one answer, and `evaluate.py` on an untrained model
never reaches it: every window returns None, so the whole grouping path is unexercised by the
smoke run. These cases drive the real `Detector.detect` over a real `SpectralFront` and the real
`MessageCodec`, with only the decoder trunk replaced by a scripted one, and pin four outcomes:
majority payload wins, the multi/single confidence split follows non-overlapping window buckets,
and a failed detection reports an honest empty result rather than raising.
"""

import dataclasses
from typing import cast

import numpy as np
import pytest
import torch
from torch import nn

from apw_watermark_neural.config import load_config
from apw_watermark_neural.model import NeuralWatermark
from apw_watermark_neural.payload import Message, MessageCodec
from apw_watermark_neural.pipeline import Detector, FrozenThresholds
from apw_watermark_neural.stft import SpectralFront

CLIP_SECONDS = 6.0
LOGIT = 6.0


class ScriptedDecoder(nn.Module):
    """Returns exactly the presence and per-frame bit traces a case asks for.

    Windows are made non-overlapping in the config below, and each trace cell is constant across a
    whole window, so the pooled readout inside one window is that window's payload and nothing
    bleeds across a boundary.
    """

    def __init__(self, cells: list[np.ndarray | None], present: list[bool], cell_frames: int):
        super().__init__()
        self.log_tau = nn.Parameter(torch.zeros(()))
        self.cells = cells
        self.present = present
        self.cell_frames = cell_frames

    def forward(self, log_magnitude: torch.Tensor):
        frames = log_magnitude.shape[-1]
        presence = torch.full((1, frames), -8.0)
        traces = torch.zeros((1, 56, frames))
        for index, (bits, present) in enumerate(zip(self.cells, self.present, strict=True)):
            start = index * self.cell_frames
            stop = min(start + self.cell_frames, frames)
            if start >= frames:
                break
            if present:
                presence[:, start:stop] = 8.0
            if bits is not None:
                column = torch.from_numpy(bits.astype(np.float32) * 2.0 - 1.0) * LOGIT
                traces[0, :, start:stop] = column.unsqueeze(-1)
        return presence, torch.zeros((1, 56)), traces


class _Model(nn.Module):
    def __init__(self, decoder: ScriptedDecoder) -> None:
        super().__init__()
        self.decoder = decoder


def _thresholds(threshold: float) -> FrozenThresholds:
    return FrozenThresholds(
        presence_threshold=threshold,
        target_false_positive_rate=1e-3,
        calibration_trials=0,
        measured_false_positive_rate=0.0,
        calibrated_at="1970-01-01T00:00:00Z",
        channels=(),
        config_digest="test",
        weights_digest="test",
        calibration_clip_seconds=CLIP_SECONDS,
        presence_windows_per_clip=0,
    )


def _detector(cells, present, threshold: float = 0.5, window_seconds: float = 1.0,
              hop_seconds: float = 1.0):
    config = load_config()
    front = SpectralFront(config.stft)
    # Default is hop == window, so every `start // window_frames` bucket is distinct and the
    # multi/single split is a function of how many cells decoded and nothing else. The overlap case
    # passes a shorter hop to make two accepted windows share one bucket.
    detector_config = dataclasses.replace(
        config.detector,
        window_seconds=window_seconds,
        window_hop_seconds=hop_seconds,
        presence_window_seconds=1.0,
        presence_smooth_seconds=0.05,
        min_presence_span_seconds=0.5,
        min_locator_seconds=1.0,
    )
    cell_frames = int(1.0 * config.stft.frames_per_second)
    decoder = ScriptedDecoder(cells, present, cell_frames)
    codec = MessageCodec(config.payload)
    audio = torch.zeros(1, int(CLIP_SECONDS * config.stft.sample_rate))
    # The detector reads only `model.decoder`; the stub supplies exactly that surface and the
    # cast is what makes the substitution explicit rather than widening the real signature.
    detect = Detector(cast(NeuralWatermark, _Model(decoder)), front, detector_config, codec)
    return detect.detect(audio, _thresholds(threshold)), codec


@pytest.fixture(scope="module")
def payloads() -> tuple[tuple[Message, np.ndarray], tuple[Message, np.ndarray]]:
    codec = MessageCodec()
    rng = np.random.default_rng(20260831)
    first, second = codec.random_message(rng), codec.random_message(rng)
    assert first != second
    return (first, codec.encode(first)), (second, codec.encode(second))


def test_majority_payload_wins_and_reports_the_windows_that_carried_it(payloads):
    (message_a, bits_a), (message_b, bits_b) = payloads
    # Three windows carry A, two carry B, one carries nothing decodable.
    cells = [bits_a, bits_a, bits_a, bits_b, bits_b, None]
    detection, _ = _detector(cells, [True] * 6)

    assert detection.locator == message_a
    assert detection.bits is not None
    assert np.array_equal(detection.bits, bits_a)
    assert detection.bits_corrected == 0
    assert detection.confidence_class == "locator_multi"


def test_one_decoding_window_is_locator_single_not_multi(payloads):
    (message_a, bits_a), _ = payloads
    # Only the first cell is above the presence gate, so every later window's masked span falls
    # under min_presence_span_seconds and never reaches the codec.
    detection, _ = _detector([bits_a] + [None] * 5, [True] + [False] * 5)

    assert detection.locator == message_a
    assert detection.confidence_class == "locator_single"


def test_two_overlapping_windows_are_one_bucket_not_two(payloads):
    (message_a, bits_a), _ = payloads
    # Two-second windows on a one-second hop. Presence covers only the first two seconds, so the
    # windows at frame 0 and frame 93 both decode and both fall in bucket 0. Overlapping reads of
    # the same audio are ONE observation; counting them as two would report `locator_multi` on
    # evidence that never repeated.
    detection, _ = _detector([bits_a] * 6, [True, True] + [False] * 4,
                             window_seconds=2.0, hop_seconds=1.0)

    assert detection.locator == message_a
    assert detection.confidence_class == "locator_single"


def test_no_crc_valid_window_with_presence_reports_presence_and_no_payload(payloads):
    detection, _ = _detector([None] * 6, [True] * 6)

    assert detection.presence_detected is True
    assert detection.locator is None
    assert detection.bits is None
    assert detection.bits_corrected == 0
    assert detection.confidence_class == "presence"


def test_silent_clip_reports_nothing_rather_than_raising(payloads):
    detection, _ = _detector([None] * 6, [False] * 6)

    assert detection.presence_detected is False
    assert detection.locator is None
    assert detection.bits is None
    assert detection.confidence_class == "none"
