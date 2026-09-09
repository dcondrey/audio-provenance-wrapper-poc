"""Torch port of crates/audio-provenance-bench/src/perceptual.rs.

Spec 9.4 requires the training objective and the bench measurement to be the SAME function, so a
training win cannot be a measurement artifact. Every constant here is copied from the Rust, not
re-derived: half-Bark partition, Schroeder spreading, alpha = 0.5 tonality, absolute threshold
anchored at 96 dB SPL for digital full scale, and the `sum(w^2) * FRAME` normalising divisor that
puts band energies in per-sample power units.

The measurement's STFT is NOT the model's STFT: FRAME 2048 / HOP 1024 / plain Hann here, against
n_fft 2048 / hop 512 / sqrt-Hann in stft.py. Conflating them is a silent error.
"""

import math

import torch
from torch import nn

from .config import PerceptualConfig

ENERGY_FLOOR = 1e-20


def bark(frequency_hz: torch.Tensor) -> torch.Tensor:
    return 13.0 * torch.atan(0.00076 * frequency_hz) + 3.5 * torch.atan((frequency_hz / 7500.0) ** 2)


def absolute_threshold_db(frequency_hz: torch.Tensor) -> torch.Tensor:
    khz = (frequency_hz / 1000.0).clamp_min(0.02)
    return (
        3.64 * khz ** (-0.8)
        - 6.5 * torch.exp(-0.6 * (khz - 3.3) ** 2)
        + 1e-3 * khz**4
    )


def spreading_db(delta_bark: torch.Tensor) -> torch.Tensor:
    x = delta_bark + 0.474
    return 15.81 + 7.5 * x - 17.5 * torch.sqrt(1.0 + x * x)


class PerceptualModel(nn.Module):
    """Noise-to-mask ratio, differentiable, matching the bench's numbers."""

    # REQUIRED: annotation-only declarations for the registered buffers. Without them every read
    # resolves through nn.Module.__getattr__ as `Tensor | Module`. Assigning a default here would
    # shadow the buffer registration, so these MUST stay bare annotations.
    window: torch.Tensor
    accumulate: torch.Tensor
    spread: torch.Tensor
    offset: torch.Tensor
    threshold_in_quiet: torch.Tensor

    def __init__(self, config: PerceptualConfig, sample_rate: int) -> None:
        super().__init__()
        self.config = config
        self.sample_rate = sample_rate
        frame = config.frame
        bins = frame // 2 + 1
        nyquist = sample_rate / 2.0

        index = torch.arange(bins, dtype=torch.float64)
        frequency = nyquist * index / max(bins - 1, 1)
        band_of_bin = torch.floor(bark(frequency) / config.bark_partition_width).clamp_min(0.0).long()
        band_count = int(band_of_bin.max().item()) + 1
        centres = torch.arange(band_count, dtype=torch.float64) * config.bark_partition_width + 0.25

        accumulate = torch.zeros(band_count, bins, dtype=torch.float64)
        accumulate[band_of_bin, index.long()] = 1.0

        spl = absolute_threshold_db(frequency).clamp_max(config.threshold_in_quiet_max_spl_db)
        power = torch.pow(10.0, (spl - config.full_scale_spl_db) / 10.0)
        threshold_in_quiet = torch.full((band_count,), float("inf"), dtype=torch.float64)
        threshold_in_quiet = threshold_in_quiet.scatter_reduce(
            0, band_of_bin, power, reduce="amin", include_self=True
        )
        threshold_in_quiet = torch.where(
            torch.isfinite(threshold_in_quiet), threshold_in_quiet, torch.full_like(threshold_in_quiet, 1e-12)
        )

        delta = centres.unsqueeze(1) - centres.unsqueeze(0)
        spread = torch.pow(10.0, spreading_db(delta) / 10.0)
        offset_db = config.tonality_alpha * (14.5 + centres) + (1.0 - config.tonality_alpha) * 5.5
        offset = torch.pow(10.0, offset_db / 10.0)

        window = 0.5 - 0.5 * torch.cos(
            2.0 * math.pi * torch.arange(frame, dtype=torch.float64) / frame
        )
        window_power = float((window * window).sum().item()) * frame

        self.band_count = band_count
        self.window_power = window_power
        self.register_buffer("window", window.float())
        self.register_buffer("accumulate", accumulate.float())
        self.register_buffer("spread", spread.float())
        self.register_buffer("offset", offset.float())
        self.register_buffer("threshold_in_quiet", threshold_in_quiet.float())

    def _frames(self, audio: torch.Tensor) -> torch.Tensor:
        frame, hop = self.config.frame, self.config.hop
        if audio.shape[-1] < frame:
            raise ValueError(f"need at least {frame} samples, got {audio.shape[-1]}")
        return audio.unfold(-1, frame, hop)

    def _band_power(self, frames: torch.Tensor) -> torch.Tensor:
        windowed = frames * self.window
        spectrum = torch.fft.rfft(windowed, n=self.config.frame, dim=-1)
        power = spectrum.real**2 + spectrum.imag**2
        return torch.einsum("bk,...k->...b", self.accumulate, power) / self.window_power

    def noise_to_mask(self, cover: torch.Tensor, marked: torch.Tensor) -> dict[str, torch.Tensor]:
        """Returns per-frame per-band NMR in dB, the bench's per-frame scalar, and the silence mask.

        `cover` and `marked` are (B, samples).
        """
        residual = marked - cover
        cover_frames = self._frames(cover)
        residual_frames = self._frames(residual)

        frame_energy = (cover_frames**2).mean(dim=-1)
        active = frame_energy >= 10.0 ** (self.config.silence_floor_dbfs / 10.0)

        cover_bands = self._band_power(cover_frames)
        error_bands = self._band_power(residual_frames)

        masked = torch.einsum("bj,...j->...b", self.spread, cover_bands) / self.offset
        threshold = torch.maximum(masked, self.threshold_in_quiet).clamp_min(ENERGY_FLOOR)

        ratio = error_bands / threshold
        per_band_db = 10.0 * torch.log10(ratio.clamp_min(ENERGY_FLOOR))
        per_frame_db = 10.0 * torch.log10(ratio.mean(dim=-1).clamp_min(ENERGY_FLOOR))
        return {
            "per_band_db": per_band_db,
            "per_frame_db": per_frame_db,
            "active": active,
            "ratio": ratio,
        }

    def loss(self, cover: torch.Tensor, marked: torch.Tensor) -> torch.Tensor:
        """L_perc of spec 9.4: ReLU of the per-band NMR in dB, summed over the band, averaged over
        active frames. Silent frames contribute nothing, exactly as the bench skips them."""
        parts = self.noise_to_mask(cover, marked)
        penalty = torch.relu(parts["per_band_db"]).sum(dim=-1)
        active = parts["active"].float()
        denominator = active.sum().clamp_min(1.0)
        return (penalty * active).sum() / denominator

    @torch.no_grad()
    def measure(self, cover: torch.Tensor, marked: torch.Tensor) -> dict[str, float]:
        """The bench's PerceptualMeasurement, for a single (B=1) pair. Reported for the worst
        channel by the caller, never mixed down."""
        parts = self.noise_to_mask(cover, marked)
        active = parts["active"]
        nmr = parts["per_frame_db"][active]
        residual = marked - cover
        seg = self._segmental_snr(cover, residual)
        result = {
            "segmental_snr_db": seg[0],
            "segments_scored": seg[1],
            "segments_skipped_silent": seg[2],
            "peak_residual_dbfs": float(20.0 * torch.log10(residual.abs().max().clamp_min(1e-20)).item()),
            "frames_scored": int(active.sum().item()),
        }
        if nmr.numel() > 0:
            result["noise_to_mask_mean_db"] = float(nmr.mean().item())
            result["noise_to_mask_max_db"] = float(nmr.max().item())
            result["frames_above_mask_fraction"] = float((nmr > 0.0).float().mean().item())
        else:
            result["noise_to_mask_mean_db"] = None
            result["noise_to_mask_max_db"] = None
            result["frames_above_mask_fraction"] = None
        return result

    def _segmental_snr(self, cover: torch.Tensor, residual: torch.Tensor) -> tuple[float | None, int, int]:
        seg = 1024
        if cover.shape[-1] < seg:
            return None, 0, 0
        signal = (cover.unfold(-1, seg, seg) ** 2).mean(dim=-1)
        noise = (residual.unfold(-1, seg, seg) ** 2).mean(dim=-1)
        active = signal >= 10.0 ** (self.config.silence_floor_dbfs / 10.0)
        ratio = 10.0 * torch.log10(signal / noise.clamp_min(ENERGY_FLOOR))
        ratio = ratio.clamp(-20.0, 100.0)
        scored = int(active.sum().item())
        skipped = int((~active).sum().item())
        if scored == 0:
            return None, 0, skipped
        return float(ratio[active].mean().item()), scored, skipped


def masking_budget(
    model: PerceptualModel,
    cover_log_magnitude: torch.Tensor,
    kappa: float,
    b_max: float,
    band_first_bin: int,
    band_last_bin: int,
    n_fft: int,
    sample_rate: int,
) -> torch.Tensor:
    """Spec 2.7's per-bin, per-frame ceiling B[t,f] in nepers, on the MODEL's STFT grid.

    B = clamp(kappa * 10^((M - L)/20), 0, B_max) where L is the cover's per-bin level in dB and M
    is the masking threshold from the same Schroeder / half-Bark model. The band mapping is
    recomputed on the model grid because the model's bins are 23.4 Hz and the measurement's are
    23.4 Hz only by coincidence of n_fft; the sample rates and hops differ.
    """
    device = cover_log_magnitude.device
    bins = band_last_bin - band_first_bin + 1
    index = torch.arange(band_first_bin, band_last_bin + 1, dtype=torch.float64, device=device)
    frequency = index * sample_rate / n_fft
    band_of_bin = torch.floor(bark(frequency) / model.config.bark_partition_width).clamp_min(0.0).long()
    band_count = int(band_of_bin.max().item()) + 1
    centres = (
        torch.arange(band_count, dtype=torch.float64, device=device) * model.config.bark_partition_width
        + 0.25
    )
    accumulate = torch.zeros(band_count, bins, dtype=torch.float32, device=device)
    accumulate[band_of_bin, torch.arange(bins, device=device)] = 1.0
    delta = centres.unsqueeze(1) - centres.unsqueeze(0)
    spread = torch.pow(10.0, spreading_db(delta) / 10.0).float()
    offset_db = model.config.tonality_alpha * (14.5 + centres) + (1.0 - model.config.tonality_alpha) * 5.5
    offset = torch.pow(10.0, offset_db / 10.0).float()

    magnitude = cover_log_magnitude.squeeze(1).exp()
    power = magnitude**2
    band_power = torch.einsum("bk,nkt->nbt", accumulate, power)
    masked = torch.einsum("bj,njt->nbt", spread, band_power) / offset.view(1, -1, 1)
    threshold_per_bin = torch.einsum("bk,nbt->nkt", accumulate, masked)

    level_db = 20.0 * torch.log10(magnitude.clamp_min(1e-9))
    mask_db = 10.0 * torch.log10(threshold_per_bin.clamp_min(ENERGY_FLOOR))
    budget = kappa * torch.pow(10.0, (mask_db - level_db) / 20.0)
    return budget.clamp(0.0, b_max).unsqueeze(1)
