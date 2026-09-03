"""Impulse-response sources: measured corpora (licence-gated) and the image-source synthesizer.

Spec 9.2's priority order is Audio Provenance's own Stage 0 measurements, then MIT Acoustical Reverberation
Scene Statistics (CC BY 4.0), then EchoThief. Nothing measured exists yet, so a run with no corpus
directories is 100% synthetic and the model card records that, rather than a run silently believing
it trained on real rooms.
"""

import enum
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import soundfile as sf

from ..config import RirConfig
from ..data.manifest import load_manifest
from .image_source import RirPair, sample_room, synthesise_rir


class RirSource(enum.StrEnum):
    SYNTHETIC = "image_source_synthetic"
    MEASURED = "measured_corpus"


@dataclass(frozen=True)
class DrawnRir:
    pair: RirPair | None
    impulse: np.ndarray
    source: RirSource
    identifier: str
    drr_db: float


def _resample_linear(signal: np.ndarray, source_rate: int, target_rate: int) -> np.ndarray:
    if source_rate == target_rate:
        return signal
    duration = signal.size / source_rate
    target_len = max(int(round(duration * target_rate)), 2)
    source_t = np.arange(signal.size, dtype=np.float64) / source_rate
    target_t = np.arange(target_len, dtype=np.float64) / target_rate
    return np.interp(target_t, source_t, signal).astype(np.float32)


class RirCorpus:
    """Draws an impulse response, from measured files where they exist and synthesis otherwise."""

    def __init__(self, config: RirConfig, sample_rate: int) -> None:
        self.config = config
        self.sample_rate = sample_rate
        self.measured: list[tuple[Path, str]] = []
        self._pool: list[RirPair] = []
        for directory in config.corpus_dirs:
            root = Path(directory)
            manifest = root / "manifest.jsonl"
            if not manifest.exists():
                raise FileNotFoundError(
                    f"{root}: an impulse-response directory must carry manifest.jsonl declaring the "
                    "licence and source of every file. Spec 9.2 makes the licence gating."
                )
            for entry in load_manifest(manifest):
                self.measured.append((entry.resolve(root), entry.source))

    @property
    def measured_count(self) -> int:
        return len(self.measured)

    def provenance(self) -> dict:
        return {
            "measured_files": self.measured_count,
            "synthetic_pool_size": self.config.pool_size,
            "synthetic_fraction": self.config.synthetic_fraction if self.measured else 1.0,
            "synthesizer": "image_source_eyring_windowed_sinc",
            "note": (
                "No measured impulse responses are present in this run."
                if not self.measured
                else "Measured and synthetic responses are mixed per synthetic_fraction."
            ),
        }

    def _synthesise(self, rng: np.random.Generator) -> RirPair:
        room = sample_room(
            rng,
            self.config.room_dims_m_min,
            self.config.room_dims_m_max,
            self.config.rt60_s,
            self.config.source_distance_m,
            self.config.max_order,
            self.config.fractional_delay_taps,
            self.config.hf_rt60_ratio,
        )
        return synthesise_rir(room, self.sample_rate, self.config.max_ir_seconds, rng)

    def draw(self, rng: np.random.Generator, drr_db: float) -> DrawnRir:
        use_synthetic = not self.measured or rng.random() < self.config.synthetic_fraction
        if use_synthetic:
            # PERF: image-source synthesis costs 0.3-1.2 s per room, which is several times a
            # training step. A pool of rooms is drawn once and reused; pool_size = 0 synthesises
            # every draw, which is correct but only affordable offline.
            if self.config.pool_size > 0:
                while len(self._pool) < self.config.pool_size:
                    self._pool.append(self._synthesise(rng))
                pair = self._pool[int(rng.integers(0, len(self._pool)))]
                identifier = (
                    f"pool_rt60_{pair.measured_rt60_seconds:.2f}s_d{pair.source_distance_m:.2f}m"
                    if pair.measured_rt60_seconds
                    else f"pool_d{pair.source_distance_m:.2f}m"
                )
                return DrawnRir(pair=pair, impulse=pair.at_drr(drr_db),
                                source=RirSource.SYNTHETIC, identifier=identifier, drr_db=drr_db)
            room = sample_room(
                rng,
                self.config.room_dims_m_min,
                self.config.room_dims_m_max,
                self.config.rt60_s,
                self.config.source_distance_m,
                self.config.max_order,
                self.config.fractional_delay_taps,
                self.config.hf_rt60_ratio,
            )
            pair = synthesise_rir(room, self.sample_rate, self.config.max_ir_seconds, rng)
            identifier = (
                f"synth_rt60_{room.rt60_seconds:.2f}s_d{room.source_distance_m:.2f}m_"
                f"{room.room_dims_m[0]:.1f}x{room.room_dims_m[1]:.1f}x{room.room_dims_m[2]:.1f}"
            )
            return DrawnRir(pair=pair, impulse=pair.at_drr(drr_db), source=RirSource.SYNTHETIC,
                            identifier=identifier, drr_db=drr_db)

        path, source = self.measured[int(rng.integers(0, len(self.measured)))]
        data, rate = sf.read(str(path), dtype="float32", always_2d=True)
        impulse = _resample_linear(data[:, 0], rate, self.sample_rate)
        limit = int(self.config.max_ir_seconds * self.sample_rate)
        impulse = impulse[:limit]
        energy = float((impulse**2).sum())
        if energy > 0.0:
            impulse = impulse / np.sqrt(energy)
        return DrawnRir(pair=None, impulse=impulse, source=RirSource.MEASURED,
                        identifier=f"{source}:{path.name}", drr_db=float("nan"))
