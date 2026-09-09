"""Campaign configuration: the rooms x speakers x microphones x distances x clips matrix.

The full cross product of spec section 10.1 is 3 rooms x 3 speakers x 3 microphones x 5 distances
x 2 SPL levels = 270 cells, and 200 clips through all of them is roughly 450 hours of wall clock.
The kill criteria do not ask for that: K4 needs >= 3 rooms x >= 2 speakers x >= 2 microphones
x >= 200 clips at 1.0 m, and K6 needs a distance ladder. So a campaign is a list of STAGES, each
with its own subset of the grid and its own clip count, and docs/RUNBOOK.md prints the hours.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

SCHEMA = "audio-provenance-capture-rig-campaign/1"
IDENTIFIER = re.compile(r"^[a-z0-9][a-z0-9_]{0,63}$")
CAPTURE_MODES = ("live", "ingest")
STAGE_KINDS = ("sweep", "noise", "corpus")


class ConfigError(ValueError):
    """The campaign file does not describe a runnable campaign."""


def _identifier(value: Any, field_name: str) -> str:
    if not isinstance(value, str) or not IDENTIFIER.match(value):
        raise ConfigError(
            f"{field_name} must match {IDENTIFIER.pattern} (lower-case, digits, underscore); got {value!r}. "
            "Identifiers become path segments, so anything else is refused at the boundary."
        )
    return value


def _require(mapping: dict, key: str, context: str) -> Any:
    if key not in mapping:
        raise ConfigError(f"{context}: missing required key {key!r}")
    return mapping[key]


def _positive_float(value: Any, field_name: str) -> float:
    try:
        number = float(value)
    except (TypeError, ValueError) as exc:
        raise ConfigError(f"{field_name} must be a number, got {value!r}") from exc
    if not number > 0.0 or number != number:
        raise ConfigError(f"{field_name} must be positive and finite, got {number}")
    return number


@dataclass(frozen=True)
class SplCalibration:
    """The operator's sound-level-meter reading paired with the capture-side dBFS at that moment.

    Without this, a capture has dBFS and no absolute reference, and every SPL and dBA field in the
    output record is emitted as null rather than invented.
    """

    meter_reading_dba: float
    mic_rms_dbfs: float
    measured_at: str
    meter: str

    @staticmethod
    def parse(raw: Any, context: str) -> "SplCalibration | None":
        if raw is None:
            return None
        if not isinstance(raw, dict):
            raise ConfigError(f"{context}.spl_calibration must be a mapping")
        return SplCalibration(
            meter_reading_dba=float(_require(raw, "meter_reading_dba", context)),
            mic_rms_dbfs=float(_require(raw, "mic_rms_dbfs", context)),
            measured_at=str(_require(raw, "measured_at", context)),
            meter=str(_require(raw, "meter", context)),
        )

    def to_dict(self) -> dict:
        return {
            "meter_reading_dba": self.meter_reading_dba,
            "mic_rms_dbfs": self.mic_rms_dbfs,
            "measured_at": self.measured_at,
            "meter": self.meter,
        }


ROOM_ROLES = ("treated", "office", "live", "other")


@dataclass(frozen=True)
class Room:
    id: str
    description: str
    role: str
    rt60_target_seconds: float | None
    spl_calibration: SplCalibration | None

    @staticmethod
    def parse(raw: dict) -> "Room":
        room_id = _identifier(_require(raw, "id", "room"), "room.id")
        context = f"room[{room_id}]"
        target = raw.get("rt60_target_seconds")
        role = str(raw.get("role", "other"))
        if role not in ROOM_ROLES:
            raise ConfigError(f"{context}.role must be one of {ROOM_ROLES}, got {role!r}")
        return Room(
            id=room_id,
            description=str(raw.get("description", "")),
            role=role,
            rt60_target_seconds=None if target is None else _positive_float(target, f"{context}.rt60_target_seconds"),
            spl_calibration=SplCalibration.parse(raw.get("spl_calibration"), context),
        )

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "description": self.description,
            "role": self.role,
            "rt60_target_seconds": self.rt60_target_seconds,
            "spl_calibration": self.spl_calibration.to_dict() if self.spl_calibration else None,
        }


@dataclass(frozen=True)
class Speaker:
    id: str
    description: str
    output_device: str | None

    @staticmethod
    def parse(raw: dict) -> "Speaker":
        speaker_id = _identifier(_require(raw, "id", "speaker"), "speaker.id")
        device = raw.get("output_device")
        return Speaker(
            id=speaker_id,
            description=str(raw.get("description", "")),
            output_device=None if device is None else str(device),
        )

    def to_dict(self) -> dict:
        return {"id": self.id, "description": self.description, "output_device": self.output_device}


@dataclass(frozen=True)
class Microphone:
    """A capture device.

    `mode` is the decisive field. Spec 10.1 names a current iPhone and a current Android phone among
    the three capture devices, and neither can be opened as a CoreAudio input by this process. Those
    are `ingest`: the operator records on the phone and hands the rig a file, which `capture-rig
    ingest` registers into the same campaign store with the same condition record. Only `live`
    microphones are driven by the full-duplex path.
    """

    id: str
    description: str
    mode: str
    input_device: str | None

    @staticmethod
    def parse(raw: dict) -> "Microphone":
        mic_id = _identifier(_require(raw, "id", "microphone"), "microphone.id")
        mode = str(raw.get("mode", "live"))
        if mode not in CAPTURE_MODES:
            raise ConfigError(f"microphone[{mic_id}].mode must be one of {CAPTURE_MODES}, got {mode!r}")
        device = raw.get("input_device")
        if mode == "live" and device is None:
            raise ConfigError(
                f"microphone[{mic_id}] is live but declares no input_device; run `capture-rig devices` "
                "and copy the name, or set mode: ingest"
            )
        return Microphone(
            id=mic_id,
            description=str(raw.get("description", "")),
            mode=mode,
            input_device=None if device is None else str(device),
        )

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "description": self.description,
            "mode": self.mode,
            "input_device": self.input_device,
        }


@dataclass(frozen=True)
class SweepConfig:
    f_start_hz: float = 20.0
    f_end_hz: float = 20000.0
    duration_seconds: float = 10.0
    silence_seconds: float = 3.0
    amplitude_dbfs: float = -12.0
    repeats: int = 3
    ir_seconds: float = 2.0
    pre_seconds: float = 0.01

    @staticmethod
    def parse(raw: Any) -> "SweepConfig":
        raw = raw or {}
        if not isinstance(raw, dict):
            raise ConfigError("sweep must be a mapping")
        defaults = SweepConfig()
        config = SweepConfig(
            f_start_hz=float(raw.get("f_start_hz", defaults.f_start_hz)),
            f_end_hz=float(raw.get("f_end_hz", defaults.f_end_hz)),
            duration_seconds=_positive_float(raw.get("duration_seconds", defaults.duration_seconds), "sweep.duration_seconds"),
            silence_seconds=float(raw.get("silence_seconds", defaults.silence_seconds)),
            amplitude_dbfs=float(raw.get("amplitude_dbfs", defaults.amplitude_dbfs)),
            repeats=int(raw.get("repeats", defaults.repeats)),
            ir_seconds=_positive_float(raw.get("ir_seconds", defaults.ir_seconds), "sweep.ir_seconds"),
            pre_seconds=float(raw.get("pre_seconds", defaults.pre_seconds)),
        )
        if config.repeats < 1:
            raise ConfigError("sweep.repeats must be at least 1")
        if config.silence_seconds < config.ir_seconds:
            raise ConfigError(
                f"sweep.silence_seconds ({config.silence_seconds}) is shorter than sweep.ir_seconds "
                f"({config.ir_seconds}); the decay would be truncated before T30 can be fitted"
            )
        return config

    def to_dict(self) -> dict:
        return {
            "f_start_hz": self.f_start_hz,
            "f_end_hz": self.f_end_hz,
            "duration_seconds": self.duration_seconds,
            "silence_seconds": self.silence_seconds,
            "amplitude_dbfs": self.amplitude_dbfs,
            "repeats": self.repeats,
            "ir_seconds": self.ir_seconds,
            "pre_seconds": self.pre_seconds,
        }


@dataclass(frozen=True)
class Stage:
    name: str
    kind: str
    rooms: tuple[str, ...]
    speakers: tuple[str, ...]
    microphones: tuple[str, ...]
    distances_m: tuple[float, ...]
    spl_levels_dba: tuple[float | None, ...]
    clips: int
    arms: tuple[str, ...]
    noise_seconds: float

    def to_dict(self) -> dict:
        return {
            "name": self.name,
            "kind": self.kind,
            "rooms": list(self.rooms),
            "speakers": list(self.speakers),
            "microphones": list(self.microphones),
            "distances_m": list(self.distances_m),
            "spl_levels_dba": list(self.spl_levels_dba),
            "clips": self.clips,
            "arms": list(self.arms),
            "noise_seconds": self.noise_seconds,
        }


@dataclass(frozen=True)
class ClipSources:
    marked_dir: Path | None
    unmarked_dir: Path | None

    def directory(self, arm: str) -> Path:
        directory = self.marked_dir if arm == "marked" else self.unmarked_dir
        if directory is None:
            raise ConfigError(
                f"clips.{arm}_dir is not configured but a corpus stage requests the {arm} arm. "
                "Spec 10.2(e) makes the unmarked arm mandatory, not optional."
            )
        return directory


@dataclass(frozen=True)
class Campaign:
    campaign_id: str
    output_dir: Path
    sample_rate: int
    capture_channels: int
    rooms: tuple[Room, ...]
    speakers: tuple[Speaker, ...]
    microphones: tuple[Microphone, ...]
    stages: tuple[Stage, ...]
    sweep: SweepConfig
    clips: ClipSources
    source_path: Path | None = None
    notes: str = ""
    _by_id: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    def room(self, room_id: str) -> Room:
        return self._lookup(self.rooms, room_id, "room")

    def speaker(self, speaker_id: str) -> Speaker:
        return self._lookup(self.speakers, speaker_id, "speaker")

    def microphone(self, mic_id: str) -> Microphone:
        return self._lookup(self.microphones, mic_id, "microphone")

    @staticmethod
    def _lookup(items, item_id: str, kind: str):
        for item in items:
            if item.id == item_id:
                return item
        raise ConfigError(f"unknown {kind} id {item_id!r}")

    def to_dict(self) -> dict:
        return {
            "schema": SCHEMA,
            "campaign_id": self.campaign_id,
            "output_dir": str(self.output_dir),
            "sample_rate": self.sample_rate,
            "capture_channels": self.capture_channels,
            "rooms": [r.to_dict() for r in self.rooms],
            "speakers": [s.to_dict() for s in self.speakers],
            "microphones": [m.to_dict() for m in self.microphones],
            "stages": [s.to_dict() for s in self.stages],
            "sweep": self.sweep.to_dict(),
            "clips": {
                "marked_dir": str(self.clips.marked_dir) if self.clips.marked_dir else None,
                "unmarked_dir": str(self.clips.unmarked_dir) if self.clips.unmarked_dir else None,
            },
            "notes": self.notes,
        }


def _parse_stage(raw: dict, campaign_rooms, campaign_speakers, campaign_mics, defaults: dict) -> Stage:
    name = _identifier(_require(raw, "name", "stage"), "stage.name")
    kind = str(_require(raw, "kind", f"stage[{name}]"))
    if kind not in STAGE_KINDS:
        raise ConfigError(f"stage[{name}].kind must be one of {STAGE_KINDS}, got {kind!r}")

    def subset(key: str, available: tuple[str, ...]) -> tuple[str, ...]:
        raw_value = raw.get(key)
        if raw_value is None:
            return available
        chosen = tuple(_identifier(v, f"stage[{name}].{key}") for v in raw_value)
        unknown = [c for c in chosen if c not in available]
        if unknown:
            raise ConfigError(f"stage[{name}].{key} names unknown ids {unknown}")
        return chosen

    distances = raw.get("distances_m", defaults["distances_m"])
    spls = raw.get("spl_levels_dba", defaults["spl_levels_dba"])
    arms = tuple(str(a) for a in raw.get("arms", ("marked", "unmarked")))
    for arm in arms:
        if arm not in ("marked", "unmarked"):
            raise ConfigError(f"stage[{name}].arms may only contain 'marked' and 'unmarked', got {arm!r}")
    clips = int(raw.get("clips", 0))
    if kind == "corpus" and clips < 1:
        raise ConfigError(f"stage[{name}] is a corpus stage and must set clips >= 1")
    if kind == "corpus" and "unmarked" not in arms:
        raise ConfigError(
            f"stage[{name}] is a corpus stage with no unmarked arm. Spec 10.2(e): the false-positive "
            "arm is not an afterthought and cannot be synthesised later."
        )
    return Stage(
        name=name,
        kind=kind,
        rooms=subset("rooms", campaign_rooms),
        speakers=() if kind == "noise" else subset("speakers", campaign_speakers),
        microphones=subset("microphones", campaign_mics),
        distances_m=() if kind == "noise" else tuple(_positive_float(d, f"stage[{name}].distances_m") for d in distances),
        spl_levels_dba=() if kind == "noise" else tuple(None if s is None else float(s) for s in spls),
        clips=clips,
        arms=() if kind != "corpus" else arms,
        noise_seconds=float(raw.get("noise_seconds", 60.0)),
    )


def load_campaign(path: str | Path) -> Campaign:
    source = Path(path)
    raw = yaml.safe_load(source.read_text(encoding="utf-8"))
    if not isinstance(raw, dict):
        raise ConfigError(f"{source}: campaign file must be a YAML mapping")
    schema = raw.get("schema")
    if schema != SCHEMA:
        raise ConfigError(f"{source}: schema must be {SCHEMA!r}, got {schema!r}")

    rooms = tuple(Room.parse(r) for r in _require(raw, "rooms", str(source)))
    speakers = tuple(Speaker.parse(s) for s in _require(raw, "speakers", str(source)))
    mics = tuple(Microphone.parse(m) for m in _require(raw, "microphones", str(source)))
    if not rooms or not speakers or not mics:
        raise ConfigError(f"{source}: rooms, speakers and microphones must each be non-empty")
    for group, label in ((rooms, "room"), (speakers, "speaker"), (mics, "microphone")):
        ids = [item.id for item in group]
        if len(set(ids)) != len(ids):
            raise ConfigError(f"{source}: duplicate {label} id in {ids}")

    # A campaign that declares no SPL level gets one cell per condition with spl_target_dba None,
    # never 0.0. An invented target would reach `params.spl_target_dba` in the report through a field
    # that, unlike the measured `spl_dba`, has no null path — the exact fabrication this tool exists
    # to refuse.
    defaults = {
        "distances_m": raw.get("distances_m", [1.0]),
        "spl_levels_dba": raw.get("spl_levels_dba", [None]),
    }

    clip_raw = raw.get("clips") or {}
    base = source.parent
    clips = ClipSources(
        marked_dir=(base / clip_raw["marked_dir"]).resolve() if clip_raw.get("marked_dir") else None,
        unmarked_dir=(base / clip_raw["unmarked_dir"]).resolve() if clip_raw.get("unmarked_dir") else None,
    )

    stage_raw = _require(raw, "stages", str(source))
    if not stage_raw:
        raise ConfigError(f"{source}: stages must be non-empty")
    stages = tuple(
        _parse_stage(s, tuple(r.id for r in rooms), tuple(s_.id for s_ in speakers), tuple(m.id for m in mics), defaults)
        for s in stage_raw
    )
    stage_names = [s.name for s in stages]
    if len(set(stage_names)) != len(stage_names):
        raise ConfigError(f"{source}: duplicate stage name in {stage_names}")

    sample_rate = int(raw.get("sample_rate", 48000))
    if sample_rate < 32000:
        raise ConfigError(
            f"{source}: sample_rate {sample_rate} is below 32 kHz; Watermark-N runs at 48 kHz and spec "
            "2.2 refuses a host below 32 kHz outright"
        )
    channels = int(raw.get("capture_channels", 1))
    if channels not in (1, 2):
        raise ConfigError(f"{source}: capture_channels must be 1 or 2, got {channels}")

    return Campaign(
        campaign_id=_identifier(_require(raw, "campaign_id", str(source)), "campaign_id"),
        output_dir=(base / _require(raw, "output_dir", str(source))).resolve(),
        sample_rate=sample_rate,
        capture_channels=channels,
        rooms=rooms,
        speakers=speakers,
        microphones=mics,
        stages=stages,
        sweep=SweepConfig.parse(raw.get("sweep")),
        clips=clips,
        source_path=source.resolve(),
        notes=str(raw.get("notes", "")),
    )
