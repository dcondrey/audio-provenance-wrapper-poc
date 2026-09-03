"""Synchronous playback and capture over one PortAudio full-duplex stream.

Playback and capture share a stream so the two clocks are the device's own, not two independently
scheduled ones. Where the output and input are different physical devices the clocks are genuinely
independent and drift; that drift is a MEASURAND of this campaign, not a defect to correct, and
nothing here resamples it away.
"""

from __future__ import annotations

import numpy as np

from .devices import DeviceError, _sounddevice, resolve

DEFAULT_TAIL_SECONDS = 1.0


def play_and_record(
    stimulus: np.ndarray,
    sample_rate: int,
    output_device: str,
    input_device: str,
    input_channels: int = 1,
    tail_seconds: float = DEFAULT_TAIL_SECONDS,
) -> np.ndarray:
    """Play `stimulus` and capture concurrently, returning the capture as float64.

    The capture runs `tail_seconds` longer than the stimulus so the reverberant tail and any
    playback-path latency land inside the recording rather than being cut off at the end.
    """
    sd = _sounddevice()
    if stimulus.ndim != 1:
        raise DeviceError("stimulus must be mono; channel mapping is the device's job, not ours")
    if input_channels not in (1, 2):
        raise DeviceError("input_channels must be 1 or 2")
    out = resolve(output_device, "output")
    inp = resolve(input_device, "input")
    padded = np.concatenate([stimulus, np.zeros(int(round(tail_seconds * sample_rate)))])
    try:
        captured = sd.playrec(
            padded.astype(np.float32).reshape(-1, 1),
            samplerate=sample_rate,
            channels=input_channels,
            device=(inp.index, out.index),
            blocking=True,
            dtype="float32",
        )
    except Exception as exc:
        raise DeviceError(
            f"full-duplex stream failed between output {out.name!r} and input {inp.name!r} at "
            f"{sample_rate} Hz: {exc}"
        ) from exc
    data = np.asarray(captured, dtype=np.float64)
    return data[:, 0] if data.ndim == 2 and data.shape[1] == 1 else data


def record_only(
    seconds: float,
    sample_rate: int,
    input_device: str,
    input_channels: int = 1,
) -> np.ndarray:
    """Capture with nothing playing. This is how a room's background noise is measured."""
    sd = _sounddevice()
    if seconds <= 0.0:
        raise DeviceError("recording length must be positive")
    inp = resolve(input_device, "input")
    try:
        captured = sd.rec(
            int(round(seconds * sample_rate)),
            samplerate=sample_rate,
            channels=input_channels,
            device=inp.index,
            blocking=True,
            dtype="float32",
        )
    except Exception as exc:
        raise DeviceError(f"capture from {inp.name!r} at {sample_rate} Hz failed: {exc}") from exc
    data = np.asarray(captured, dtype=np.float64)
    return data[:, 0] if data.ndim == 2 and data.shape[1] == 1 else data
