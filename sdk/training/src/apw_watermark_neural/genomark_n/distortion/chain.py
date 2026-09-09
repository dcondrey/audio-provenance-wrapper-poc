"""Spec 6, applied in physical order.

Split of responsibility, stated because getting it wrong models a chain that does not exist:
  D1 (source codec history), D2 (random sample offset and marked fraction) and D12 (time crop)
  belong to the DATASET: they are properties of the source clip and of how the window is cut, and
  D1 in particular precedes embedding so it may be cached. Everything from D0 to D11 is here.
"""

import math
from dataclasses import dataclass, field

import numpy as np

import torch

from ..config import CodecConfig, DistortionConfig
from ..qmark import QGrid
from ..rir.corpus import RirCorpus
from .codec import CodecError, CodecRoundTrip
from .noise import room_noise
from .dsp import (
    fft_convolve,
    fft_filter,
    high_pass,
    low_pass,
    one_pole_envelope,
    peaking,
    resample_linear,
    soft_clip,
    thd_to_coefficients,
)

PHASES = ("A", "B", "C")


@dataclass
class DistortionOutcome:
    audio: torch.Tensor
    stages: list[str] = field(default_factory=list)
    metadata: dict = field(default_factory=dict)


def _ramp(value_a: tuple[float, float], value_c: tuple[float, float], t: float) -> tuple[float, float]:
    return (
        value_a[0] + (value_c[0] - value_a[0]) * t,
        value_a[1] + (value_c[1] - value_a[1]) * t,
    )


class DistortionChain:
    """Stateless per call except for the codec's call counter; all randomness comes from `rng`."""

    PHASE_A_RANGES = {"drr_db": (12.0, 25.0), "noise_snr_db": (35.0, 45.0), "agc_p": 0.0}

    def __init__(
        self,
        config: DistortionConfig,
        codec_config: CodecConfig,
        sample_rate: int,
        rir_corpus: RirCorpus | None = None,
        q_grid: QGrid | None = None,
        noise_provider=None,
    ) -> None:
        self.config = config
        self.sample_rate = sample_rate
        self.rir = rir_corpus
        self.q_grid = q_grid
        self.codec = CodecRoundTrip(codec_config.ffmpeg, codec_config.timeout_s)
        self.codec_config = codec_config
        self.noise_provider = noise_provider
        self.codec_failures = 0

    def ranges(self, phase: str, progress: float) -> dict:
        """Spec 9.3's curriculum. Phase A has no room; Phase B ramps to the Phase C ranges."""
        if phase == "A":
            return {"drr_db": None, "noise_snr_db": None, "agc_p": 0.0, "physical": False}
        if phase == "B":
            t = min(max(progress, 0.0), 1.0)
            return {
                "drr_db": _ramp(self.PHASE_A_RANGES["drr_db"], (2.0, 18.0), t),
                "noise_snr_db": _ramp(self.PHASE_A_RANGES["noise_snr_db"], (20.0, 40.0), t),
                "agc_p": self.config.agc_p * t,
                "physical": True,
            }
        return {
            "drr_db": self.config.drr_db,
            "noise_snr_db": self.config.noise_snr_db,
            "agc_p": self.config.agc_p,
            "physical": True,
        }

    def __call__(
        self,
        audio: torch.Tensor,
        rng: torch.Generator,
        phase: str = "C",
        progress: float = 1.0,
        force_capture_codec: bool = False,
    ) -> DistortionOutcome:
        if audio.dim() != 2:
            raise ValueError(f"expected (batch, samples), got {tuple(audio.shape)}")
        original_samples = audio.shape[-1]
        ranges = self.ranges(phase, progress)
        stages: list[str] = []
        metadata: dict = {"phase": phase, "progress": progress}

        def draw(low: float, high: float) -> float:
            return float(torch.rand((), generator=rng, device=rng.device).item()) * (high - low) + low

        def chance(probability: float) -> bool:
            return float(torch.rand((), generator=rng, device=rng.device).item()) < probability

        electronic_only = ranges["physical"] and chance(self.config.electronic_only_p)
        metadata["branch"] = "electronic_only" if electronic_only or not ranges["physical"] else "acoustic"

        if self.q_grid is not None and chance(self.config.q_embed_p):
            audio = self.q_grid.embed_perturbation(audio, rng)
            stages.append("D0_apw_watermark_q_embed")

        target_dbfs = draw(*self.config.playback_gain_dbfs)
        peak = audio.abs().amax(dim=-1, keepdim=True).clamp_min(1e-9)
        audio = audio / peak * (10.0 ** (target_dbfs / 20.0))
        stages.append("D3_playback_gain")
        metadata["playback_dbfs"] = target_dbfs

        acoustic = ranges["physical"] and not electronic_only
        if acoustic:
            if chance(self.config.speaker_nonlinearity_p):
                thd = draw(*self.config.thd_range)
                third, fifth = thd_to_coefficients(thd)
                audio = soft_clip(audio, third, fifth)
                envelope = one_pole_envelope(audio, self.config.thermal_time_constant_s, self.sample_rate)
                reduction = 10.0 ** (
                    -self.config.thermal_compression_db * envelope.clamp(0.0, 1.0) / 20.0
                )
                audio = audio * reduction
                stages.append("D4_speaker_nonlinearity")
                metadata["thd"] = thd

            sections = [
                high_pass(self.sample_rate, draw(*self.config.highpass_hz)),
                low_pass(self.sample_rate, draw(*self.config.lowpass_hz)),
            ]
            count = int(draw(self.config.peaking_sections[0], self.config.peaking_sections[1] + 1))
            for _ in range(count):
                low, high = self.config.peaking_centre_hz
                centre = math.exp(draw(math.log(low), math.log(high)))
                sections.append(
                    peaking(
                        self.sample_rate,
                        centre,
                        draw(*self.config.peaking_q),
                        draw(*self.config.peaking_gain_db),
                    )
                )
            audio = fft_filter(audio, sections, self.sample_rate)
            stages.append("D5_transducer")
            metadata["peaking_sections"] = count

            if self.rir is not None and chance(self.config.rir_p):
                drr = draw(*ranges["drr_db"])
                seed = int(torch.randint(0, 2**31 - 1, (), generator=rng, device=rng.device).item())
                drawn = self.rir.draw(np.random.default_rng(seed), drr)
                impulse = torch.from_numpy(drawn.impulse).to(audio.device, audio.dtype).unsqueeze(0)
                audio = fft_convolve(audio, impulse)
                stages.append("D6_room_impulse_response")
                metadata["drr_db"] = drr
                metadata["rir"] = drawn.identifier
                metadata["rir_source"] = str(drawn.source)

            snr = draw(*ranges["noise_snr_db"])
            tilt = draw(*self.config.room_noise_tilt_db_per_octave)
            signal_rms = audio.pow(2).mean(dim=-1, keepdim=True).sqrt().clamp_min(1e-9)
            noise = room_noise(audio.shape, rng, self.sample_rate, tilt).to(audio.device, audio.dtype)
            if self.noise_provider is not None:
                noise = noise + self.noise_provider(audio.shape, rng).to(audio.device, audio.dtype)
            noise_rms = noise.pow(2).mean(dim=-1, keepdim=True).sqrt().clamp_min(1e-9)
            audio = audio + noise / noise_rms * signal_rms * (10.0 ** (-snr / 20.0))
            stages.append("D7_background_noise")
            metadata["noise_snr_db"] = snr
            metadata["noise_tilt_db_per_octave"] = tilt

            if chance(ranges["agc_p"]):
                audio = self._agc(
                    audio,
                    ratio=draw(*self.config.agc_ratio),
                    attack_ms=draw(*self.config.agc_attack_ms),
                    release_ms=draw(*self.config.agc_release_ms),
                )
                stages.append("D8_capture_agc")

        ppm = draw(*self.config.drift_ppm)
        audio = resample_linear(audio, 1.0 + ppm * 1e-6)
        stages.append("D9_clock_drift")
        metadata["drift_ppm"] = ppm
        if chance(self.config.resample_round_trip_p):
            audio = resample_linear(audio, 44_100 / self.sample_rate, keep_length=False)
            audio = resample_linear(audio, self.sample_rate / 44_100, keep_length=False)
            stages.append("D9_resample_round_trip")

        bits = 24 if chance(self.config.quantise_24bit_p) else self.config.quantise_bits
        audio = self._quantise(audio, bits, rng)
        stages.append(f"D10_quantise_{bits}bit")

        if force_capture_codec or chance(self.config.capture_codec_p):
            index = int(torch.randint(
                0, len(self.codec_config.capture_codecs), (), generator=rng, device=rng.device
            ).item())
            spec = self.codec_config.capture_codecs[index]
            try:
                audio = self.codec(audio, spec, self.sample_rate)
                stages.append(f"D11_capture_codec_{spec.replace(':', '_')}")
                metadata["capture_codec"] = spec
            except CodecError as error:
                self.codec_failures += 1
                metadata["capture_codec_error"] = str(error)[:200]

        samples = audio.shape[-1]
        if samples != original_samples:
            audio = (
                audio[..., :original_samples]
                if samples > original_samples
                else torch.nn.functional.pad(audio, (0, original_samples - samples))
            )
        return DistortionOutcome(audio=audio, stages=stages, metadata=metadata)

    def _quantise(self, audio: torch.Tensor, bits: int, rng: torch.Generator) -> torch.Tensor:
        step = 2.0 ** (1 - bits)
        tpdf = (
            torch.rand(audio.shape, generator=rng, device=rng.device)
            - torch.rand(audio.shape, generator=rng, device=rng.device)
        ).to(audio.device, audio.dtype) * step
        dithered = audio + tpdf
        quantised = torch.round(dithered / step) * step
        return dithered + (quantised - dithered).detach()

    def _agc(self, audio: torch.Tensor, ratio: float, attack_ms: float, release_ms: float,
             control_rate: int = 500) -> torch.Tensor:
        """Single-band compressor with a slow makeup gain.

        The attack/release recursion is asymmetric so it cannot be a convolution. It runs on a
        500 Hz control signal under no_grad and the resulting gain is applied multiplicatively with
        gradient flowing through the signal, the same treatment spec 6 D0 prescribes for Q's QIM.
        """
        with torch.no_grad():
            block = max(self.sample_rate // control_rate, 1)
            usable = (audio.shape[-1] // block) * block
            if usable < block:
                return audio
            frames = audio[..., :usable].reshape(audio.shape[0], -1, block)
            level_db = 10.0 * torch.log10(frames.pow(2).mean(dim=-1).clamp_min(1e-12))
            threshold = torch.quantile(level_db, 0.70, dim=-1, keepdim=True)
            over = (level_db - threshold).clamp_min(0.0)
            target = -over * (1.0 - 1.0 / ratio)
            attack = math.exp(-1.0 / max(attack_ms * 1e-3 * control_rate, 1.0))
            release = math.exp(-1.0 / max(release_ms * 1e-3 * control_rate, 1.0))
            gain = torch.zeros_like(target)
            previous = torch.zeros_like(target[:, 0])
            for index in range(target.shape[1]):
                desired = target[:, index]
                coefficient = torch.where(desired < previous, attack, release)
                previous = coefficient * previous + (1.0 - coefficient) * desired
                gain[:, index] = previous
            makeup = -gain.mean(dim=-1, keepdim=True)
            linear = 10.0 ** ((gain + makeup) / 20.0)
            envelope = torch.nn.functional.interpolate(
                linear.unsqueeze(1), size=audio.shape[-1], mode="linear", align_corners=False
            ).squeeze(1)
        return audio * envelope
