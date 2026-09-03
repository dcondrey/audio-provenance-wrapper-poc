"""Runs a detector over a campaign's captures, blind, and scores afterwards.

The order of the two steps in `_run_one` is the contract of this whole tool: `detector.detect` is
called with the audio and the frozen thresholds, and the true payload is not read until after it has
returned. Reversing those two lines would turn every number this rig produces into the oracle
best-of-search maximum that spec 4.1 says the published field reports.
"""

from __future__ import annotations

import json
import time
from dataclasses import dataclass
from pathlib import Path

from ..audio import AudioError, read_wav, to_mono
from ..campaign import CaptureTask
from ..state import CaptureStore, STATUS_COMPLETE
from .adapter import Detection, Detector, DetectorError

PAYLOAD_MAP_SCHEMA = "audio-provenance-capture-rig-payloads/1"


@dataclass(frozen=True)
class TrialResult:
    task_id: str
    channel: str | None
    arm: str
    clip_id: str | None
    room_id: str
    speaker_id: str | None
    microphone_id: str | None
    distance_m: float | None
    duration_seconds: float
    detect_seconds: float
    detection: Detection
    expected_payload_hex: str | None
    exact: bool
    bit_error_rate: float | None
    error: str | None

    def to_dict(self) -> dict:
        return {
            "task_id": self.task_id,
            "channel": self.channel,
            "arm": self.arm,
            "clip_id": self.clip_id,
            "room_id": self.room_id,
            "speaker_id": self.speaker_id,
            "microphone_id": self.microphone_id,
            "distance_m": self.distance_m,
            "duration_seconds": self.duration_seconds,
            "detect_seconds": self.detect_seconds,
            "exact": self.exact,
            "payload_hex": self.detection.payload_hex,
            "bit_error_rate": self.bit_error_rate,
            "confidence": self.detection.confidence,
            "presence": self.detection.presence,
            "presence_score": self.detection.presence_score,
            "bits_corrected": self.detection.bits_corrected,
            "findings": list(self.detection.findings),
            "error": self.error,
        }


def load_payload_map(path: str | Path) -> dict[str, str]:
    """clip_id -> payload hex, for SCORING ONLY.

    This file is read by the scorer and never reaches a detector. It exists because a marked clip's
    payload is ground truth, and ground truth on the detection path is the defect this rig is built
    to avoid.
    """
    record = json.loads(Path(path).read_text(encoding="utf-8"))
    if record.get("schema") != PAYLOAD_MAP_SCHEMA:
        raise ValueError(f"{path}: schema must be {PAYLOAD_MAP_SCHEMA!r}, got {record.get('schema')!r}")
    payloads = record.get("payloads")
    if not isinstance(payloads, dict) or not payloads:
        raise ValueError(f"{path}: 'payloads' must be a non-empty mapping of clip id to hex payload")
    for clip_id, payload in payloads.items():
        if not isinstance(payload, str):
            raise ValueError(f"{path}: payload for {clip_id!r} is not a string")
        bytes.fromhex(payload)
    return {str(k): str(v) for k, v in payloads.items()}


def bit_error_rate(expected_hex: str, actual_hex: str) -> float | None:
    expected, actual = bytes.fromhex(expected_hex), bytes.fromhex(actual_hex)
    if len(expected) != len(actual) or not expected:
        return None
    differing = sum(bin(a ^ b).count("1") for a, b in zip(expected, actual, strict=True))
    return differing / (8 * len(expected))


def _run_one(
    detector: Detector,
    thresholds: dict,
    task: CaptureTask,
    record: dict,
    wav_path: Path,
    payloads: dict[str, str],
) -> TrialResult:
    arm = task.arm or record.get("arm") or "unknown"
    duration = float(record.get("capture", {}).get("duration_seconds", 0.0))

    detection = Detection()
    error: str | None = None
    elapsed = 0.0
    try:
        audio, sample_rate = read_wav(wav_path)
        duration = audio.shape[0] / sample_rate
        started = time.perf_counter()
        # BLIND. Audio and the frozen thresholds. Nothing below this line is available above it.
        detection = detector.detect(to_mono(audio), thresholds)
        elapsed = time.perf_counter() - started
    except (AudioError, DetectorError, ValueError) as exc:
        error = f"{type(exc).__name__}: {exc}"

    # Ground truth is consulted only now, to score what the detector already committed to.
    expected = payloads.get(task.clip_id or "") if arm == "marked" else None
    exact = bool(expected and detection.payload_hex == expected)
    ber = (
        bit_error_rate(expected, detection.payload_hex)
        if expected and detection.payload_hex
        else None
    )
    return TrialResult(
        task_id=task.task_id,
        channel=task.channel_id,
        arm=arm,
        clip_id=task.clip_id,
        room_id=task.room_id,
        speaker_id=task.speaker_id,
        microphone_id=task.microphone_id,
        distance_m=task.distance_m,
        duration_seconds=duration,
        detect_seconds=elapsed,
        detection=detection,
        expected_payload_hex=expected,
        exact=exact,
        bit_error_rate=ber,
        error=error or detection.error,
    )


def run_trials(
    store: CaptureStore,
    tasks: list[CaptureTask],
    detector: Detector,
    thresholds: dict,
    payloads: dict[str, str],
) -> tuple[list[TrialResult], list[str]]:
    """Detect over every complete corpus capture. Returns (results, skipped task ids)."""
    results: list[TrialResult] = []
    skipped: list[str] = []
    for task in tasks:
        if task.kind != "corpus":
            continue
        if store.status(task).status != STATUS_COMPLETE:
            skipped.append(task.task_id)
            continue
        results.append(
            _run_one(detector, thresholds, task, store.load_record(task), store.wav_path(task), payloads)
        )
    return results, skipped
