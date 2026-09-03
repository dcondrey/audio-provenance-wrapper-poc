"""Executing a campaign against real hardware, one capture at a time, resumably."""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

import numpy as np

from .audio import AudioError, pcm_sha256, read_wav, to_mono
from .campaign import CaptureTask
from .config import Campaign, ConfigError
from .devices import DeviceError, describe_pair
from .playback import play_and_record, record_only
from .record import condition_record, utc_now
from .rooms import IR_DIR, RoomMeasurement, analyse_sweep_capture, sweep_pair_for, write_impulse_response
from .signals import sweep_stimulus
from .state import STATUS_COMPLETE, CaptureStore

ProgressFn = Callable[[str, str], None]


class ExecutionError(RuntimeError):
    """A capture could not be taken, and the campaign will not pretend otherwise."""


@dataclass
class RoomContext:
    """Per-room measurements that every later capture record in that room cites."""

    measurements: dict[str, dict]
    background: dict[str, dict]

    def for_room(self, room_id: str) -> tuple[dict | None, dict | None]:
        return self.measurements.get(room_id), self.background.get(room_id)


def load_room_context(store: CaptureStore) -> RoomContext:
    """Read back what the sweep and noise stages already measured.

    A corpus capture taken before its room was swept records `measured_rt60_seconds: null`. That is
    the honest answer and it is why the runbook orders the sweep stage first.
    """
    measurements: dict[str, dict] = {}
    ir_root = store.root / IR_DIR
    if ir_root.is_dir():
        for sidecar in sorted(ir_root.rglob("*.json")):
            if sidecar.name.startswith("."):
                continue
            record = json.loads(sidecar.read_text(encoding="utf-8"))
            room_id = record.get("room_id")
            distance = record.get("distance_m")
            if room_id is None:
                continue
            existing = measurements.get(room_id)
            better = existing is None or (
                distance is not None and abs(distance - 1.0) < abs((existing.get("distance_m") or 99.0) - 1.0)
            )
            if better and record.get("rt60", {}).get("seconds") is not None:
                measurements[room_id] = {
                    "rt60_seconds": record["rt60"]["seconds"],
                    "drr_db": record.get("drr_db"),
                    "distance_m": distance,
                    "source": f"measured_sweep:{record.get('task_id')}",
                }

    background: dict[str, dict] = {}
    for _, record in store.iter_records():
        if record.get("kind") != "noise":
            continue
        room_id = record.get("condition", {}).get("room_id")
        levels = record.get("capture", {}).get("levels", {})
        if room_id:
            background[room_id] = {
                "spl_dba": levels.get("spl_dba"),
                "a_weighted_rms_dbfs": levels.get("a_weighted_rms_dbfs"),
                "source": f"measured_noise:{record.get('task_id')}",
            }
    return RoomContext(measurements=measurements, background=background)


def _clip_audio(task: CaptureTask, campaign: Campaign) -> tuple[np.ndarray, str]:
    if task.clip_path is None:
        raise ExecutionError(f"{task.task_id}: corpus task has no clip path")
    try:
        data, rate = read_wav(task.clip_path)
    except AudioError as exc:
        raise ExecutionError(str(exc)) from exc
    if rate != campaign.sample_rate:
        raise ExecutionError(
            f"{task.clip_path}: sample rate {rate} does not match the campaign's {campaign.sample_rate}. "
            "Resample the corpus before the campaign; resampling inside the playback path would put a "
            "conversion in the measured chain that the record does not describe."
        )
    return to_mono(data), pcm_sha256(data)


def run_task(
    campaign: Campaign,
    store: CaptureStore,
    task: CaptureTask,
    context: RoomContext,
    dry_run: bool = False,
) -> tuple[Path | None, RoomMeasurement | None]:
    """Take one capture and commit it. Returns (path, room measurement for a sweep task)."""
    microphone = campaign.microphone(task.microphone_id)
    if microphone.mode != "live":
        raise ExecutionError(
            f"{task.task_id}: microphone {microphone.id!r} is mode {microphone.mode!r}. Record it on "
            "the device and register the file with `capture-rig ingest`."
        )
    speaker = campaign.speaker(task.speaker_id) if task.speaker_id else None
    if task.kind != "noise" and (speaker is None or speaker.output_device is None):
        raise ConfigError(f"{task.task_id}: speaker {task.speaker_id!r} declares no output_device")

    started = utc_now()
    if dry_run:
        return None, None

    measurement: RoomMeasurement | None = None
    clip_digest: str | None = None
    extra: dict = {}
    if task.kind == "noise":
        stage = next(s for s in campaign.stages if s.name == task.stage)
        captured = record_only(
            stage.noise_seconds, campaign.sample_rate, microphone.input_device, campaign.capture_channels
        )
        devices = describe_pair(None, microphone.input_device)
    elif task.kind == "sweep":
        pair = sweep_pair_for(campaign.sweep, campaign.sample_rate)
        stimulus = sweep_stimulus(pair, campaign.sweep.silence_seconds, campaign.sweep.amplitude_dbfs)
        captured = play_and_record(
            stimulus,
            campaign.sample_rate,
            speaker.output_device,
            microphone.input_device,
            campaign.capture_channels,
        )
        devices = describe_pair(speaker.output_device, microphone.input_device)
        measurement = analyse_sweep_capture(
            captured, pair, task, campaign.sweep.ir_seconds, campaign.sweep.pre_seconds
        )
        extra["room_measurement"] = measurement.to_dict()
        extra["stimulus"] = pair.to_dict()
    else:
        clip, clip_digest = _clip_audio(task, campaign)
        captured = play_and_record(
            clip,
            campaign.sample_rate,
            speaker.output_device,
            microphone.input_device,
            campaign.capture_channels,
        )
        devices = describe_pair(speaker.output_device, microphone.input_device)

    room_measurement, background = context.for_room(task.room_id)
    record = condition_record(
        campaign,
        task,
        captured,
        campaign.sample_rate,
        capture_mode="live",
        started_at=started,
        finished_at=utc_now(),
        devices=devices,
        room_measurement=room_measurement,
        background=background,
        clip_digest=clip_digest,
        extra=extra,
    )
    path = store.commit(task, captured, campaign.sample_rate, record)
    if measurement is not None:
        write_impulse_response(store.root, task, measurement)
    return path, measurement


def ingest_capture(
    campaign: Campaign,
    store: CaptureStore,
    task: CaptureTask,
    source_file: Path,
    context: RoomContext,
    note: str = "",
    alignment: dict | None = None,
) -> Path:
    """Register a recording made on a device this process cannot open, such as a phone.

    Spec 10.1 names an iPhone and an Android phone among the three capture devices; neither is a
    CoreAudio input here. The condition record is identical to a live capture's apart from
    `capture.mode`, so an ingested capture is a first-class trial and appears in the same report row.
    """
    microphone = campaign.microphone(task.microphone_id)
    if microphone.mode != "ingest":
        raise ExecutionError(
            f"{task.task_id}: microphone {microphone.id!r} is mode {microphone.mode!r}; use `capture-rig run`"
        )
    try:
        data, rate = read_wav(source_file)
    except AudioError as exc:
        raise ExecutionError(str(exc)) from exc
    if rate != campaign.sample_rate:
        raise ExecutionError(
            f"{source_file}: sample rate {rate} does not match the campaign's {campaign.sample_rate}. "
            "Convert it before ingest and record the conversion, rather than letting the rig hide one."
        )
    audio = to_mono(data)
    clip_digest = None
    if task.kind == "corpus" and task.clip_path is not None:
        clip_digest = _clip_audio(task, campaign)[1]
    room_measurement, background = context.for_room(task.room_id)
    record = condition_record(
        campaign,
        task,
        audio,
        campaign.sample_rate,
        capture_mode="ingest",
        started_at=utc_now(),
        finished_at=utc_now(),
        devices={"host_api": None, "output": None, "input": {"name": microphone.description or microphone.id}},
        room_measurement=room_measurement,
        background=background,
        clip_digest=clip_digest,
        extra={
            "ingest": {
                "source_file": str(source_file),
                "source_pcm_sha256": pcm_sha256(data),
                "note": note,
                # PROVENANCE ONLY. This is how confidently the segmenter placed this clip in the
                # operator's continuous take, and it exists so a mis-registered `--task` is visible in
                # the record. It never reaches a detector; see capture_rig/alignment/__init__.py.
                "alignment": alignment,
            }
        },
    )
    return store.commit(task, audio, campaign.sample_rate, record)


def run_campaign(
    campaign: Campaign,
    store: CaptureStore,
    tasks: list[CaptureTask],
    progress: ProgressFn,
    dry_run: bool = False,
    stop_on_error: bool = False,
) -> dict:
    """Run every pending task in order, resuming over whatever is already complete."""
    context = load_room_context(store)
    done = failed = skipped = 0
    failures: list[dict] = []
    measurements: list[RoomMeasurement] = []
    for task in tasks:
        if store.status(task).status == STATUS_COMPLETE:
            skipped += 1
            continue
        try:
            _, measurement = run_task(campaign, store, task, context, dry_run=dry_run)
            if measurement is not None:
                measurements.append(measurement)
                context = load_room_context(store)
            done += 1
            progress("ok", task.task_id)
        except (ExecutionError, ConfigError, DeviceError, AudioError) as exc:
            failed += 1
            failures.append({"task_id": task.task_id, "error": f"{type(exc).__name__}: {exc}"})
            progress("fail", f"{task.task_id}: {exc}")
            if stop_on_error:
                break
    return {
        "captured": done,
        "already_complete": skipped,
        "failed": failed,
        "failures": failures,
        "measurements": [m.to_dict() for m in measurements],
    }
