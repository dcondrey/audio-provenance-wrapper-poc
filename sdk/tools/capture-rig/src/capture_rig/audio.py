"""WAV read/write and content digests. One place, so every path agrees on dtype and layout."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path

import numpy as np
import soundfile as sf

CAPTURE_SUBTYPE = "PCM_24"


class AudioError(ValueError):
    """An audio file could not be read or written in the form the rig requires."""


def read_wav(path: str | Path) -> tuple[np.ndarray, int]:
    """Read a WAV as float64 shaped (frames,) for mono or (frames, channels) otherwise."""
    try:
        data, sample_rate = sf.read(str(path), dtype="float64", always_2d=False)
    except (RuntimeError, sf.LibsndfileError) as exc:
        raise AudioError(f"{path}: cannot be read as audio ({exc})") from exc
    if data.size == 0:
        raise AudioError(f"{path}: contains no frames")
    return np.asarray(data, dtype=np.float64), int(sample_rate)


def to_mono(data: np.ndarray) -> np.ndarray:
    return data.mean(axis=1) if data.ndim == 2 else data


def frame_count(path: str | Path) -> int:
    try:
        with sf.SoundFile(str(path)) as handle:
            return int(len(handle))
    except (RuntimeError, sf.LibsndfileError) as exc:
        raise AudioError(f"{path}: cannot be opened to count frames ({exc})") from exc


def duration_seconds(path: str | Path) -> float:
    """Clip length from the container header. Never decodes; `plan` calls this once per clip."""
    try:
        with sf.SoundFile(str(path)) as handle:
            return len(handle) / float(handle.samplerate)
    except (RuntimeError, sf.LibsndfileError, ZeroDivisionError) as exc:
        raise AudioError(f"{path}: cannot be opened to read its duration ({exc})") from exc


def pcm_sha256(data: np.ndarray) -> str:
    """Digest of the sample values, not of the container, so a header rewrite is not a content change."""
    array = np.ascontiguousarray(np.asarray(data, dtype=np.float32))
    return hashlib.sha256(array.tobytes()).hexdigest()


def file_pcm_sha256(path: str | Path) -> str:
    data, _ = read_wav(path)
    return pcm_sha256(data)


def write_wav_atomic(path: str | Path, data: np.ndarray, sample_rate: int, subtype: str = CAPTURE_SUBTYPE) -> Path:
    """Write via a sibling temporary file and `os.replace`.

    IMPORTANT: a crash mid-write must never leave a short file that a resume mistakes for a finished
    capture. `os.replace` is atomic within a filesystem, and the temporary lives in the destination
    directory so the rename never crosses one.
    """
    destination = Path(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    # IMPORTANT: the temporary carries no usable extension, so the container format must be stated.
    # libsndfile infers format from the filename and refuses `.clip000.wav.partial`.
    temporary = destination.with_name(f".{destination.name}.partial")
    try:
        with sf.SoundFile(
            str(temporary),
            mode="w",
            samplerate=int(sample_rate),
            channels=1 if data.ndim == 1 else int(data.shape[1]),
            format="WAV",
            subtype=subtype,
        ) as handle:
            handle.write(np.asarray(data, dtype=np.float32))
        # SoundFile exposes no file descriptor, so the flush to stable storage happens after close.
        descriptor = os.open(temporary, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        os.replace(temporary, destination)
    except (RuntimeError, sf.LibsndfileError, OSError) as exc:
        temporary.unlink(missing_ok=True)
        raise AudioError(f"{destination}: could not be written ({exc})") from exc
    return destination


def peak_dbfs(data: np.ndarray) -> float:
    peak = float(np.max(np.abs(np.asarray(data, dtype=np.float64)))) if np.asarray(data).size else 0.0
    if peak <= 0.0:
        return float("-inf")
    return float(20.0 * np.log10(peak))
