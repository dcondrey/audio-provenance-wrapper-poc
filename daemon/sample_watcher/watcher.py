from __future__ import annotations

import argparse
import array
import json
import logging
import math
import platform
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable, Iterator

from daemon.common import append_jsonl, sha256_file_with_size, utc_timestamp

log = logging.getLogger(__name__)

AUDIO_EXTENSIONS = {".wav", ".aiff", ".aif", ".mp3", ".m4a"}
DEFAULT_EVIDENCE_PATH = Path("evidence/sample_import_events.jsonl")
DEFAULT_WATCH_DIR = Path("~/Music/ProvenanceSamples")
DEFAULT_NOTES = [
    "Detected by filesystem watcher.",
    "No claim is made that this file was placed on a specific track in any host.",
]

_FINGERPRINT_FRAME_CAP = 44_100
_AFINFO_TIMEOUT_SECONDS = 5

_AFINFO_DURATION = re.compile(r"estimated duration:\s*([0-9.]+)\s*sec")
_AFINFO_CHANNELS = re.compile(r"Data format:\s*(\d+)\s+ch")
_AFINFO_RATE = re.compile(r"Data format:\s*\d+\s+ch,\s*([0-9.]+)\s+Hz")

MetadataDict = dict[str, "float | int | None"]


@dataclass(frozen=True)
class FileSignature:
    size_bytes: int
    modified_ns: int


def is_audio_file(path: Path) -> bool:
    return path.is_file() and path.suffix.lower() in AUDIO_EXTENSIONS


def iter_audio_files(watch_dir: Path, recursive: bool) -> Iterable[Path]:
    matches = watch_dir.glob("**/*" if recursive else "*")
    return (candidate for candidate in sorted(matches) if is_audio_file(candidate))


def file_signature(path: Path) -> FileSignature:
    info = path.stat()
    return FileSignature(size_bytes=info.st_size, modified_ns=info.st_mtime_ns)


def empty_audio_metadata() -> MetadataDict:
    return {"duration_seconds": None, "sample_rate": None, "channels": None}


def _metadata_from_rate_and_frames(rate: int, frames: int, channels: int) -> MetadataDict:
    return {
        "duration_seconds": frames / rate if rate else None,
        "sample_rate": rate or None,
        "channels": channels,
    }


def _read_wave(path: Path) -> MetadataDict:
    import wave

    try:
        with wave.open(str(path), "rb") as handle:
            return _metadata_from_rate_and_frames(
                handle.getframerate(), handle.getnframes(), handle.getnchannels()
            )
    except (EOFError, OSError, wave.Error):
        return _read_riff_header(path) or _read_afinfo(path)


def _iter_chunks(handle, byte_order: str, limit: int = 256) -> Iterator[tuple[bytes, int, int]]:
    """Yield (chunk_id, data_offset, size) for a RIFF/IFF stream, bounded in count."""
    import struct

    for _ in range(limit):
        header = handle.read(8)
        if len(header) < 8:
            return
        chunk_id = header[:4]
        (size,) = struct.unpack(byte_order + "I", header[4:])
        offset = handle.tell()
        yield chunk_id, offset, size
        handle.seek(offset + size + (size & 1))


def _read_riff_header(path: Path) -> MetadataDict | None:
    """Portable WAV header reader for what `wave` rejects (float, extensible)."""
    import struct

    try:
        with path.open("rb") as handle:
            head = handle.read(12)
            if head[:4] not in (b"RIFF", b"RF64") or head[8:12] != b"WAVE":
                return None
            channels = rate = block_align = 0
            data_size = None
            for chunk_id, offset, size in _iter_chunks(handle, "<"):
                if chunk_id == b"fmt " and size >= 16:
                    handle.seek(offset)
                    _fmt, channels, rate, _bps, block_align, _bits = struct.unpack(
                        "<HHIIHH", handle.read(16)
                    )
                elif chunk_id == b"data":
                    data_size = size
                    break
            if not (rate and channels and block_align) or data_size is None:
                return None
            if data_size == 0xFFFFFFFF:  # RF64 / streamed placeholder: size unknown here
                return None
            return _metadata_from_rate_and_frames(rate, data_size // block_align, channels)
    except (OSError, struct.error):
        return None


def _read_aiff_header(path: Path) -> MetadataDict | None:
    """Portable AIFF/AIFC COMM-chunk reader (the stdlib `aifc` module is gone in 3.13)."""
    import struct

    try:
        with path.open("rb") as handle:
            head = handle.read(12)
            if head[:4] != b"FORM" or head[8:12] not in (b"AIFF", b"AIFC"):
                return None
            for chunk_id, offset, size in _iter_chunks(handle, ">"):
                if chunk_id == b"COMM" and size >= 18:
                    handle.seek(offset)
                    channels, frames, _bits = struct.unpack(">hIh", handle.read(8))
                    exponent, mantissa = struct.unpack(">HQ", handle.read(10))
                    sign = -1.0 if exponent & 0x8000 else 1.0
                    exponent &= 0x7FFF
                    rate = sign * mantissa * 2.0 ** (exponent - 16383 - 63) if exponent else 0.0
                    if channels <= 0 or not 0 < rate < 1e8:
                        return None
                    return _metadata_from_rate_and_frames(int(round(rate)), frames, channels)
    except (OverflowError, OSError, struct.error):
        return None
    return None


def _read_aiff(path: Path) -> MetadataDict:
    return _read_aiff_header(path) or _read_afinfo(path)


def _read_afinfo(path: Path) -> MetadataDict:
    """macOS only: shell out to `afinfo`. Elsewhere, or on failure, metadata is unavailable."""
    metadata = empty_audio_metadata()
    if platform.system() != "Darwin":
        return metadata
    try:
        completed = subprocess.run(
            ["afinfo", str(path)],
            check=False,
            capture_output=True,
            text=True,
            timeout=_AFINFO_TIMEOUT_SECONDS,
        )
    except (FileNotFoundError, OSError, subprocess.TimeoutExpired):
        return metadata

    if completed.returncode != 0:
        return metadata

    report = completed.stdout
    duration = _AFINFO_DURATION.search(report)
    if duration:
        metadata["duration_seconds"] = float(duration.group(1))

    rate = _AFINFO_RATE.search(report)
    if rate:
        as_float = float(rate.group(1))
        metadata["sample_rate"] = int(as_float) if as_float.is_integer() else as_float

    channels = _AFINFO_CHANNELS.search(report)
    if channels:
        metadata["channels"] = int(channels.group(1))

    return metadata


_METADATA_READERS: dict[str, Callable[[Path], MetadataDict]] = {
    ".wav": _read_wave,
    ".aif": _read_aiff,
    ".aiff": _read_aiff,
}


def extract_audio_metadata(path: Path) -> MetadataDict:
    return _METADATA_READERS.get(path.suffix.lower(), _read_afinfo)(path)


def _no_fingerprint() -> dict[str, float | None]:
    return {"rms": None, "zero_crossing_rate": None}


def _mono_samples(raw: bytes, channels: int) -> Iterator[float]:
    """Interleaved signed 16-bit LE frames to a normalised mono stream."""
    pcm = array.array("h")
    pcm.frombytes(raw)
    if sys.byteorder == "big":
        pcm.byteswap()

    if channels <= 1:
        return (sample / 32768.0 for sample in pcm)
    return (
        sum(pcm[offset : offset + channels]) / (channels * 32768.0)
        for offset in range(0, len(pcm) - channels + 1, channels)
    )


def compute_audio_fingerprint(path: Path) -> dict[str, float | None]:
    """Cheap perceptual summary. WAV/PCM16 only; anything else reports nothing.

    Deliberately not a provenance binding. It exists so the correlation engine
    can tell two unrelated imports apart, not so anything can be identified.
    """
    if path.suffix.lower() != ".wav":
        return _no_fingerprint()

    try:
        import wave

        with wave.open(str(path), "rb") as handle:
            if handle.getsampwidth() != 2 or handle.getnframes() == 0:
                return _no_fingerprint()
            channels = handle.getnchannels()
            raw = handle.readframes(min(handle.getnframes(), _FINGERPRINT_FRAME_CAP))

        count = 0
        energy = 0.0
        crossings = 0
        previous_sign = None
        for sample in _mono_samples(raw, channels):
            count += 1
            energy += sample * sample
            sign = sample >= 0
            if previous_sign is not None and sign != previous_sign:
                crossings += 1
            previous_sign = sign

        if count == 0:
            return _no_fingerprint()

        rate = crossings / (count - 1) if count > 1 else 0.0
        return {
            "rms": round(math.sqrt(energy / count), 6),
            "zero_crossing_rate": round(rate, 6),
        }
    except Exception:
        log.debug("Audio fingerprint extraction failed for %s", path, exc_info=True)
        return _no_fingerprint()


def build_sample_file_event(
    path: Path,
    observed_at: str | None = None,
    expected_signature: FileSignature | None = None,
) -> dict[str, object]:
    """Build one `sample_file_observed` record, or refuse.

    IMPORTANT: `directly_observed` is a claim about a file that held still. The
    stat taken here brackets every read below, and the closing stat must agree
    with it. A write landing anywhere inside that window means the hash, the
    metadata and the fingerprint may describe different bytes, so the record is
    refused rather than emitted with a proof level it has not earned.
    """
    target = path.expanduser().resolve()
    opening = target.stat()

    if expected_signature is not None and (
        opening.st_size != expected_signature.size_bytes
        or opening.st_mtime_ns != expected_signature.modified_ns
    ):
        raise OSError(f"{target} changed after its stability check; evidence not recorded")

    digest, hashed_bytes = sha256_file_with_size(target)
    audio_metadata = extract_audio_metadata(target)
    audio_fingerprint = compute_audio_fingerprint(target)

    closing = target.stat()
    if (
        hashed_bytes != opening.st_size
        or closing.st_size != opening.st_size
        or closing.st_mtime_ns != opening.st_mtime_ns
    ):
        raise OSError(f"{target} changed while being read; evidence not recorded")

    suffix = target.suffix.lower()
    notes = list(DEFAULT_NOTES)
    if all(value is None for value in audio_metadata.values()):
        notes.append(
            "Audio metadata unavailable: no portable reader for this format on "
            f"{platform.system() or 'this platform'}; fields are null, not measured."
        )
    return {
        "event_type": "sample_file_observed",
        "proof_level": "directly_observed",
        "file_name": target.name,
        "file_path": str(target),
        "sha256": digest,
        "format": suffix.lstrip("."),
        "file_extension": suffix,
        "file_size_bytes": hashed_bytes,
        "created_at": utc_timestamp(getattr(opening, "st_birthtime", opening.st_ctime)),
        "modified_at": utc_timestamp(opening.st_mtime),
        "observed_at": observed_at or utc_timestamp(),
        "audio_metadata": audio_metadata,
        "audio_fingerprint": audio_fingerprint,
        "notes": notes,
    }


def append_event(event: dict[str, object], evidence_path: Path) -> None:
    append_jsonl(evidence_path.expanduser(), event)


class SampleWatcher:
    """Polls a folder and emits one evidence record per settled audio file.

    A file is recorded only after its signature repeats for `stable_polls`
    consecutive scans, so a sample still being written is not hashed mid-flight.
    """

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
        # Keyed by resolved path. Every one of these is pruned against the live
        # listing on each scan; a bounce/delete/re-bounce loop would otherwise
        # grow them without bound for the lifetime of the daemon.
        self._seen: dict[str, FileSignature] = {}
        self._pending: dict[str, tuple[FileSignature, int]] = {}
        self._read_failed: set[str] = set()

    def _ensure_watch_dir(self) -> None:
        self.watch_dir.mkdir(parents=True, exist_ok=True)

    def mark_existing_seen(self) -> None:
        self._ensure_watch_dir()
        for path in iter_audio_files(self.watch_dir, self.recursive):
            self._seen[str(path.resolve())] = file_signature(path)

    def _forget_vanished(self, live_keys: set[str]) -> None:
        for key in list(self._seen):
            if key not in live_keys:
                del self._seen[key]
        for key in list(self._pending):
            if key not in live_keys:
                del self._pending[key]
        self._read_failed &= live_keys

    def _settled(self, key: str, signature: FileSignature) -> bool:
        """Advance the stability counter, reporting whether the file has settled."""
        previous, count = self._pending.get(key, (signature, 0))
        count = count + 1 if previous == signature else 1
        self._pending[key] = (signature, count)
        return count >= self.stable_polls

    def scan_once(self) -> list[dict[str, object]]:
        self._ensure_watch_dir()
        emitted: list[dict[str, object]] = []

        live = list(iter_audio_files(self.watch_dir, self.recursive))
        by_key = {str(path.resolve()): path for path in live}
        self._forget_vanished(set(by_key))

        for key, path in by_key.items():
            try:
                signature = file_signature(path)
            except OSError:
                continue

            if self._seen.get(key) == signature:
                continue
            if not self._settled(key, signature):
                continue

            try:
                event = build_sample_file_event(path, expected_signature=signature)
            except OSError as exc:
                # A file whose stat is stable but whose bytes will not read
                # re-qualifies on every poll, so this path retries forever. Warn
                # once per episode rather than either spamming the log or
                # dropping the evidence silently.
                if key not in self._read_failed:
                    log.warning(
                        "Sample file stat is stable but its content is unreadable; "
                        "evidence not emitted, will keep retrying: %s (%s)",
                        path,
                        exc,
                    )
                    self._read_failed.add(key)
                continue

            self._read_failed.discard(key)
            append_event(event, self.evidence_path)
            emitted.append(event)
            self._seen[key] = signature
            self._pending.pop(key, None)

        return emitted

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


def observe_existing_files(
    watch_dir: Path, evidence_path: Path, recursive: bool = False
) -> list[dict[str, object]]:
    watch_dir = watch_dir.expanduser()
    evidence_path = evidence_path.expanduser()
    watch_dir.mkdir(parents=True, exist_ok=True)

    emitted: list[dict[str, object]] = []
    for path in iter_audio_files(watch_dir, recursive):
        try:
            event = build_sample_file_event(path)
        except OSError as exc:
            log.warning("Skipping unstable file: %s", exc)
            continue
        append_event(event, evidence_path)
        emitted.append(event)

    return emitted


_CLI_FLAGS: tuple[tuple[str, dict[str, object]], ...] = (
    (
        "--watch-dir",
        {
            "type": Path,
            "default": DEFAULT_WATCH_DIR,
            "help": "Folder to watch for sample files. Defaults to ~/Music/ProvenanceSamples.",
        },
    ),
    (
        "--evidence-file",
        {
            "type": Path,
            "default": DEFAULT_EVIDENCE_PATH,
            "help": "JSONL output path. Defaults to evidence/sample_import_events.jsonl.",
        },
    ),
    ("--poll-interval", {"type": float, "default": 2.0, "help": "Seconds between directory scans."}),
    (
        "--stable-polls",
        {
            "type": int,
            "default": 2,
            "help": "Number of unchanged scans required before hashing a detected file.",
        },
    ),
    (
        "--scan-existing",
        {
            "action": "store_true",
            "help": "Record audio files that already exist in the watch folder at startup.",
        },
    ),
    ("--once", {"action": "store_true", "help": "Record current audio files once and exit."}),
    (
        "--recursive",
        {"action": "store_true", "help": "Scan subdirectories under the configured sample folder."},
    ),
)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Watch a local sample folder and write provenance evidence JSONL."
    )
    for flag, options in _CLI_FLAGS:
        parser.add_argument(flag, **options)  # type: ignore[arg-type]
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
