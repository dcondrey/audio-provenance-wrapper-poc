"""The condition record written beside every capture.

Spec section 10 exists because the published field reports acoustic results with room type, RT60,
background level and playback SPL documented nowhere. Every capture this rig produces carries those
fields or an explicit null with the reason, and nothing here invents a level it cannot source.
"""

from __future__ import annotations

import datetime as _dt
import math
import platform
from typing import Any

from . import __version__
from .acoustics import a_weighted_rms_dbfs, dba_from_dbfs, rms_dbfs
from .audio import peak_dbfs, pcm_sha256
from .campaign import CaptureTask
from .config import Campaign

RECORD_SCHEMA = "audio-provenance-capture-rig-capture/1"
CLIPPING_PEAK_DBFS = -0.5


def utc_now() -> str:
    return _dt.datetime.now(_dt.UTC).isoformat(timespec="seconds").replace("+00:00", "Z")


def _finite(value: float) -> float | None:
    return value if math.isfinite(value) else None


def level_report(audio, sample_rate: int, calibration) -> dict:
    """Capture levels in dBFS, and in dB SPL(A) only when a meter reading anchors them."""
    peak = peak_dbfs(audio)
    rms = rms_dbfs(audio)
    a_weighted = a_weighted_rms_dbfs(audio, sample_rate)
    report: dict[str, Any] = {
        "peak_dbfs": _finite(peak),
        "rms_dbfs": _finite(rms),
        "a_weighted_rms_dbfs": _finite(a_weighted),
        "clipped": bool(math.isfinite(peak) and peak > CLIPPING_PEAK_DBFS),
        "spl_dba": None,
        "spl_source": None,
        "spl_absent_reason": "no spl_calibration on this room; a dBFS reading has no absolute reference",
    }
    if calibration is not None and math.isfinite(a_weighted):
        report["spl_dba"] = dba_from_dbfs(a_weighted, calibration.mic_rms_dbfs, calibration.meter_reading_dba)
        report["spl_source"] = "derived_from_operator_meter_calibration"
        report["spl_absent_reason"] = None
    return report


def condition_record(
    campaign: Campaign,
    task: CaptureTask,
    audio,
    sample_rate: int,
    *,
    capture_mode: str,
    started_at: str,
    finished_at: str,
    devices: dict,
    room_measurement: dict | None = None,
    background: dict | None = None,
    clip_digest: str | None = None,
    extra: dict | None = None,
) -> dict:
    room = campaign.room(task.room_id)
    frames = int(audio.shape[0])
    record = {
        "schema": RECORD_SCHEMA,
        "campaign_id": campaign.campaign_id,
        "task_id": task.task_id,
        "stage": task.stage,
        "kind": task.kind,
        "channel": task.channel_id,
        "arm": task.arm,
        "condition": {
            "room_id": task.room_id,
            "room_description": room.description,
            "speaker_id": task.speaker_id,
            "microphone_id": task.microphone_id,
            "distance_m": task.distance_m,
            "spl_target_dba": task.spl_dba,
            "rt60_target_seconds": room.rt60_target_seconds,
            "measured_rt60_seconds": (room_measurement or {}).get("rt60_seconds"),
            "measured_drr_db": (room_measurement or {}).get("drr_db"),
            "rt60_source": (room_measurement or {}).get("source"),
            "background_dba": (background or {}).get("spl_dba"),
            "background_source": (background or {}).get("source"),
        },
        "capture": {
            "mode": capture_mode,
            "frames": frames,
            "sample_rate": sample_rate,
            "duration_seconds": frames / sample_rate,
            "channels": 1 if audio.ndim == 1 else int(audio.shape[1]),
            "pcm_sha256": pcm_sha256(audio),
            "levels": level_report(audio, sample_rate, room.spl_calibration),
            "spl_calibration": room.spl_calibration.to_dict() if room.spl_calibration else None,
        },
        "source_clip": {"clip_id": task.clip_id, "pcm_sha256": clip_digest},
        "devices": devices,
        "timing": {"started_at": started_at, "finished_at": finished_at},
        "software": {
            "capture_rig_version": __version__,
            "python": platform.python_version(),
            "platform": platform.platform(),
        },
    }
    if extra:
        record.update(extra)
    return record
