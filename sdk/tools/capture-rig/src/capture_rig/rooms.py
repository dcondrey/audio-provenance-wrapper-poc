"""Room measurement: sweep capture -> impulse response -> RT60, DRR, harmonic distortion -> K0.

Two outputs, both named in spec section 10.2. The measurement record evaluates kill criterion K0,
and the impulse response WAV plus its manifest line is training data: spec 9.2 ranks Audio Provenance's own
measured responses first, above the MIT survey and EchoThief, and the training tree's `RirCorpus`
reads exactly the manifest this module writes.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

import numpy as np

from .acoustics import ANALYSIS_BAND_HZ, DRR_CONVENTION, drr_db, mid_frequency_rt60, octave_band_rt60, rt60_t30
from .audio import to_mono, write_wav_atomic
from .campaign import CaptureTask
from .signals import SweepPair, exponential_sweep
from .state import write_json_atomic
from .sweep import ExtractedResponse, measure_response

IR_DIR = "impulse_responses"
MANIFEST_NAME = "manifest.jsonl"
MEASUREMENT_SCHEMA = "audio-provenance-capture-rig-room-measurement/1"
K0_TREATED_RT60_LIMIT_SECONDS = 0.6
K0_MIN_DRR_DB = 0.0
K0_DISTANCE_M = 1.0
K0_DISTANCE_TOLERANCE_M = 0.05

# Only the fields `training//apw-watermark-neural/data/manifest.py::CorpusEntry` accepts. It constructs the
# dataclass with **record, so an extra key is a TypeError at load time, not an ignored field.
MANIFEST_FIELDS = ("path", "licence", "source", "attribution", "url", "sample_rate", "seconds")


class RoomMeasurementError(ValueError):
    """A sweep capture cannot yield the room measurement being asked of it."""


def sweep_pair_for(sweep_config, sample_rate: int) -> SweepPair:
    return exponential_sweep(
        sample_rate=sample_rate,
        duration_seconds=sweep_config.duration_seconds,
        f_start_hz=sweep_config.f_start_hz,
        f_end_hz=sweep_config.f_end_hz,
    )


def sweep_pair_from_record(record: dict) -> SweepPair:
    """Rebuild the inverse filter from the stimulus the capture ACTUALLY used.

    IMPORTANT: never from the campaign file's current `sweep:` block. Editing that block after a room
    has been swept would silently reanalyse every stored capture against the wrong inverse filter,
    and the result is not an error, it is a plausible-looking wrong RT60.
    """
    stimulus = record.get("stimulus")
    if not isinstance(stimulus, dict):
        raise RoomMeasurementError(
            f"{record.get('task_id')}: the sweep capture carries no `stimulus` block, so the inverse "
            "filter that deconvolves it cannot be reconstructed. Re-capture it; the campaign file's "
            "current sweep settings are not evidence of what was played."
        )
    try:
        return exponential_sweep(
            sample_rate=int(stimulus["sample_rate"]),
            duration_seconds=float(stimulus["duration_seconds"]),
            f_start_hz=float(stimulus["f_start_hz"]),
            f_end_hz=float(stimulus["f_end_hz"]),
        )
    except (KeyError, TypeError, ValueError) as exc:
        raise RoomMeasurementError(f"{record.get('task_id')}: unusable stimulus block ({exc})") from exc


@dataclass(frozen=True)
class RoomMeasurement:
    task_id: str
    room_id: str
    speaker_id: str | None
    microphone_id: str | None
    distance_m: float | None
    response: ExtractedResponse
    rt60: dict
    octave_rt60: dict
    drr: float | None
    rt60_mid_seconds: float | None

    def to_dict(self) -> dict:
        return {
            "schema": MEASUREMENT_SCHEMA,
            "task_id": self.task_id,
            "room_id": self.room_id,
            "speaker_id": self.speaker_id,
            "microphone_id": self.microphone_id,
            "distance_m": self.distance_m,
            "rt60": self.rt60,
            "rt60_mid_seconds": self.rt60_mid_seconds,
            "rt60_octave_bands": self.octave_rt60,
            "analysis_band_hz": list(ANALYSIS_BAND_HZ),
            "drr_db": self.drr,
            "drr_convention": DRR_CONVENTION,
            "deconvolution": self.response.to_dict(),
        }

    def condition_summary(self) -> dict:
        """The subset that travels into every corpus capture record for this room."""
        return {
            "rt60_seconds": self.rt60.get("seconds"),
            "drr_db": self.drr,
            "source": f"measured_sweep:{self.task_id}",
        }


def analyse_sweep_capture(
    capture: np.ndarray,
    pair: SweepPair,
    task: CaptureTask,
    ir_seconds: float,
    pre_seconds: float,
) -> RoomMeasurement:
    mono = to_mono(np.asarray(capture, dtype=np.float64))
    response = measure_response(mono, pair, ir_seconds=ir_seconds, pre_seconds=pre_seconds)
    ir = response.ir
    octave = octave_band_rt60(ir, response.sample_rate)
    return RoomMeasurement(
        task_id=task.task_id,
        room_id=task.room_id,
        speaker_id=task.speaker_id,
        microphone_id=task.microphone_id,
        distance_m=task.distance_m,
        response=response,
        rt60=rt60_t30(ir, response.sample_rate).to_dict(),
        octave_rt60=octave,
        rt60_mid_seconds=mid_frequency_rt60(octave),
        drr=drr_db(ir, response.sample_rate),
    )


def ir_relative_path(task: CaptureTask) -> Path:
    return Path(task.relative_path).with_suffix(".wav")


def write_impulse_response(root: Path, task: CaptureTask, measurement: RoomMeasurement) -> Path:
    """Write the IR WAV and its full measurement sidecar under `<root>/impulse_responses/`."""
    destination = Path(root) / IR_DIR / ir_relative_path(task)
    write_wav_atomic(destination, measurement.response.ir, measurement.response.sample_rate, subtype="FLOAT")
    write_json_atomic(destination.with_suffix(".json"), measurement.to_dict())
    return destination


def write_rir_manifest(root: Path, sample_rate: int) -> Path:
    """Emit `impulse_responses/manifest.jsonl` in the exact form the training tree's loader demands.

    Every entry declares `licence: audio-provenance-owned` and `source: audio-provenance-stage0`, which are both on
    the training tree's allowlist. Anything else in this directory is not written here, so a run
    cannot quietly train on a response whose provenance nobody stated.
    """
    ir_root = Path(root) / IR_DIR
    if not ir_root.is_dir():
        raise RoomMeasurementError(f"{ir_root}: no impulse responses have been measured yet")
    entries = []
    for wav in sorted(ir_root.rglob("*.wav")):
        if wav.name.startswith("."):
            continue
        sidecar = wav.with_suffix(".json")
        seconds = None
        if sidecar.exists():
            record = json.loads(sidecar.read_text(encoding="utf-8"))
            samples = record.get("deconvolution", {}).get("ir_samples")
            rate = record.get("deconvolution", {}).get("sample_rate", sample_rate)
            seconds = round(samples / rate, 6) if samples else None
        entries.append(
            {
                "path": wav.relative_to(ir_root).as_posix(),
                "licence": "audio-provenance-owned",
                "source": "audio-provenance-stage0",
                "sample_rate": int(sample_rate),
                "seconds": seconds,
            }
        )
    if not entries:
        raise RoomMeasurementError(f"{ir_root}: contains no impulse response WAV files")
    manifest = ir_root / MANIFEST_NAME
    with manifest.open("w", encoding="utf-8") as handle:
        for entry in entries:
            handle.write(json.dumps({k: v for k, v in entry.items() if k in MANIFEST_FIELDS and v is not None}) + "\n")
    return manifest


def evaluate_k0(measurements: list[RoomMeasurement], rooms) -> dict:
    """Kill criterion K0, spec 12.3.

    Fires when the treated room's measured RT60 at 1.0 m exceeds 0.6 s, or when the measured DRR at
    1.0 m is below 0 dB in EVERY room. Measured, never simulated; a room with no 1.0 m measurement is
    reported as unevaluated rather than assumed to pass.
    """
    roles = {room.id: room.role for room in rooms}
    at_distance = [
        m for m in measurements
        if m.distance_m is not None and abs(m.distance_m - K0_DISTANCE_M) <= K0_DISTANCE_TOLERANCE_M
    ]
    per_room: dict[str, dict] = {}
    for measurement in at_distance:
        entry = per_room.setdefault(
            measurement.room_id,
            {"role": roles.get(measurement.room_id, "other"), "rt60_seconds": [], "drr_db": [], "untrustworthy": 0},
        )
        if not measurement.rt60.get("trustworthy", False):
            entry["untrustworthy"] += 1
        if measurement.rt60.get("seconds") is not None:
            entry["rt60_seconds"].append(measurement.rt60["seconds"])
        if measurement.drr is not None:
            entry["drr_db"].append(measurement.drr)

    summary = {}
    for room_id, entry in per_room.items():
        summary[room_id] = {
            "role": entry["role"],
            "measurements": len(entry["rt60_seconds"]),
            "untrustworthy_measurements": entry["untrustworthy"],
            "median_rt60_seconds": float(np.median(entry["rt60_seconds"])) if entry["rt60_seconds"] else None,
            "max_rt60_seconds": max(entry["rt60_seconds"], default=None),
            "median_drr_db": float(np.median(entry["drr_db"])) if entry["drr_db"] else None,
            "max_drr_db": max(entry["drr_db"], default=None),
        }

    reasons: list[str] = []
    treated = [room_id for room_id, entry in summary.items() if entry["role"] == "treated"]
    if not treated:
        reasons.append(
            "no room with role 'treated' has a measurement at 1.0 m; K0's first arm cannot be evaluated"
        )
    for room_id in treated:
        rt60 = summary[room_id]["median_rt60_seconds"]
        if rt60 is None:
            reasons.append(f"treated room {room_id} produced no usable RT60 at 1.0 m")
        elif rt60 > K0_TREATED_RT60_LIMIT_SECONDS:
            reasons.append(
                f"treated room {room_id} measures RT60 {rt60:.3f} s at 1.0 m, above the "
                f"{K0_TREATED_RT60_LIMIT_SECONDS} s limit"
            )
    drr_values = [entry["max_drr_db"] for entry in summary.values() if entry["max_drr_db"] is not None]
    if summary and drr_values and all(value < K0_MIN_DRR_DB for value in drr_values):
        reasons.append(
            f"measured DRR at 1.0 m is below {K0_MIN_DRR_DB} dB in all {len(summary)} measured room(s); "
            "the target envelope does not exist in realistic spaces"
        )
    if not summary:
        reasons.append("no measurement at 1.0 m exists; K0 is unevaluated")
    untrustworthy = sum(entry["untrustworthy_measurements"] for entry in summary.values())

    evaluable = bool(summary) and bool(treated)
    return {
        "criterion": "K0",
        "distance_m": K0_DISTANCE_M,
        "evaluated": evaluable,
        "fires": bool(reasons) and evaluable,
        "verdict": "kill" if (reasons and evaluable) else ("pass" if evaluable else "unevaluated"),
        "reasons": reasons,
        "rooms": summary,
        "untrustworthy_measurements": untrustworthy,
        "measurement_quality_note": (
            f"{untrustworthy} RT60 measurement(s) at 1.0 m are flagged noise-limited or poorly fitted. "
            "Those values are biased and this verdict is provisional until they are re-measured with a "
            "longer sweep, a longer silent tail or a higher playback level."
            if untrustworthy
            else "Every RT60 measurement at 1.0 m passed the noise-floor and decay-linearity checks."
        ),
        "rt60_convention": measurements[0].rt60.get("convention") if measurements else None,
        "drr_convention": DRR_CONVENTION,
    }
