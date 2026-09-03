"""Expansion of a campaign into the ordered list of individual captures it asks for."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from .audio import AudioError, duration_seconds
from .config import Campaign, ConfigError, Stage

CLIP_EXTENSIONS = (".wav", ".flac", ".aiff", ".aif")


def distance_token(distance_m: float) -> str:
    return f"{distance_m:.2f}".rstrip("0").rstrip(".").replace(".", "p") + "m"


def spl_token(spl_dba: float | None) -> str:
    return "nospl" if spl_dba is None else f"{spl_dba:.0f}dba"


@dataclass(frozen=True)
class CaptureTask:
    """One capture the campaign owes. `task_id` is its stable identity across resumes."""

    stage: str
    kind: str
    room_id: str
    microphone_id: str
    speaker_id: str | None = None
    distance_m: float | None = None
    spl_dba: float | None = None
    arm: str | None = None
    clip_id: str | None = None
    clip_path: Path | None = None
    repeat: int | None = None

    @property
    def relative_path(self) -> Path:
        if self.kind == "noise":
            return Path(self.stage) / self.room_id / "noise" / f"{self.microphone_id}.wav"
        cell = f"{self.speaker_id}__{self.microphone_id}__{distance_token(self.distance_m or 0.0)}__{spl_token(self.spl_dba)}"
        if self.kind == "sweep":
            return Path(self.stage) / self.room_id / cell / "sweep" / f"rep{self.repeat:02d}.wav"
        return Path(self.stage) / self.room_id / cell / str(self.arm) / f"{self.clip_id}.wav"

    @property
    def task_id(self) -> str:
        return self.relative_path.with_suffix("").as_posix()

    @property
    def channel_id(self) -> str | None:
        """Spec 10.4's `physical_<room>_<speaker>_<mic>_<distance>` bench channel name.

        None for noise captures, which measure a room rather than a transmission path.
        """
        if self.kind == "noise" or self.speaker_id is None or self.distance_m is None:
            return None
        return f"physical_{self.room_id}_{self.speaker_id}_{self.microphone_id}_{distance_token(self.distance_m)}"


def list_clips(directory: Path, limit: int) -> list[Path]:
    if not directory.is_dir():
        raise ConfigError(f"{directory}: clip directory does not exist")
    clips = sorted(p for p in directory.iterdir() if p.suffix.lower() in CLIP_EXTENSIONS)
    if len(clips) < limit:
        raise ConfigError(
            f"{directory}: stage asks for {limit} clips but the directory holds {len(clips)}. "
            "A campaign that silently runs fewer clips reports a detection rate over a different "
            "trial count than the one it claims."
        )
    return clips[:limit]


def expand_stage(campaign: Campaign, stage: Stage) -> list[CaptureTask]:
    tasks: list[CaptureTask] = []
    if stage.kind == "noise":
        for room_id in stage.rooms:
            for mic_id in stage.microphones:
                tasks.append(CaptureTask(stage=stage.name, kind="noise", room_id=room_id, microphone_id=mic_id))
        return tasks

    clip_cache: dict[str, list[Path]] = {}
    for room_id in stage.rooms:
        for speaker_id in stage.speakers:
            for mic_id in stage.microphones:
                for distance in stage.distances_m:
                    for spl in stage.spl_levels_dba:
                        common = {
                            "stage": stage.name,
                            "room_id": room_id,
                            "speaker_id": speaker_id,
                            "microphone_id": mic_id,
                            "distance_m": distance,
                            "spl_dba": spl,
                        }
                        if stage.kind == "sweep":
                            for repeat in range(campaign.sweep.repeats):
                                tasks.append(CaptureTask(kind="sweep", repeat=repeat, **common))
                            continue
                        for arm in stage.arms:
                            if arm not in clip_cache:
                                clip_cache[arm] = list_clips(campaign.clips.directory(arm), stage.clips)
                            for clip in clip_cache[arm]:
                                tasks.append(
                                    CaptureTask(kind="corpus", arm=arm, clip_id=clip.stem, clip_path=clip, **common)
                                )
    return tasks


def expand(campaign: Campaign, stages: tuple[str, ...] | None = None) -> list[CaptureTask]:
    selected = campaign.stages if stages is None else tuple(s for s in campaign.stages if s.name in stages)
    if stages is not None:
        unknown = set(stages) - {s.name for s in campaign.stages}
        if unknown:
            raise ConfigError(f"unknown stage name(s): {sorted(unknown)}")
    tasks: list[CaptureTask] = []
    for stage in selected:
        tasks.extend(expand_stage(campaign, stage))
    ids = [t.task_id for t in tasks]
    if len(set(ids)) != len(ids):
        raise ConfigError("campaign expansion produced duplicate task ids; two stages collide on one output path")
    return tasks


def estimate_seconds(campaign: Campaign, tasks: list[CaptureTask], settle_seconds: float = 2.0) -> dict:
    """Wall-clock estimate per stage. Item 6 of the deliverable asks how long a campaign takes."""
    per_stage: dict[str, dict] = {}
    seconds_by_clip: dict[Path, float] = {}
    for task in tasks:
        entry = per_stage.setdefault(task.stage, {"captures": 0, "seconds": 0.0, "kind": task.kind})
        entry["captures"] += 1
        if task.kind == "sweep":
            entry["seconds"] += campaign.sweep.duration_seconds + campaign.sweep.silence_seconds + settle_seconds
        elif task.kind == "noise":
            stage = next(s for s in campaign.stages if s.name == task.stage)
            entry["seconds"] += stage.noise_seconds + settle_seconds
        else:
            entry["seconds"] += _clip_seconds(task.clip_path, seconds_by_clip) + settle_seconds
    total = sum(v["seconds"] for v in per_stage.values())
    return {
        "per_stage": per_stage,
        "total_seconds": total,
        "total_hours": total / 3600.0,
        "note": (
            "Playback and capture time only, plus a fixed settle allowance per capture. It excludes "
            "room setup, distance changes, meter readings and operator handling, which dominate a "
            "real day. Multiply by roughly 1.6 for a scheduled figure."
        ),
    }


def _clip_seconds(path: Path | None, cache: dict[Path, float]) -> float:
    if path is None:
        return 0.0
    if path not in cache:
        try:
            cache[path] = duration_seconds(path)
        except AudioError:
            cache[path] = 0.0
    return cache[path]
