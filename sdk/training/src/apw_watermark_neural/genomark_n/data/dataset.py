"""The training item, and why it is not (audio, bits).

Spec 9.4's L_det needs a PER-FRAME marked/unmarked label, and spec 6 D2(b) makes the marked region
a random contiguous fraction of the clip. A dataset yielding only (audio, payload) cannot express
either, so the presence head would have nothing to learn against and the localisation claim in
spec 4.4(b) would be false. Each item therefore carries the cover audio, the payload, whether the
item is marked at all (50% of every batch is unmarked cover), and the marked span as FRACTIONS of
the analysis window. The span becomes samples in `collate`, after the batch's analysis length is
drawn, and frames only after the hop is known.

D1 (source codec history) is applied here because it precedes embedding and is a property of the
source clip, which is the one codec stage spec 6 permits to be cached.
D2(a) is the random SAMPLE-level offset: a sample-level read out of a longer source clip, never a
spectrogram roll (which creates a wrap discontinuity the decoder can key on) and never a
frame-aligned shift (which lets the model lean on STFT frame phase).
"""

import math
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import soundfile as sf
import torch
from torch.utils.data import Dataset

from ..config import CodecConfig, DataConfig, DistortionConfig, PayloadConfig
from ..payload import MessageCodec
from ..seeding import example_seed
from .manifest import CorpusEntry, load_manifest
from .synthetic import SyntheticCorpus


@dataclass
class TrainingItem:
    audio: np.ndarray
    bits: np.ndarray
    is_marked: bool
    span_offset_fraction: float
    span_length_fraction: float
    content_class: str
    source: str


class NeuralWatermarkDataset(Dataset):
    def __init__(
        self,
        data: DataConfig,
        distortion: DistortionConfig,
        codec: CodecConfig,
        payload: PayloadConfig,
        sample_rate: int,
        epoch_size: int,
        base_seed: int,
        unmarked_fraction: float = 0.5,
        synthetic: SyntheticCorpus | None = None,
        apply_source_codec: bool = True,
    ) -> None:
        self.data = data
        self.distortion = distortion
        self.codec_config = codec
        self.codec = MessageCodec(payload)
        self.sample_rate = sample_rate
        self.epoch_size = epoch_size
        self.base_seed = base_seed
        self.unmarked_fraction = unmarked_fraction
        self.epoch = 0
        self.apply_source_codec = apply_source_codec
        self.entries: list[tuple[CorpusEntry, Path]] = []
        for manifest in data.manifests:
            root = Path(manifest).parent
            for entry in load_manifest(manifest):
                self.entries.append((entry, entry.resolve(root)))
        self.synthetic = synthetic
        if not self.entries and self.synthetic is None:
            raise ValueError(
                "no licence-cleared corpus manifests and no synthetic corpus: a dataset that "
                "silently yields nothing would let a run report a result over no data"
            )

    def __len__(self) -> int:
        return self.epoch_size

    def set_epoch(self, epoch: int) -> None:
        self.epoch = epoch

    @property
    def max_analysis_samples(self) -> int:
        return int(math.ceil(self.data.analysis_seconds[1] * self.sample_rate))

    def _load_source(self, rng: np.random.Generator) -> tuple[np.ndarray, str, str]:
        if self.entries:
            entry, path = self.entries[int(rng.integers(0, len(self.entries)))]
            info = sf.info(str(path))
            want = int(self.data.source_clip_seconds * info.samplerate)
            start = int(rng.integers(0, max(info.frames - want, 1)))
            audio, rate = sf.read(str(path), start=start, frames=want, dtype="float32", always_2d=True)
            mono = audio.mean(axis=1)
            if rate != self.sample_rate:
                duration = mono.size / rate
                target = int(round(duration * self.sample_rate))
                mono = np.interp(
                    np.arange(target) / self.sample_rate,
                    np.arange(mono.size) / rate,
                    mono,
                ).astype(np.float32)
            return mono, "corpus", entry.source
        synthetic = self.synthetic
        if synthetic is None:
            raise ValueError("dataset has no corpus entries and no synthetic corpus")
        index = int(rng.integers(0, len(synthetic)))
        return synthetic.load(index), synthetic.content_class(index), "synthetic"

    def _source_codec(self, audio: np.ndarray, rng: np.random.Generator) -> np.ndarray:
        from ..distortion.codec import CodecError, _round_trip

        specs = self.codec_config.source_codecs
        spec = specs[int(rng.integers(0, len(specs)))]
        codec, _, bitrate = spec.partition(":")
        try:
            return _round_trip(
                audio, self.sample_rate, codec, int(bitrate), self.codec_config.ffmpeg,
                self.codec_config.timeout_s,
            )
        except CodecError:
            return audio

    def __getitem__(self, index: int) -> TrainingItem:
        rng = np.random.default_rng(example_seed(self.base_seed, self.epoch, index))
        source, content_class, source_name = self._load_source(rng)

        if self.apply_source_codec and rng.random() < self.distortion.source_codec_p:
            source = self._source_codec(source, rng)

        want = self.max_analysis_samples
        if source.size < want:
            source = np.pad(source, (0, want - source.size))
        offset = int(rng.integers(0, source.size - want + 1))
        window = source[offset : offset + want].astype(np.float32)

        is_marked = rng.random() >= self.unmarked_fraction
        low, high = self.distortion.marked_fraction_range
        length_fraction = float(rng.uniform(low, high)) if is_marked else 0.0
        offset_fraction = float(rng.uniform(0.0, 1.0 - length_fraction)) if is_marked else 0.0
        message = self.codec.random_message(rng)
        return TrainingItem(
            audio=window,
            bits=self.codec.encode(message),
            is_marked=is_marked,
            span_offset_fraction=offset_fraction,
            span_length_fraction=length_fraction,
            content_class=content_class,
            source=source_name,
        )


@dataclass
class Batch:
    audio: torch.Tensor
    bits: torch.Tensor
    is_marked: torch.Tensor
    span_start: torch.Tensor
    span_end: torch.Tensor
    content_classes: list[str]
    analysis_samples: int


def collate(items: list[TrainingItem], analysis_seconds: tuple[float, float],
            sample_rate: int, generator: np.random.Generator) -> Batch:
    """Spec 4.4(c): the analysis length is sampled from U(3 s, 5 s).

    It is drawn ONCE PER BATCH rather than once per item, because a per-item length cannot be
    stacked into a tensor without padding, and padding would hand the decoder a silent region whose
    boundary is a synchronisation cue that no real capture provides.
    """
    seconds = float(generator.uniform(*analysis_seconds))
    length = int(seconds * sample_rate)
    audio = torch.from_numpy(np.stack([item.audio[:length] for item in items]))
    bits = torch.from_numpy(np.stack([item.bits for item in items])).float()
    is_marked = torch.tensor([item.is_marked for item in items], dtype=torch.bool)
    starts, ends = [], []
    for item in items:
        if not item.is_marked:
            starts.append(0)
            ends.append(0)
            continue
        start = int(item.span_offset_fraction * length)
        end = min(length, start + max(int(item.span_length_fraction * length), 1))
        starts.append(start)
        ends.append(end)
    return Batch(
        audio=audio,
        bits=bits,
        is_marked=is_marked,
        span_start=torch.tensor(starts, dtype=torch.long),
        span_end=torch.tensor(ends, dtype=torch.long),
        content_classes=[item.content_class for item in items],
        analysis_samples=length,
    )
