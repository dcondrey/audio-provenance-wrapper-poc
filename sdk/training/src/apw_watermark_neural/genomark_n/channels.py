"""Evaluation channels, named and parameterised to mirror crates/audio-provenance-bench.

Same names, same room presets, same transducer cascade, same level matching, so a number produced
here is comparable to a number in the Rust bench report rather than merely similar in spirit.

TWO RULES COPIED FROM THE BENCH, both load-bearing:
  - the channel NEVER realigns the audio before detection ("sync is the detector's job"), so the
    room's propagation delay and the codec's encoder delay stay in the signal;
  - output is truncated to the input length, exactly as AcousticRerecord does.
`acoustic_1m_room` is spec 10.5's proposed RoomPreset::NEAR_FIELD_1M; it does not exist in the Rust
bench yet and is marked `proposed` in its parameters so a report cannot imply otherwise.
"""

import zlib
from collections.abc import Callable
from dataclasses import dataclass, field

import numpy as np
import torch

from .config import CodecConfig, RirConfig
from .distortion.codec import CodecError, CodecRoundTrip
from .distortion.dsp import fft_convolve, fft_filter, high_pass, low_pass, peaking, resample_linear
from .rir.image_source import ImageSourceRoom, synthesise_rir

SIMULATION_DISCLAIMER = (
    "SIMULATED acoustic path. The impulse response is synthesised by the image-source method with "
    "Eyring absorption and windowed-sinc fractional delays, the transducer curve is a fixed filter "
    "cascade, and the room noise is Gaussian. This row is NOT a speaker-to-microphone measurement "
    "and may not be reported as one."
)


@dataclass(frozen=True)
class RoomPreset:
    rt60_seconds: float
    source_distance_m: float
    room_noise_snr_db: float
    clock_drift: float
    direct_to_reverberant_db: float
    proposed: bool = False


PRESETS = {
    "acoustic_small_room": RoomPreset(0.28, 0.5, 40.0, 1.0001, 8.0),
    "acoustic_1m_room": RoomPreset(0.35, 1.0, 36.0, 1.00015, 8.0, proposed=True),
    "acoustic_medium_room": RoomPreset(0.60, 2.0, 32.0, 1.0002, 2.0),
    "acoustic_large_room": RoomPreset(1.40, 6.0, 26.0, 0.9997, -4.0),
}


def transducer_sections(sample_rate: int):
    return [
        high_pass(sample_rate, 95.0),
        high_pass(sample_rate, 95.0),
        low_pass(sample_rate, 15_000.0),
        peaking(sample_rate, 3_200.0, 1.0, 4.0),
    ]


ChannelTransform = Callable[[torch.Tensor, int], torch.Tensor]


@dataclass
class Channel:
    name: str
    family: str
    params: dict = field(default_factory=dict)
    _apply: ChannelTransform | None = None

    def apply(self, audio: torch.Tensor, seed: int) -> torch.Tensor:
        if self._apply is None:
            raise ValueError(f"channel {self.name!r} was built without a transform")
        return self._apply(audio, seed)


class ChannelBank:
    def __init__(self, sample_rate: int, codec_config: CodecConfig, rir_config: RirConfig) -> None:
        self.sample_rate = sample_rate
        self.codec = CodecRoundTrip(codec_config.ffmpeg, codec_config.timeout_s)
        self.rir_config = rir_config
        self._rooms: dict[str, np.ndarray] = {}

    def _room_impulse(self, name: str) -> np.ndarray:
        if name in self._rooms:
            return self._rooms[name]
        preset = PRESETS[name]
        # IMPORTANT: str.__hash__ is salted per process, so hashing the name here would give a
        # different room on every run and no two eval reports would be comparable.
        rng = np.random.default_rng(zlib.crc32(name.encode("utf-8")))
        side = 3.0 + preset.source_distance_m * 2.0
        room = ImageSourceRoom(
            room_dims_m=(side, side * 0.82, 3.0),
            source_m=(side / 2.0, side * 0.41 - preset.source_distance_m / 2.0, 1.5),
            receiver_m=(side / 2.0, side * 0.41 + preset.source_distance_m / 2.0, 1.5),
            rt60_seconds=preset.rt60_seconds,
            max_order=self.rir_config.max_order,
            fractional_delay_taps=self.rir_config.fractional_delay_taps,
            hf_rt60_ratio=self.rir_config.hf_rt60_ratio,
        )
        pair = synthesise_rir(room, self.sample_rate, self.rir_config.max_ir_seconds, rng)
        impulse = pair.at_drr(preset.direct_to_reverberant_db)
        self._rooms[name] = impulse
        return impulse

    def _codec_channel(self, name: str, spec: str) -> Channel:
        codec, _, bitrate = spec.partition(":")

        def apply(audio: torch.Tensor, seed: int) -> torch.Tensor:
            try:
                return self.codec(audio, spec, self.sample_rate)
            except CodecError as error:
                raise RuntimeError(f"{name}: {error}") from error

        return Channel(name=name, family="codec",
                       params={"codec": codec, "bitrate_kbps": int(bitrate)}, _apply=apply)

    def _acoustic_channel(self, name: str) -> Channel:
        preset = PRESETS[name]

        def apply(audio: torch.Tensor, seed: int) -> torch.Tensor:
            generator = torch.Generator(device="cpu").manual_seed(seed)
            source_rms = audio.pow(2).mean().sqrt().clamp_min(1e-9)
            impulse = torch.from_numpy(self._room_impulse(name)).to(audio.device, audio.dtype)
            wet = fft_convolve(audio, impulse.unsqueeze(0))
            wet = fft_filter(wet, transducer_sections(self.sample_rate), self.sample_rate)
            noise = torch.randn(wet.shape, generator=generator).to(audio.device, audio.dtype)
            wet_rms = wet.pow(2).mean().sqrt().clamp_min(1e-9)
            wet = wet + noise * wet_rms * (10.0 ** (-preset.room_noise_snr_db / 20.0))
            wet = resample_linear(wet, preset.clock_drift)
            return wet / wet.pow(2).mean().sqrt().clamp_min(1e-9) * source_rms

        return Channel(
            name=name,
            family="simulated_acoustic",
            params={
                "rt60_seconds": preset.rt60_seconds,
                "source_distance_m": preset.source_distance_m,
                "room_noise_snr_db": preset.room_noise_snr_db,
                "clock_drift": preset.clock_drift,
                "direct_to_reverberant_db": preset.direct_to_reverberant_db,
                "transducer_curve": "hp95x2_lp15k_peak3k2_plus4db",
                "level_matched_to_input_rms": True,
                "simulated": True,
                "proposed_preset": preset.proposed,
                "disclaimer": SIMULATION_DISCLAIMER,
            },
            _apply=apply,
        )

    def build(self, name: str) -> Channel:
        if name == "identity":
            return Channel(name, "native", {}, lambda audio, seed: audio)
        if name in PRESETS:
            return self._acoustic_channel(name)
        if name.startswith(("mp3_", "aac_", "opus_")):
            codec, _, bitrate = name.partition("_")
            return self._codec_channel(name, f"{codec}:{int(bitrate)}")
        if name.startswith("resample_via_"):
            rate = int(name.rsplit("_", 1)[1])

            def resample(audio: torch.Tensor, seed: int) -> torch.Tensor:
                down = resample_linear(audio, rate / self.sample_rate, keep_length=False)
                return resample_linear(down, self.sample_rate / rate, keep_length=False)[..., : audio.shape[-1]]

            return Channel(name, "native", {"via_sample_rate": rate}, resample)
        if name.startswith("gain_"):
            db = float(name.replace("gain_", "").replace("minus_", "-").replace("plus_", "").replace("db", ""))
            factor = 10.0 ** (db / 20.0)
            return Channel(name, "native", {"gain_db": db}, lambda audio, seed: audio * factor)
        if name.startswith("noise_snr_"):
            snr = float(name.replace("noise_snr_", "").replace("db", ""))

            def add_noise(audio: torch.Tensor, seed: int) -> torch.Tensor:
                generator = torch.Generator(device="cpu").manual_seed(seed)
                noise = torch.randn(audio.shape, generator=generator).to(audio.device, audio.dtype)
                rms = audio.pow(2).mean().sqrt().clamp_min(1e-9)
                return audio + noise * rms * (10.0 ** (-snr / 20.0))

            return Channel(name, "native", {"snr_db": snr}, add_noise)
        if name.startswith("lowpass_"):
            hz = float(name.replace("lowpass_", "").replace("k", "")) * 1000.0
            return Channel(
                name, "native", {"cutoff_hz": hz},
                lambda audio, seed: fft_filter(audio, [low_pass(self.sample_rate, hz)] * 2, self.sample_rate),
            )
        if name.startswith("drift_"):
            body = name.removeprefix("drift_").removesuffix("pct")
            sign = -1.0 if body.startswith("minus_") else 1.0
            percent = sign * float(body.removeprefix("minus_").removeprefix("plus_").replace("p", "."))
            ratio = 1.0 + percent / 100.0
            return Channel(name, "native", {"rate_ratio": ratio},
                           lambda audio, seed: resample_linear(audio, ratio))
        raise KeyError(f"unknown channel {name!r}")

    def build_all(self, names) -> list[Channel]:
        return [self.build(name) for name in names]
