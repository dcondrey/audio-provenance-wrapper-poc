"""The resumable capture store.

A campaign runs for days across three rooms and gets interrupted. Resume therefore keys on the
INTEGRITY of what is on disk, never on a counter or a log line: a capture counts as done only when
its sidecar exists and the WAV it names still has the frame count and byte size the sidecar declared.
A crash between opening the file and finishing the write leaves a short WAV, which fails that check
and re-queues, which is the whole point.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator

from .audio import AudioError, file_pcm_sha256, frame_count, write_wav_atomic
from .campaign import CaptureTask

CAPTURES_DIR = "captures"
STATUS_MISSING = "missing"
STATUS_COMPLETE = "complete"
STATUS_CORRUPT = "corrupt"


class StoreError(RuntimeError):
    """The capture store is in a state the rig will not silently work around."""


@dataclass(frozen=True)
class CellStatus:
    task_id: str
    status: str
    reason: str | None = None


def write_json_atomic(path: str | Path, payload: dict) -> Path:
    destination = Path(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_name(f".{destination.name}.partial")
    text = json.dumps(payload, indent=2, sort_keys=False) + "\n"
    try:
        with open(temporary, "w", encoding="utf-8") as handle:
            handle.write(text)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, destination)
    except OSError as exc:
        temporary.unlink(missing_ok=True)
        raise StoreError(f"{destination}: could not be written ({exc})") from exc
    return destination


class CaptureStore:
    def __init__(self, root: str | Path) -> None:
        self.root = Path(root)
        self.captures_root = self.root / CAPTURES_DIR

    def wav_path(self, task: CaptureTask) -> Path:
        return self.captures_root / task.relative_path

    def sidecar_path(self, task: CaptureTask) -> Path:
        return self.wav_path(task).with_suffix(".json")

    def status(self, task: CaptureTask, verify_digest: bool = False) -> CellStatus:
        wav, sidecar = self.wav_path(task), self.sidecar_path(task)
        if not sidecar.exists():
            return CellStatus(task.task_id, STATUS_MISSING, "no sidecar")
        if not wav.exists():
            return CellStatus(task.task_id, STATUS_CORRUPT, "sidecar present but the capture is gone")
        try:
            record = json.loads(sidecar.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            return CellStatus(task.task_id, STATUS_CORRUPT, f"unreadable sidecar ({exc})")
        capture = record.get("capture", {})
        declared_frames = capture.get("frames")
        declared_bytes = capture.get("file_bytes")
        if declared_frames is None or declared_bytes is None:
            return CellStatus(task.task_id, STATUS_CORRUPT, "sidecar declares no frame count or file size")
        try:
            actual_frames = frame_count(wav)
        except AudioError as exc:
            return CellStatus(task.task_id, STATUS_CORRUPT, str(exc))
        if actual_frames != int(declared_frames):
            return CellStatus(
                task.task_id,
                STATUS_CORRUPT,
                f"truncated: {actual_frames} frames on disk, sidecar declares {declared_frames}",
            )
        if wav.stat().st_size != int(declared_bytes):
            return CellStatus(
                task.task_id,
                STATUS_CORRUPT,
                f"size mismatch: {wav.stat().st_size} bytes on disk, sidecar declares {declared_bytes}",
            )
        if verify_digest:
            declared_digest = capture.get("pcm_sha256")
            try:
                actual_digest = file_pcm_sha256(wav)
            except AudioError as exc:
                return CellStatus(task.task_id, STATUS_CORRUPT, str(exc))
            if declared_digest != actual_digest:
                return CellStatus(task.task_id, STATUS_CORRUPT, "PCM digest does not match the sidecar")
        return CellStatus(task.task_id, STATUS_COMPLETE)

    def pending(self, tasks: list[CaptureTask], verify_digest: bool = False) -> list[CaptureTask]:
        """Tasks still owed. A corrupt cell is owed again; that is the resume contract."""
        return [t for t in tasks if self.status(t, verify_digest).status != STATUS_COMPLETE]

    def survey(self, tasks: list[CaptureTask], verify_digest: bool = False) -> list[CellStatus]:
        return [self.status(t, verify_digest) for t in tasks]

    def commit(self, task: CaptureTask, audio, sample_rate: int, record: dict) -> Path:
        """Write the capture, then the sidecar. The sidecar is the completion marker and goes last.

        IMPORTANT: the stored digest is taken from the FILE, not from the in-memory array. Captures
        are written as 24-bit PCM, so a digest of the float samples before quantisation would never
        match a re-read and the integrity check that drives resume would fire on every cell.
        """
        wav = write_wav_atomic(self.wav_path(task), audio, sample_rate)
        payload = dict(record)
        capture = dict(payload.get("capture", {}))
        capture["file_bytes"] = wav.stat().st_size
        capture["pcm_sha256"] = file_pcm_sha256(wav)
        payload["capture"] = capture
        write_json_atomic(self.sidecar_path(task), payload)
        return wav

    def load_record(self, task: CaptureTask) -> dict:
        sidecar = self.sidecar_path(task)
        try:
            return json.loads(sidecar.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise StoreError(f"{sidecar}: cannot be read as a capture record ({exc})") from exc

    def iter_records(self) -> Iterator[tuple[Path, dict]]:
        if not self.captures_root.is_dir():
            return
        for sidecar in sorted(self.captures_root.rglob("*.json")):
            if sidecar.name.startswith("."):
                continue
            try:
                yield sidecar, json.loads(sidecar.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as exc:
                raise StoreError(f"{sidecar}: cannot be read as a capture record ({exc})") from exc
