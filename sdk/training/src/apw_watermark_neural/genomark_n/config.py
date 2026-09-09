"""Every hyperparameter WATERMARK_N_SPEC.md fixes, in one typed tree.

Defaults are the spec's Phase C values. A YAML file overlays a subset; nothing is invented at
call sites, so a run is fully described by (config file, seed, git revision).
"""

import dataclasses
import json
import typing
from dataclasses import dataclass, field
from pathlib import Path

import yaml

SPEC = "docs/WATERMARK_N_SPEC.md"
ALGORITHM_ID = "apw-watermark-neural-v1"


class ConfigError(ValueError):
    """A config value violates a bound the spec states as hard."""


@dataclass(frozen=True)
class StftConfig:
    """Spec 2.3 / 2.4. The band is inclusive at both ends."""

    sample_rate: int = 48_000
    n_fft: int = 2048
    hop: int = 512
    band_first_bin: int = 9
    band_last_bin: int = 328

    @property
    def band_bins(self) -> int:
        return self.band_last_bin - self.band_first_bin + 1

    @property
    def bin_hz(self) -> float:
        return self.sample_rate / self.n_fft

    @property
    def band_low_hz(self) -> float:
        return self.band_first_bin * self.bin_hz

    @property
    def band_high_hz(self) -> float:
        return self.band_last_bin * self.bin_hz

    @property
    def frames_per_second(self) -> float:
        return self.sample_rate / self.hop

    def validate(self) -> None:
        if self.band_bins != 320:
            raise ConfigError(f"spec 2.4 fixes 320 band bins, config gives {self.band_bins}")
        if self.band_bins % 16 != 0:
            raise ConfigError("four frequency-halving stages need band_bins divisible by 16")
        if self.band_last_bin >= self.n_fft // 2 + 1:
            raise ConfigError("band_last_bin is above the Nyquist bin")


@dataclass(frozen=True)
class PayloadConfig:
    """Spec 3.1 / 3.2. Widening flip_search without widening the CRC is how a false identity ships."""

    version_bits: int = 3
    namespace_bits: int = 4
    locator_bits: int = 25
    crc_bits: int = 24
    crc_poly: int = 0x864CFB
    flip_candidates: int = 4
    flip_max_weight: int = 2
    version_value: int = 1

    @property
    def message_bits(self) -> int:
        return self.version_bits + self.namespace_bits + self.locator_bits + self.crc_bits

    @property
    def covered_bits(self) -> int:
        return self.version_bits + self.namespace_bits + self.locator_bits

    def validate(self) -> None:
        if self.message_bits != 56:
            raise ConfigError(f"spec 3.1 fixes a 56-bit message, config gives {self.message_bits}")
        if (self.flip_candidates, self.flip_max_weight) != (4, 2):
            raise ConfigError(
                "spec 3.2/3.4 fix k=2 over the 4 least-confident bits; a wider search must be "
                "paid for with a wider CRC and a redone false-accept count"
            )


@dataclass(frozen=True)
class PerceptualConfig:
    """Spec 2.7 / 9.4 L_perc. Ported from crates/audio-provenance-bench/src/perceptual.rs."""

    frame: int = 2048
    hop: int = 1024
    full_scale_spl_db: float = 96.0
    tonality_alpha: float = 0.5
    silence_floor_dbfs: float = -80.0
    bark_partition_width: float = 0.5
    threshold_in_quiet_max_spl_db: float = 90.0


@dataclass(frozen=True)
class BudgetConfig:
    """Spec 2.7. B_max is a ceiling no calibration may raise."""

    kappa: float = 0.5
    b_max_nepers: float = 0.35

    def validate(self) -> None:
        if self.b_max_nepers > 0.35:
            raise ConfigError(
                "spec 2.7: B_max = 0.35 nepers is a hard ceiling; needing more than 3.04 dB of "
                "per-bin swing kills the program rather than raising the ceiling"
            )
        if self.kappa <= 0.0:
            raise ConfigError("kappa must be positive")


@dataclass(frozen=True)
class EncoderConfig:
    """Spec 2.5. Downsampling is on frequency only; time resolution is preserved end to end."""

    stem_channels: int = 32
    stage_channels: tuple[int, ...] = (64, 128, 256, 256)
    message_embed_dim: int = 64
    bottleneck_time_dilation: int = 2
    group_norm_groups: int = 8


@dataclass(frozen=True)
class DecoderConfig:
    """Spec 2.6."""

    stem_channels: int = 48
    stage_channels: tuple[int, ...] = (96, 192, 256, 256)
    presence_hidden: int = 64
    group_norm_groups: int = 8
    tau_init: float = 1.0


@dataclass(frozen=True)
class QCoexistenceConfig:
    """Spec 8.2 and apw_watermark's params.rs. The grid is Q's, not N's."""

    reference_sample_rate: int = 44_100
    frame: int = 1024
    band_first_bin: int = 20
    cells: int = 40
    frames_per_slot: int = 2
    trim_each_tail: int = 2
    guard_hz: tuple[float, ...] = (1000.0, 1200.0, 2000.0, 2500.0, 3000.0, 3500.0)
    delta: float = 0.8
    residual_limit_nepers: float = 0.04
    q_gain_db: float = 0.43


@dataclass(frozen=True)
class DistortionConfig:
    """Spec 6, Phase C ranges. `probability_scale` is the curriculum ramp, not a spec value."""

    q_embed_p: float = 0.8
    source_codec_p: float = 0.5
    marked_fraction_range: tuple[float, float] = (0.2, 1.0)
    playback_gain_dbfs: tuple[float, float] = (-18.0, 3.0)
    speaker_nonlinearity_p: float = 0.7
    thd_range: tuple[float, float] = (0.003, 0.03)
    thermal_compression_db: float = 2.0
    thermal_time_constant_s: float = 0.2
    transducer_p: float = 1.0
    highpass_hz: tuple[float, float] = (60.0, 160.0)
    lowpass_hz: tuple[float, float] = (11_000.0, 18_000.0)
    peaking_sections: tuple[int, int] = (3, 5)
    peaking_centre_hz: tuple[float, float] = (200.0, 8000.0)
    peaking_gain_db: tuple[float, float] = (-6.0, 6.0)
    peaking_q: tuple[float, float] = (0.7, 3.0)
    rir_p: float = 1.0
    drr_db: tuple[float, float] = (-6.0, 18.0)
    noise_p: float = 1.0
    noise_snr_db: tuple[float, float] = (15.0, 40.0)
    room_noise_tilt_db_per_octave: tuple[float, float] = (-6.0, -3.0)
    agc_p: float = 0.5
    agc_ratio: tuple[float, float] = (2.0, 6.0)
    agc_attack_ms: tuple[float, float] = (5.0, 50.0)
    agc_release_ms: tuple[float, float] = (50.0, 500.0)
    agc_makeup_time_constant_s: float = 3.0
    drift_ppm: tuple[float, float] = (-300.0, 300.0)
    resample_round_trip_p: float = 0.3
    quantise_bits: int = 16
    quantise_24bit_p: float = 0.2
    capture_codec_p: float = 0.7
    electronic_only_p: float = 0.35


@dataclass(frozen=True)
class CodecConfig:
    """Spec 6 D1 / D11. The forward pass is a real encoder; the backward pass is identity."""

    ffmpeg: str = "/opt/homebrew/bin/ffmpeg"
    source_codecs: tuple[str, ...] = ("mp3:128", "mp3:320", "aac:128", "aac:256", "opus:96", "opus:192")
    capture_codecs: tuple[str, ...] = ("aac:64", "aac:256", "opus:32", "opus:128", "mp3:96", "mp3:320")
    timeout_s: float = 60.0


@dataclass(frozen=True)
class RirConfig:
    """Spec 9.2. Corpora are licence-gated; the synthesizer removes the data dependency."""

    corpus_dirs: tuple[str, ...] = ()
    synthetic_fraction: float = 1.0
    room_dims_m_min: tuple[float, float, float] = (2.8, 2.4, 2.3)
    room_dims_m_max: tuple[float, float, float] = (9.0, 7.0, 3.6)
    rt60_s: tuple[float, float] = (0.18, 0.80)
    source_distance_m: tuple[float, float] = (0.3, 3.0)
    max_order: int = 12
    max_ir_seconds: float = 1.5
    hf_rt60_ratio: float = 0.55
    fractional_delay_taps: int = 33
    interpolation: str = "windowed_sinc"
    pool_size: int = 64


@dataclass(frozen=True)
class DataConfig:
    """Spec 9.1. `manifests` are JSONL files listing per-track path + licence."""

    manifests: tuple[str, ...] = ()
    noise_manifests: tuple[str, ...] = ()
    source_clip_seconds: float = 10.0
    analysis_seconds: tuple[float, float] = (3.0, 5.0)
    cache_source_codec: bool = True
    synthetic_items: int = 0
    num_workers: int = 0


@dataclass(frozen=True)
class LossConfig:
    """Spec 9.4."""

    message: float = 1.0
    message_per_frame: float = 0.2
    detection: float = 1.0
    perceptual: float = 20.0
    spectral: float = 4.0
    q_coexistence: float = 5.0
    adversarial: float = 0.1
    adversarial_enabled: bool = False
    spectral_n_ffts: tuple[int, ...] = (512, 1024, 2048, 4096)


@dataclass(frozen=True)
class OptimConfig:
    """Spec 9.5."""

    lr: float = 1e-4
    betas: tuple[float, float] = (0.9, 0.99)
    weight_decay: float = 1e-2
    warmup_steps: int = 2000
    total_steps: int = 320_000
    min_lr: float = 1e-6
    grad_clip: float = 1.0
    batch_size: int = 16
    ema_decay: float = 0.9999
    unmarked_fraction: float = 0.5


@dataclass(frozen=True)
class CurriculumConfig:
    """Spec 9.3."""

    phase_a_end: int = 60_000
    phase_b_end: int = 180_000
    phase_c_end: int = 320_000


@dataclass(frozen=True)
class DetectorConfig:
    """Spec 4.5 / 3.5. Thresholds live in a frozen calibration artifact, never in the eval run."""

    window_seconds: float = 30.0
    window_hop_seconds: float = 15.0
    presence_window_seconds: float = 10.0
    presence_smooth_seconds: float = 0.5
    presence_frame_threshold: float = 0.5
    min_presence_span_seconds: float = 8.0
    min_locator_seconds: float = 20.0
    max_windows: int = 64
    calibration_path: str | None = None

    def validate(self) -> None:
        if self.min_locator_seconds < self.min_presence_span_seconds:
            raise ConfigError(
                "min_locator_seconds must be at least min_presence_span_seconds: a window that "
                "cannot hold the required presence span can never pass the message gate"
            )
        if self.max_windows < 1 or self.max_windows > 64:
            raise ConfigError("spec 3.4 budgets 64 windows per file as the design maximum")
        if self.window_hop_seconds > self.window_seconds:
            raise ConfigError("window hop must not exceed the window length")


@dataclass(frozen=True)
class EvalConfig:
    """Spec 12.2. Target FPR is what the calibration solves for, not what the eval reports."""

    channels: tuple[str, ...] = (
        "identity",
        "mp3_128",
        "aac_128",
        "opus_64",
        "resample_via_44100",
        "gain_minus_6db",
        "noise_snr_30db",
        "lowpass_16k",
        "drift_plus_0p1pct",
        "acoustic_small_room",
        "acoustic_1m_room",
        "acoustic_medium_room",
    )
    corpus_items: int = 200
    clip_seconds: float = 30.0
    false_positive_trials: int = 5000
    target_false_positive_rate: float = 1e-3


@dataclass(frozen=True)
class RunConfig:
    seed: int = 20260831
    device: str = "cpu"
    out_dir: str = "runs/dev"
    log_every: int = 10
    checkpoint_every: int = 1000
    max_steps: int | None = None
    deterministic: bool = True


@dataclass(frozen=True)
class Config:
    run: RunConfig = field(default_factory=RunConfig)
    stft: StftConfig = field(default_factory=StftConfig)
    payload: PayloadConfig = field(default_factory=PayloadConfig)
    perceptual: PerceptualConfig = field(default_factory=PerceptualConfig)
    budget: BudgetConfig = field(default_factory=BudgetConfig)
    encoder: EncoderConfig = field(default_factory=EncoderConfig)
    decoder: DecoderConfig = field(default_factory=DecoderConfig)
    q: QCoexistenceConfig = field(default_factory=QCoexistenceConfig)
    distortion: DistortionConfig = field(default_factory=DistortionConfig)
    codec: CodecConfig = field(default_factory=CodecConfig)
    rir: RirConfig = field(default_factory=RirConfig)
    data: DataConfig = field(default_factory=DataConfig)
    loss: LossConfig = field(default_factory=LossConfig)
    optim: OptimConfig = field(default_factory=OptimConfig)
    curriculum: CurriculumConfig = field(default_factory=CurriculumConfig)
    detector: DetectorConfig = field(default_factory=DetectorConfig)
    evaluation: EvalConfig = field(default_factory=EvalConfig)

    def validate(self) -> None:
        self.stft.validate()
        self.payload.validate()
        self.budget.validate()
        self.detector.validate()

    def to_dict(self) -> dict:
        return dataclasses.asdict(self)

    def to_json(self) -> str:
        return json.dumps(self.to_dict(), indent=2, sort_keys=True)


def _coerce(annotation, value):
    origin = typing.get_origin(annotation)
    if origin is tuple:
        args = typing.get_args(annotation)
        if len(args) == 2 and args[1] is Ellipsis:
            return tuple(_coerce(args[0], item) for item in value)
        return tuple(_coerce(arg, item) for arg, item in zip(args, value, strict=True))
    if origin is typing.Union or origin is type(int | None):
        args = [arg for arg in typing.get_args(annotation) if arg is not type(None)]
        if value is None:
            return None
        return _coerce(args[0], value)
    if dataclasses.is_dataclass(annotation):
        return _build(annotation, value)
    if annotation in (int, float, str, bool):
        return annotation(value)
    return value


def _build(cls, data: dict):
    if data is None:
        return cls()
    hints = typing.get_type_hints(cls)
    known = {f.name for f in dataclasses.fields(cls)}
    unknown = set(data) - known
    if unknown:
        raise ConfigError(f"{cls.__name__} has no field(s) {sorted(unknown)}")
    kwargs = {name: _coerce(hints[name], value) for name, value in data.items()}
    return cls(**kwargs)


def load_config(path: str | Path | None = None, overrides: dict | None = None) -> Config:
    data: dict = {}
    if path is not None:
        loaded = yaml.safe_load(Path(path).read_text(encoding="utf-8"))
        if loaded is not None:
            data = loaded
    if overrides:
        for section, values in overrides.items():
            data.setdefault(section, {}).update(values)
    config = _build(Config, data)
    config.validate()
    return config
