"""CoreAudio device enumeration and name resolution, via PortAudio.

The import of `sounddevice` is lazy. Report generation, the sweep mathematics and the alignment
bookkeeping all run on a machine with no audio hardware and must not fail because PortAudio cannot
open a device there.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


class DeviceError(RuntimeError):
    """An audio device could not be enumerated, resolved or opened."""


def _sounddevice():
    try:
        import sounddevice
    except (ImportError, OSError) as exc:
        raise DeviceError(
            f"PortAudio is unavailable in this process ({exc}). Device enumeration, playback and "
            "capture need it; the offline paths (sweep analysis, alignment, report) do not."
        ) from exc
    return sounddevice


@dataclass(frozen=True)
class Device:
    index: int
    name: str
    host_api: str
    max_input_channels: int
    max_output_channels: int
    default_samplerate: float
    is_default_input: bool
    is_default_output: bool

    def to_dict(self) -> dict:
        return {
            "index": self.index,
            "name": self.name,
            "host_api": self.host_api,
            "max_input_channels": self.max_input_channels,
            "max_output_channels": self.max_output_channels,
            "default_samplerate": self.default_samplerate,
            "is_default_input": self.is_default_input,
            "is_default_output": self.is_default_output,
        }


def list_devices() -> list[Device]:
    sd = _sounddevice()
    try:
        raw: Any = sd.query_devices()
        host_apis = sd.query_hostapis()
        default_in, default_out = sd.default.device
    except Exception as exc:  # PortAudio raises a family of errors that share no base class
        raise DeviceError(f"PortAudio could not enumerate devices: {exc}") from exc
    devices: list[Device] = []
    for index, entry in enumerate(raw):
        host = host_apis[entry["hostapi"]]["name"] if entry["hostapi"] < len(host_apis) else "unknown"
        devices.append(
            Device(
                index=index,
                name=str(entry["name"]),
                host_api=str(host),
                max_input_channels=int(entry["max_input_channels"]),
                max_output_channels=int(entry["max_output_channels"]),
                default_samplerate=float(entry["default_samplerate"]),
                is_default_input=index == default_in,
                is_default_output=index == default_out,
            )
        )
    return devices


def resolve(name: str, direction: str, devices: list[Device] | None = None) -> Device:
    """Resolve a configured device name to exactly one device.

    An ambiguous name is an error rather than a first match: running an entire campaign into the
    wrong microphone because two devices shared a prefix is not recoverable after the fact.
    """
    if direction not in ("input", "output"):
        raise DeviceError("direction must be 'input' or 'output'")
    pool = devices if devices is not None else list_devices()
    usable = [d for d in pool if (d.max_input_channels if direction == "input" else d.max_output_channels) > 0]
    exact = [d for d in usable if d.name == name]
    if len(exact) == 1:
        return exact[0]
    if len(exact) > 1:
        raise DeviceError(
            f"{direction} device name {name!r} matches {len(exact)} devices exactly "
            f"(indices {[d.index for d in exact]}); rename one in Audio MIDI Setup"
        )
    partial = [d for d in usable if name.lower() in d.name.lower()]
    if len(partial) == 1:
        return partial[0]
    if not partial:
        raise DeviceError(
            f"no {direction} device matches {name!r}. Available: {[d.name for d in usable]}"
        )
    raise DeviceError(
        f"{direction} device name {name!r} is ambiguous, matching {[d.name for d in partial]}. "
        "Use the full name."
    )


def describe_pair(output_name: str | None, input_name: str | None) -> dict:
    """The device block that goes into every capture record."""
    devices = list_devices()
    block: dict = {"host_api": None, "output": None, "input": None}
    if output_name is not None:
        device = resolve(output_name, "output", devices)
        block["output"] = device.to_dict()
        block["host_api"] = device.host_api
    if input_name is not None:
        device = resolve(input_name, "input", devices)
        block["input"] = device.to_dict()
        block["host_api"] = block["host_api"] or device.host_api
    return block
