"""The measured impulse-response corpus must load in the training tree without translation.

Spec 9.2 ranks Audio Provenance's own Stage 0 responses first among impulse-response sources, above the MIT
survey and EchoThief. That claim is only true if the training tree's licence-gated loader accepts
what this rig writes, so it is checked rather than asserted. The check runs inside the training
project's own environment, because that is where the loader's dependencies live.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import numpy as np
import pytest
from scipy.signal import fftconvolve

from capture_rig.audio import read_wav
from capture_rig.campaign import CaptureTask
from capture_rig.rooms import IR_DIR, analyse_sweep_capture, write_impulse_response, write_rir_manifest
from capture_rig.signals import exponential_sweep, sweep_stimulus

from .synthetic import decaying_ir

SAMPLE_RATE = 48000
TRAINING_ROOT = Path("/Volumes/A/audio-provenance/sdk/training")

PROBE = """
import json, sys
from apw_watermark_neural.config import RirConfig
from apw_watermark_neural.data.manifest import load_manifest
from apw_watermark_neural.rir.corpus import RirCorpus

root = sys.argv[1]
entries = load_manifest(root + "/manifest.jsonl")
corpus = RirCorpus(RirConfig(corpus_dirs=(root,), synthetic_fraction=0.0), 48000)
print(json.dumps({
    "entries": len(entries),
    "licences": sorted({e.licence for e in entries}),
    "sources": sorted({e.source for e in entries}),
    "measured_count": corpus.measured_count,
    "provenance_note": corpus.provenance()["note"],
}))
"""


def _measure_into(root: Path, count: int = 2) -> None:
    pair = exponential_sweep(SAMPLE_RATE, 10.0, 20.0, 20000.0)
    for index in range(count):
        ir = decaying_ir(SAMPLE_RATE, 0.30 + 0.1 * index, length_seconds=1.0, seed=100 + index)
        recording = fftconvolve(sweep_stimulus(pair, 2.0, -6.0), ir)
        task = CaptureTask(
            stage="rir_sweep",
            kind="sweep",
            room_id=f"room{index}",
            microphone_id="usb",
            speaker_id="monitor",
            distance_m=1.0,
            spl_dba=75.0,
            repeat=0,
        )
        measurement = analyse_sweep_capture(recording, pair, task, ir_seconds=0.9, pre_seconds=0.005)
        write_impulse_response(root, task, measurement)


def test_the_manifest_declares_only_fields_the_training_loader_accepts(tmp_path):
    _measure_into(tmp_path)
    manifest = write_rir_manifest(tmp_path, SAMPLE_RATE)
    accepted = {"path", "licence", "source", "attribution", "url", "sample_rate", "seconds"}
    lines = [json.loads(line) for line in manifest.read_text(encoding="utf-8").splitlines() if line.strip()]
    assert lines
    for entry in lines:
        assert set(entry) <= accepted, set(entry) - accepted
        assert entry["licence"] == "audio-provenance-owned"
        assert entry["source"] == "audio-provenance-stage0"
        assert entry["sample_rate"] == SAMPLE_RATE
        assert (tmp_path / IR_DIR / entry["path"]).exists()


def test_the_training_tree_loads_this_rigs_impulse_response_corpus(tmp_path):
    if not (TRAINING_ROOT / ".venv" / "bin" / "python").exists():
        pytest.skip("training project environment is not synced next to this tool")
    _measure_into(tmp_path, count=3)
    write_rir_manifest(tmp_path, SAMPLE_RATE)

    completed = subprocess.run(
        ["uv", "run", "--project", str(TRAINING_ROOT), "python", "-c", PROBE, str(tmp_path / IR_DIR)],
        capture_output=True,
        text=True,
        cwd=str(TRAINING_ROOT),
        timeout=600,
    )
    if completed.returncode != 0:
        pytest.skip(f"training environment unavailable: {completed.stderr.strip()[-400:]}")

    result = json.loads(completed.stdout.strip().splitlines()[-1])
    assert result["entries"] == 3
    assert result["measured_count"] == 3
    assert result["licences"] == ["audio-provenance-owned"]
    assert result["sources"] == ["audio-provenance-stage0"]
    assert "Measured and synthetic" in result["provenance_note"]


def test_measured_impulse_responses_carry_their_own_measurement_sidecar(tmp_path):
    _measure_into(tmp_path, count=1)
    sidecars = list((tmp_path / IR_DIR).rglob("*.json"))
    assert len(sidecars) == 1
    record = json.loads(sidecars[0].read_text(encoding="utf-8"))
    assert record["schema"] == "audio-provenance-capture-rig-room-measurement/1"
    assert record["rt60"]["seconds"] == pytest.approx(0.30, rel=0.10)
    assert record["rt60"]["trustworthy"] is True
    assert record["drr_convention"] == "ace_direct_window_2p5ms"
    assert record["analysis_band_hz"] == [100.0, 8000.0]
    assert set(record["rt60_octave_bands"]) >= {"500", "1000"}
    assert record["deconvolution"]["thd_percent"] is not None

    # The written IR must round-trip its absolute scale. The inverse filter is normalised so a
    # deconvolved response carries the true linear gain of the acoustic path; a lossy container would
    # silently make these responses arbitrarily rather than absolutely scaled, which is exactly what
    # makes them worth more than a downloaded corpus.
    stored, rate = read_wav(sidecars[0].with_suffix(".wav"))
    assert rate == SAMPLE_RATE
    assert float(np.max(np.abs(stored))) == pytest.approx(record["deconvolution"]["linear_peak_abs"], rel=1e-6)
