"""Alignment bookkeeping: locating known clips inside a capture."""

from __future__ import annotations

import numpy as np

from capture_rig.alignment import locate_clip, segment_capture

SAMPLE_RATE = 48000


def _reverberant(signal: np.ndarray, rng) -> np.ndarray:
    ir = np.zeros(2048)
    ir[0] = 1.0
    ir[311] = 0.4
    ir[907] = -0.25
    ir[1600:] = rng.standard_normal(448) * 0.05
    return np.convolve(signal, ir)[: signal.size]


def test_a_clip_is_located_at_its_true_offset_through_a_reverberant_path():
    rng = np.random.default_rng(5)
    clip = rng.standard_normal(SAMPLE_RATE)
    offset = 20_137
    capture = np.zeros(offset + clip.size + SAMPLE_RATE)
    capture[offset : offset + clip.size] = _reverberant(clip * 0.3, rng)
    capture += rng.standard_normal(capture.size) * 0.005

    segment = locate_clip(capture, clip, SAMPLE_RATE, "clip000")
    assert segment.accepted
    assert abs(segment.start_sample - offset) <= 2
    assert segment.peak_to_sidelobe > 3.0


def test_a_clip_that_is_not_present_is_reported_as_not_accepted():
    rng = np.random.default_rng(6)
    segment = locate_clip(
        rng.standard_normal(SAMPLE_RATE * 2) * 0.1, rng.standard_normal(SAMPLE_RATE), SAMPLE_RATE, "absent"
    )
    assert not segment.accepted
    assert "sidelobe" in (segment.reason or "")


def test_several_clips_in_one_capture_are_segmented_in_order():
    rng = np.random.default_rng(8)
    clips = [rng.standard_normal(SAMPLE_RATE // 2) for _ in range(3)]
    gaps = [7_000, 3_500, 11_000]
    capture = np.zeros(0)
    offsets = []
    for clip, gap in zip(clips, gaps, strict=True):
        capture = np.concatenate([capture, np.zeros(gap)])
        offsets.append(capture.size)
        capture = np.concatenate([capture, clip * 0.4])
    capture = capture + rng.standard_normal(capture.size) * 0.004

    segments = segment_capture(capture, [(f"c{i}", c) for i, c in enumerate(clips)], SAMPLE_RATE)
    assert [s.clip_id for s in segments] == ["c0", "c1", "c2"]
    assert all(s.accepted for s in segments)
    for segment, offset in zip(segments, offsets, strict=True):
        assert abs(segment.start_sample - offset) <= 2


def test_every_segment_is_stamped_as_bookkeeping_only():
    rng = np.random.default_rng(10)
    clip = rng.standard_normal(SAMPLE_RATE // 2)
    capture = np.concatenate([np.zeros(1000), clip])
    segment = locate_clip(capture, clip, SAMPLE_RATE, "c")
    assert segment.to_dict()["provenance"] == "alignment_bookkeeping_only_never_used_for_detection"
