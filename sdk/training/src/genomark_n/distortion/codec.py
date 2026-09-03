"""Spec 6 D1 / D11: a REAL encoder/decoder in the forward pass, identity gradient on the backward.

SilentCipher's ablation drops MP3 accuracy to zero when this layer is removed, so it is not
optional. D11 also cannot be pre-rendered: it follows AGC and quantisation in the physical chain,
and caching it would model a chain that does not exist.
"""

import shutil
import subprocess
import tempfile
from pathlib import Path

import numpy as np
import soundfile as sf
import torch

_EXTENSION = {"mp3": "mp3", "aac": "m4a", "opus": "opus"}
_ENCODER = {"mp3": "libmp3lame", "aac": "aac", "opus": "libopus"}


class CodecError(RuntimeError):
    """ffmpeg refused a round trip. Never swallowed: a silently skipped codec stage trains a model
    against a chain that is not the one the report claims."""


def codec_available(ffmpeg: str) -> bool:
    return shutil.which(ffmpeg) is not None or Path(ffmpeg).exists()


def _round_trip(samples: np.ndarray, sample_rate: int, codec: str, bitrate_kbps: int,
                ffmpeg: str, timeout_s: float) -> np.ndarray:
    kind = codec.lower()
    if kind not in _EXTENSION:
        raise CodecError(f"unsupported codec {codec!r}")
    with tempfile.TemporaryDirectory(prefix="genomark-n-codec-") as directory:
        root = Path(directory)
        source = root / "in.wav"
        coded = root / f"coded.{_EXTENSION[kind]}"
        decoded = root / "out.wav"
        sf.write(str(source), samples.astype(np.float32), sample_rate, subtype="FLOAT")
        encode = [
            ffmpeg, "-hide_banner", "-loglevel", "error", "-y", "-i", str(source),
            "-c:a", _ENCODER[kind], "-b:a", f"{bitrate_kbps}k", str(coded),
        ]
        decode = [
            ffmpeg, "-hide_banner", "-loglevel", "error", "-y", "-i", str(coded),
            "-ar", str(sample_rate), "-c:a", "pcm_f32le", str(decoded),
        ]
        for command in (encode, decode):
            finished = subprocess.run(command, capture_output=True, timeout=timeout_s, check=False)
            if finished.returncode != 0:
                raise CodecError(
                    f"{' '.join(command[:6])}... failed rc={finished.returncode}: "
                    f"{finished.stderr.decode('utf-8', 'replace').strip()[:400]}"
                )
        data, rate = sf.read(str(decoded), dtype="float32", always_2d=True)
    if rate != sample_rate:
        raise CodecError(f"decoder returned {rate} Hz, expected {sample_rate}")
    out = data[:, 0]
    if out.size < samples.size:
        return np.pad(out, (0, samples.size - out.size))
    return out[: samples.size]


class _StraightThrough(torch.autograd.Function):
    @staticmethod
    def forward(ctx, audio: torch.Tensor, transform) -> torch.Tensor:
        return transform(audio.detach())

    # FIXME: torch stubs type `backward` from `_SingleLevelFunction` with a fixed arity that no
    # real autograd.Function matches. Upstream stub wart, not a signature error here.
    @staticmethod
    def backward(ctx, grad_output: torch.Tensor):  # pyright: ignore[reportIncompatibleMethodOverride]
        return grad_output, None


class CodecRoundTrip:
    """Callable stage. Encoder delay and padding are NOT removed: bulk delay is exactly what the
    position-agnostic readout is claimed to absorb, and stripping it here would train that claim
    out of the model."""

    def __init__(self, ffmpeg: str, timeout_s: float = 60.0) -> None:
        self.ffmpeg = ffmpeg
        self.timeout_s = timeout_s
        self.calls = 0

    def available(self) -> bool:
        return codec_available(self.ffmpeg)

    def __call__(self, audio: torch.Tensor, spec: str, sample_rate: int) -> torch.Tensor:
        codec, _, bitrate = spec.partition(":")
        rate_kbps = int(bitrate) if bitrate else 128

        def transform(batch: torch.Tensor) -> torch.Tensor:
            device, dtype = batch.device, batch.dtype
            numpy_batch = batch.detach().to("cpu", torch.float32).numpy()
            rows = [
                _round_trip(row, sample_rate, codec, rate_kbps, self.ffmpeg, self.timeout_s)
                for row in np.atleast_2d(numpy_batch)
            ]
            self.calls += len(rows)
            stacked = np.stack(rows).reshape(numpy_batch.shape)
            return torch.from_numpy(stacked).to(device=device, dtype=dtype)

        return _StraightThrough.apply(audio, transform)
