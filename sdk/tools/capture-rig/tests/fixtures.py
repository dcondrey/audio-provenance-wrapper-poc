"""A tiny complete campaign on disk, so the config, store and report paths are exercised end to end."""

from __future__ import annotations

import textwrap
from pathlib import Path

import numpy as np

from capture_rig.audio import write_wav_atomic

SAMPLE_RATE = 48000

CAMPAIGN_YAML = """
schema: audio-provenance-capture-rig-campaign/1
campaign_id: fixture
output_dir: out
sample_rate: 48000
capture_channels: 1
distances_m: [0.5, 1.0]
spl_levels_dba: [75]
rooms:
  - id: treated_small
    role: treated
    description: fixture treated room
    rt60_target_seconds: 0.25
    spl_calibration:
      meter_reading_dba: 74.0
      mic_rms_dbfs: -20.0
      measured_at: '2026-09-01T10:00:00Z'
      meter: fixture meter
  - id: office
    role: office
    description: fixture office
  - id: live_hard
    role: live
    description: fixture live room
speakers:
  - id: monitor
    description: fixture monitor
    output_device: Fixture Output
  - id: puck
    description: fixture puck
    output_device: Fixture Output
microphones:
  - id: usb
    mode: live
    description: fixture usb mic
    input_device: Fixture Input
  - id: phone
    mode: ingest
    description: fixture phone
sweep:
  duration_seconds: 10.0
  silence_seconds: 3.0
  repeats: 1
  ir_seconds: 1.5
clips:
  marked_dir: corpus/marked
  unmarked_dir: corpus/unmarked
stages:
  - name: rir_sweep
    kind: sweep
    distances_m: [1.0]
  - name: corpus
    kind: corpus
    distances_m: [0.5, 1.0]
    clips: 2
    arms: [marked, unmarked]
"""


def build_campaign_tree(root: Path, clips: int = 2, seconds: float = 1.0) -> Path:
    """Write a campaign file plus marked and unmarked clip directories. Returns the campaign path."""
    rng = np.random.default_rng(4242)
    for arm in ("marked", "unmarked"):
        directory = root / "corpus" / arm
        for index in range(clips):
            audio = rng.standard_normal(int(seconds * SAMPLE_RATE)) * 0.1
            write_wav_atomic(directory / f"clip{index:03d}.wav", audio, SAMPLE_RATE)
    campaign = root / "campaign.yaml"
    campaign.write_text(textwrap.dedent(CAMPAIGN_YAML).lstrip(), encoding="utf-8")
    return campaign
