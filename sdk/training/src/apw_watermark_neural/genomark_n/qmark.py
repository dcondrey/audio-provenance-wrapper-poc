"""Watermark-Q's statistic and its perturbation, in torch.

Two jobs, both from spec 8:
  D0 (spec 6): apply Q's antisymmetric +/- 0.43 dB per-bin gain so N must survive the shipped mark.
  L_q (spec 9.4): the hinge on std(D_marked - D_cover) over Q's exact statistic.

The geometry is Q's, copied from crates/apw-watermark/src/params.rs and geometry.rs: cell edges at
reference bins 20 + 2i on a 1024-point frame at 44.1 kHz, giving 40 cells / 20 pairs over
861.3-4306.6 Hz, six guard frequencies removing pairs, 14 active pairs, 2 frames per slot, and a
trimmed mean dropping 2 values from each tail of the 28.

IMPORTANT: this is a MODEL of Q's perturbation, not a port of Q's embedder. Q's QIM lands a lattice
point whose offset depends on the host; what N has to survive is the resulting per-bin gain, whose
magnitude Q bounds. Spec 8.3 is explicit that exact cancellation is unavailable and that the only
real evidence is the measured bound, which is what L_q drives and bench row N-B3 measures.
"""

import math

import torch

from .config import QCoexistenceConfig

ENERGY_FLOOR = 1e-20


class QGrid:
    def __init__(self, config: QCoexistenceConfig, sample_rate: int, device=None) -> None:
        self.config = config
        self.sample_rate = sample_rate
        bin_hz = config.reference_sample_rate / config.frame
        edges_hz = [(config.band_first_bin + 2 * i) * bin_hz for i in range(config.cells + 1)]
        self.edges_hz = edges_hz
        pairs = config.cells // 2
        self.active_pairs = [
            pair
            for pair in range(pairs)
            if not any(edges_hz[2 * pair] <= hz < edges_hz[2 * pair + 2] for hz in config.guard_hz)
        ]
        if len(self.active_pairs) != 14:
            raise ValueError(f"guard mask gave {len(self.active_pairs)} active pairs, Q fixes 14")

        native_bin_hz = sample_rate / config.frame
        bins = config.frame // 2 + 1
        cell_of_bin = torch.full((bins,), -1, dtype=torch.long)
        for cell in range(config.cells):
            low = int(math.ceil(edges_hz[cell] / native_bin_hz))
            high = int(math.floor(edges_hz[cell + 1] / native_bin_hz))
            for k in range(max(low, 0), min(high, bins - 1) + 1):
                cell_of_bin[k] = cell
        select = torch.zeros(config.cells, bins, dtype=torch.float32, device=device)
        for k in range(bins):
            if cell_of_bin[k] >= 0:
                select[cell_of_bin[k], k] = 1.0
        if select.sum(dim=1).min() < 1.0:
            raise ValueError("a Q cell contains no bin at this sample rate")
        self.select = select
        self.cell_of_bin = cell_of_bin.to(device)
        self.window = torch.hann_window(config.frame, periodic=True, device=device)

    def to(self, device) -> "QGrid":
        self.select = self.select.to(device)
        self.cell_of_bin = self.cell_of_bin.to(device)
        self.window = self.window.to(device)
        return self

    def spectrum(self, audio: torch.Tensor) -> torch.Tensor:
        return torch.stft(
            audio,
            n_fft=self.config.frame,
            hop_length=self.config.frame // 2,
            win_length=self.config.frame,
            window=self.window.to(audio.device),
            center=True,
            pad_mode="constant",
            return_complex=True,
        )

    def cell_energy(self, spectrum: torch.Tensor) -> torch.Tensor:
        power = spectrum.abs() ** 2
        return torch.einsum("ck,nkt->nct", self.select.to(power.device), power)

    def statistic(self, audio: torch.Tensor) -> torch.Tensor:
        """Q's slot statistic D[n, slot]: trimmed mean over 14 active pairs x 2 frames."""
        energy = self.cell_energy(self.spectrum(audio))
        active = torch.tensor(self.active_pairs, device=energy.device)
        energy_a = energy[:, 2 * active, :]
        energy_b = energy[:, 2 * active + 1, :]
        difference = torch.log(energy_a + ENERGY_FLOOR) - torch.log(energy_b + ENERGY_FLOOR)
        frames = difference.shape[-1]
        per_slot = frames // self.config.frames_per_slot
        if per_slot == 0:
            return difference.new_zeros((difference.shape[0], 0))
        difference = difference[..., : per_slot * self.config.frames_per_slot]
        pooled = difference.reshape(difference.shape[0], difference.shape[1], per_slot, -1)
        pooled = pooled.permute(0, 2, 1, 3).reshape(difference.shape[0], per_slot, -1)
        trim = self.config.trim_each_tail
        ordered, _ = torch.sort(pooled, dim=-1)
        return ordered[..., trim : ordered.shape[-1] - trim].mean(dim=-1)

    def residual_std(self, cover: torch.Tensor, marked: torch.Tensor) -> torch.Tensor:
        difference = self.statistic(marked) - self.statistic(cover)
        if difference.numel() < 2:
            return difference.new_zeros(())
        return difference.std()

    def coexistence_loss(self, cover: torch.Tensor, marked: torch.Tensor) -> torch.Tensor:
        """L_q of spec 9.4: a HINGE at Q's own OLA-closure bound, not a quadratic. The bound is
        what matters; driving the difference to zero would spend capacity for nothing."""
        return torch.relu(self.residual_std(cover, marked) - self.config.residual_limit_nepers)

    def embed_perturbation(self, audio: torch.Tensor, generator: torch.Generator) -> torch.Tensor:
        """D0: Q's antisymmetric per-bin gain, sign drawn per (pair, slot) as a random payload would.

        The sign pattern is drawn under no_grad; the resulting multiplicative gain is applied with
        gradient flowing through the magnitudes, exactly as spec 6 D0 requires.
        """
        spectrum = self.spectrum(audio)
        with torch.no_grad():
            batch, _, frames = spectrum.shape
            pairs = len(self.active_pairs)
            slots = max(frames // self.config.frames_per_slot, 1)
            signs = torch.randint(
                0, 2, (batch, pairs, slots), generator=generator,
                device=generator.device, dtype=torch.float32
            ).to(spectrum.device) * 2.0 - 1.0
            signs = signs.repeat_interleave(self.config.frames_per_slot, dim=-1)
            if signs.shape[-1] < frames:
                signs = torch.nn.functional.pad(signs, (0, frames - signs.shape[-1]), mode="replicate")
            signs = signs[..., :frames]
            gain_linear = 10.0 ** (self.config.q_gain_db / 20.0)
            log_gain = torch.zeros(batch, spectrum.shape[1], frames, device=spectrum.device)
            for index, pair in enumerate(self.active_pairs):
                mask_a = self.select[2 * pair].to(spectrum.device).view(1, -1, 1)
                mask_b = self.select[2 * pair + 1].to(spectrum.device).view(1, -1, 1)
                delta = signs[:, index : index + 1, :] * math.log(gain_linear)
                log_gain = log_gain + mask_a * delta - mask_b * delta
        marked = spectrum * torch.exp(log_gain).to(spectrum.dtype)
        return torch.istft(
            marked,
            n_fft=self.config.frame,
            hop_length=self.config.frame // 2,
            win_length=self.config.frame,
            window=self.window.to(audio.device),
            center=True,
            length=audio.shape[-1],
        )
