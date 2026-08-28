from __future__ import annotations

import argparse
import json
import logging
import math
import re
import struct
import subprocess
import time
import warnings
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

from daemon.common import append_jsonl, sha256_file, utc_timestamp

log = logging.getLogger(__name__)

AUDIO_EXTENSIONS = {".wav", ".aiff", ".aif", ".mp3", ".m4a"}
DEFAULT_EVIDENCE_PATH = Path("evidence/sample_import_events.jsonl")
DEFAULT_WATCH_DIR = Path("~/Music/ProvenanceSamples")
DEFAULT_NOTES = [
    "Detected by filesystem watcher.",
    "No claim is made that this file was placed on a specific Ableton track.",
]


@dataclass(frozen=True)
class FileSignature:
    size_bytes: int
    modified_ns: int


def is_audio_file(path: Path) -> bool:
    return path.is_file() and path.suffix.lower() in AUDIO_EXTENSIONS


def iter_audio_files(watch_dir: Path, recursive: bool) -> Iterable[Path]:
    pattern = "**/*" if recursive else "*"
    for path in sorted(watch_dir.glob(pattern)):
        if is_audio_file(path):
            yield path


def file_signature(path: Path) -> FileSignature:
    stat_result = path.stat()
    return FileSignature(size_bytes=stat_result.st_size, modified_ns=stat_result.st_mtime_ns)


def empty_audio_metadata() -> dict[str, float | int | None]:
    return {
        "duration_seconds": None,
        "sample_rate": None,
        "channels": None,
    }


def extract_audio_metadata(path: Path) -> dict[str, float | int | None]:
    suffix = path.suffix.lower()
    if suffix == ".wav":
        return _extract_wave_metadata(path)
    if suffix in {".aif", ".aiff"}:
        return _extract_aiff_metadata(path)
    return _extract_afinfo_metadata(path)


def _extract_wave_metadata(path: Path) -> dict[str, float | int | None]:
    import wave

    metadata = empty_audio_metadata()
    try:
        with wave.open(str(path), "rb") as audio_file:
            sample_rate = audio_file.getframerate()
            frames = audio_file.getnframes()
            metadata["sample_rate"] = sample_rate or None
            metadata["channels"] = audio_file.getnchannels()
            metadata["duration_seconds"] = frames / sample_rate if sample_rate else None
    except (EOFError, OSError, wave.Error):
        return _extract_afinfo_metadata(path)
    return metadata


def _extract_aiff_metadata(path: Path) -> dict[str, float | int | None]:
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeprecationWarning)
            import aifc
    except ModuleNotFoundError:
        return _extract_afinfo_metadata(path)

    metadata = empty_audio_metadata()
    try:
        with aifc.open(str(path), "rb") as audio_file:
            sample_rate = audio_file.getframerate()
            frames = audio_file.getnframes()
            metadata["sample_rate"] = sample_rate or None
            metadata["channels"] = audio_file.getnchannels()
            metadata["duration_seconds"] = frames / sample_rate if sample_rate else None
    except (EOFError, OSError, aifc.Error):
        return _extract_afinfo_metadata(path)
    return metadata


def _extract_afinfo_metadata(path: Path) -> dict[str, float | int | None]:
    metadata = empty_audio_metadata()
    try:
        result = subprocess.run(
            ["afinfo", str(path)],
            check=False,
            capture_output=True,
            text=True,
            timeout=5,
        )
    except (FileNotFoundError, OSError, subprocess.TimeoutExpired):
        return metadata

    if result.returncode != 0:
        return metadata

    duration_match = re.search(r"estimated duration:\s*([0-9.]+)\s*sec", result.stdout)
    channels_match = re.search(r"Data format:\s*(\d+)\s+ch", result.stdout)
    sample_rate_match = re.search(r"Data format:\s*\d+\s+ch,\s*([0-9.]+)\s+Hz", result.stdout)

    if duration_match:
        metadata["duration_seconds"] = float(duration_match.group(1))
    if sample_rate_match:
        sample_rate = float(sample_rate_match.group(1))
        metadata["sample_rate"] = int(sample_rate) if sample_rate.is_integer() else sample_rate
    if channels_match:
        metadata["channels"] = int(channels_match.group(1))

    return metadata


def compute_audio_fingerprint(path: Path) -> dict[str, float | None]:
    if path.suffix.lower() != ".wav":
        return {"rms": None, "zero_crossing_rate": None}

    try:
        import wave

        with wave.open(str(path), "rb") as wf:
            nframes = wf.getnframes()
            nchannels = wf.getnchannels()
            sampwidth = wf.getsampwidth()
            if sampwidth != 2 or nframes == 0:
                return {"rms": None, "zero_crossing_rate": None}

            max_frames = min(nframes, 44100)
            raw = wf.readframes(max_frames)
            total_samples = len(raw) // 2
            samples = struct.unpack(f"<{total_samples}h", raw)

            if nchannels > 1:
                mono = [
                    sum(samples[i : i + nchannels]) / nchannels
                    for i in range(0, len(samples), nchannels)
                ]
            else:
                mono = list(samples)

            if not mono:
                return {"rms": None, "zero_crossing_rate": None}

            norm = [s / 32768.0 for s in mono]
            rms = math.sqrt(sum(s * s for s in norm) / len(norm))
            crossings = sum(
                1 for i in range(1, len(norm)) if (norm[i] >= 0) != (norm[i - 1] >= 0)
            )
            zcr = crossings / (len(norm) - 1) if len(norm) > 1 else 0.0

            return {"rms": round(rms, 6), "zero_crossing_rate": round(zcr, 6)}
    except Exception:
        log.debug("Audio fingerprint extraction failed for %s", path, exc_info=True)
        return {"rms": None, "zero_crossing_rate": None}


def build_sample_file_event(path: Path, observed_at: str | None = None) -> dict[str, object]:
    resolved_path = path.expanduser().resolve()
    stat_result = resolved_path.stat()
    created_timestamp = getattr(stat_result, "st_birthtime", stat_result.st_ctime)

    return {
        "event_type": "sample_file_observed",
        "proof_level": "directly_observed",
        "file_name": resolved_path.name,
        "file_path": str(resolved_path),
        "sha256": sha256_file(resolved_path),
        "format": resolved_path.suffix.lower().lstrip("."),
        "file_extension": resolved_path.suffix.lower(),
        "file_size_bytes": stat_result.st_size,
        "created_at": utc_timestamp(created_timestamp),
        "modified_at": utc_timestamp(stat_result.st_mtime),
        "observed_at": observed_at or utc_timestamp(),
        "audio_metadata": extract_audio_metadata(resolved_path),
        "audio_fingerprint": compute_audio_fingerprint(resolved_path),
        "notes": list(DEFAULT_NOTES),
    }


def append_event(event: dict[str, object], evidence_path: Path) -> None:
    append_jsonl(evidence_path.expanduser(), event)


class SampleWatcher:
    def __init__(
        self,
        watch_dir: Path,
        evidence_path: Path,
        poll_interval_seconds: float = 2.0,
        stable_polls: int = 2,
        recursive: bool = False,
    ) -> None:
        self.watch_dir = watch_dir.expanduser()
        self.evidence_path = evidence_path.expanduser()
        self.poll_interval_seconds = poll_interval_seconds
        self.stable_polls = max(1, stable_polls)
        self.recursive = recursive
        self._seen: dict[str, FileSignature] = {}
        self._pending: dict[str, tuple[FileSignature, int]] = {}

    def mark_existing_seen(self) -> None:
        self.watch_dir.mkdir(parents=True, exist_ok=True)
        for path in iter_audio_files(self.watch_dir, self.recursive):
            self._seen[str(path.resolve())] = file_signature(path)

    def scan_once(self) -> list[dict[str, object]]:
        self.watch_dir.mkdir(parents=True, exist_ok=True)
        observed_events: list[dict[str, object]] = []

        for path in iter_audio_files(self.watch_dir, self.recursive):
            resolved_key = str(path.resolve())
            try:
                signature = file_signature(path)
            except OSError:
                continue

            if self._seen.get(resolved_key) == signature:
                continue

            previous_signature, stable_count = self._pending.get(resolved_key, (signature, 0))
            stable_count = stable_count + 1 if previous_signature == signature else 1
            self._pending[resolved_key] = (signature, stable_count)

            if stable_count < self.stable_polls:
                continue

            try:
                event = build_sample_file_event(path)
            except OSError:
                continue

            append_event(event, self.evidence_path)
            observed_events.append(event)
            self._seen[resolved_key] = signature
            self._pending.pop(resolved_key, None)

        return observed_events

    def run_forever(self, scan_existing: bool = False) -> None:
        if scan_existing:
            self.scan_once()
        else:
            self.mark_existing_seen()

        log.info("Watching %s for audio samples; writing %s", self.watch_dir, self.evidence_path)
        while True:
            for event in self.scan_once():
                log.info("%s", json.dumps(event, separators=(",", ":")))
            time.sleep(self.poll_interval_seconds)


def observe_existing_files(watch_dir: Path, evidence_path: Path, recursive: bool = False) -> list[dict[str, object]]:
    watch_dir = watch_dir.expanduser()
    evidence_path = evidence_path.expanduser()
    watch_dir.mkdir(parents=True, exist_ok=True)
    observed_events: list[dict[str, object]] = []

    for path in iter_audio_files(watch_dir, recursive):
        event = build_sample_file_event(path)
        append_event(event, evidence_path)
        observed_events.append(event)

    return observed_events


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Watch a local sample folder and write provenance evidence JSONL.")
    parser.add_argument(
        "--watch-dir",
        type=Path,
        default=DEFAULT_WATCH_DIR,
        help="Folder to watch for sample files. Defaults to ~/Music/ProvenanceSamples.",
    )
    parser.add_argument(
        "--evidence-file",
        type=Path,
        default=DEFAULT_EVIDENCE_PATH,
        help="JSONL output path. Defaults to evidence/sample_import_events.jsonl.",
    )
    parser.add_argument(
        "--poll-interval",
        type=float,
        default=2.0,
        help="Seconds between directory scans.",
    )
    parser.add_argument(
        "--stable-polls",
        type=int,
        default=2,
        help="Number of unchanged scans required before hashing a detected file.",
    )
    parser.add_argument(
        "--scan-existing",
        action="store_true",
        help="Record audio files that already exist in the watch folder at startup.",
    )
    parser.add_argument(
        "--once",
        action="store_true",
        help="Record current audio files once and exit.",
    )
    parser.add_argument(
        "--recursive",
        action="store_true",
        help="Scan subdirectories under the configured sample folder.",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")
    args = parse_args(argv)

    if args.once:
        for event in observe_existing_files(args.watch_dir, args.evidence_file, args.recursive):
            log.info("%s", json.dumps(event, separators=(",", ":")))
        return 0

    watcher = SampleWatcher(
        watch_dir=args.watch_dir,
        evidence_path=args.evidence_file,
        poll_interval_seconds=args.poll_interval,
        stable_polls=args.stable_polls,
        recursive=args.recursive,
    )
    try:
        watcher.run_forever(scan_existing=args.scan_existing)
    except KeyboardInterrupt:
        return 0
    return 0
