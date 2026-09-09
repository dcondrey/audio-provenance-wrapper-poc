"""Detector adapters. A detector is anything with `detect(audio, thresholds) -> Detection`.

That is the signature `training//apw-watermark-neural/pipeline.py::Detector.detect` already uses, so a
PyTorch Watermark-N detector, an AudioSeal baseline or a Rust binary all plug in the same way and
none of them can be handed anything a deployed detector would not have.
"""

from __future__ import annotations

import importlib
import json
import shlex
import subprocess
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Protocol, runtime_checkable

import numpy as np

from ..audio import write_wav_atomic

SUBPROCESS_TIMEOUT_SECONDS = 300.0
MAX_ADAPTER_STDOUT_BYTES = 1 << 20


class DetectorError(RuntimeError):
    """A detector could not be loaded, or returned something that is not a Detection."""


@dataclass(frozen=True)
class Detection:
    """What a blind detector is permitted to return.

    `presence` is the zero-bit tier of spec 3.5; `payload_hex` is the locator tier of spec 3.1 and is
    None whenever the CRC gate did not pass. A detector that returns a payload it could not CRC-gate
    is reporting a different quantity from the one every kill criterion is defined on.
    """

    payload_hex: str | None = None
    presence: bool = False
    presence_score: float | None = None
    confidence: float = 0.0
    bits_corrected: int = 0
    findings: tuple[str, ...] = field(default=())
    error: str | None = None

    @staticmethod
    def from_dict(raw: Any) -> "Detection":
        if not isinstance(raw, dict):
            raise DetectorError(f"detector returned {type(raw).__name__}, expected a JSON object")
        payload = raw.get("payload_hex")
        if payload is not None:
            if not isinstance(payload, str) or not payload:
                raise DetectorError(f"payload_hex must be a non-empty hex string or null, got {payload!r}")
            try:
                bytes.fromhex(payload)
            except ValueError as exc:
                raise DetectorError(f"payload_hex {payload!r} is not hexadecimal") from exc
        bits_corrected = int(raw.get("bits_corrected", 0))
        if bits_corrected < 0:
            raise DetectorError("bits_corrected cannot be negative")
        return Detection(
            payload_hex=payload,
            presence=bool(raw.get("presence", False)),
            presence_score=None if raw.get("presence_score") is None else float(raw["presence_score"]),
            confidence=float(raw.get("confidence", 0.0)),
            bits_corrected=bits_corrected,
            findings=tuple(str(f) for f in raw.get("findings", ())),
            error=None if raw.get("error") is None else str(raw["error"]),
        )

    def to_dict(self) -> dict:
        return {
            "payload_hex": self.payload_hex,
            "presence": self.presence,
            "presence_score": self.presence_score,
            "confidence": self.confidence,
            "bits_corrected": self.bits_corrected,
            "findings": list(self.findings),
            "error": self.error,
        }


@runtime_checkable
class Detector(Protocol):
    def detect(self, audio: np.ndarray, thresholds: dict) -> Detection: ...


class NullDetector:
    """Declines everything.

    Not a placeholder: it is the pipeline's own control. Run over the campaign it must produce
    exact_recovery_rate 0.0 and false_positive.rate 0.0, which proves the report arithmetic is not
    manufacturing detections, and it is what a campaign runs before a real model exists.
    """

    name = "null"

    def detect(self, audio: np.ndarray, thresholds: dict) -> Detection:
        return Detection(findings=("null detector: declines every trial by construction",))


class SubprocessDetector:
    """Runs an external detector once per trial over a stdout JSON contract.

    The capture is copied to a temporary file named `trial.wav`, carrying no room, speaker, distance,
    arm or clip identifier. A detector that could read the arm out of a path would not be blind, and
    the campaign's entire value is that its numbers are.
    """

    def __init__(self, command: str, sample_rate: int, timeout_seconds: float = SUBPROCESS_TIMEOUT_SECONDS) -> None:
        self.argv = shlex.split(command)
        if not self.argv:
            raise DetectorError("exec: detector spec is empty")
        self.sample_rate = sample_rate
        self.timeout_seconds = timeout_seconds
        self.name = f"exec:{self.argv[0]}"

    def detect(self, audio: np.ndarray, thresholds: dict) -> Detection:
        with tempfile.TemporaryDirectory(prefix="capture-rig-trial-") as directory:
            root = Path(directory)
            wav = write_wav_atomic(root / "trial.wav", audio, self.sample_rate, subtype="FLOAT")
            threshold_path = root / "thresholds.json"
            threshold_path.write_text(json.dumps(thresholds, indent=2), encoding="utf-8")
            argv = [*self.argv, "--audio", str(wav), "--thresholds", str(threshold_path)]
            try:
                completed = subprocess.run(
                    argv,
                    capture_output=True,
                    timeout=self.timeout_seconds,
                    check=False,
                    text=False,
                )
            except (OSError, subprocess.TimeoutExpired) as exc:
                raise DetectorError(f"{self.name}: {exc}") from exc
        if completed.returncode != 0:
            stderr = completed.stderr[-2048:].decode("utf-8", errors="replace")
            raise DetectorError(f"{self.name}: exited {completed.returncode}: {stderr}")
        if len(completed.stdout) > MAX_ADAPTER_STDOUT_BYTES:
            raise DetectorError(f"{self.name}: emitted more than {MAX_ADAPTER_STDOUT_BYTES} bytes on stdout")
        try:
            return Detection.from_dict(json.loads(completed.stdout.decode("utf-8")))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise DetectorError(f"{self.name}: stdout is not JSON ({exc})") from exc


def load_detector(spec: str, sample_rate: int, options: dict | None = None) -> Detector:
    """Resolve a detector spec.

      null                       the declining control detector
      python:<module>:<factory>  factory(sample_rate=..., options=...) -> object with .detect
      exec:<command>             external process over the stdout JSON contract
    """
    options = options or {}
    if spec == "null":
        return NullDetector()
    if spec.startswith("exec:"):
        return SubprocessDetector(spec[len("exec:"):], sample_rate)
    if spec.startswith("python:"):
        remainder = spec[len("python:"):]
        if ":" not in remainder:
            raise DetectorError(f"python detector spec must be python:<module>:<factory>, got {spec!r}")
        module_name, factory_name = remainder.rsplit(":", 1)
        try:
            module = importlib.import_module(module_name)
        except ImportError as exc:
            raise DetectorError(f"cannot import detector module {module_name!r}: {exc}") from exc
        factory = getattr(module, factory_name, None)
        if factory is None:
            raise DetectorError(f"{module_name!r} has no attribute {factory_name!r}")
        detector = factory(sample_rate=sample_rate, options=options)
        if not hasattr(detector, "detect"):
            raise DetectorError(f"{spec}: factory returned an object with no detect() method")
        return detector
    raise DetectorError(f"unrecognised detector spec {spec!r}; expected null, python:... or exec:...")
