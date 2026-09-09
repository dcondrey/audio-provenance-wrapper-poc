"""The STFT that lives OUTSIDE the exported graph.

Spec 7.2: the ONNX graph is pure convolution over log-magnitudes. Everything in this module is the
Rust side's responsibility at inference time and is reproduced here so training optimises the same
representation the deployed detector will see.

IMPORTANT: the framing is `crates/apw-watermark-neural`'s, clause for clause, not `torch.stft(center=True)`.
The lead pad is the WINDOW LENGTH (2048), not half of it, and the frame count is
ceil((2048 + samples) / 512). `center=True` pads 1024 and would put every frame index four hops out
of step with the Rust loader while the model card claimed otherwise. The model is shift-equivariant
so the choice is free to training and not free to the contract.
"""

import torch
from torch.nn import functional

from .config import StftConfig


def sqrt_hann(n_fft: int, device=None, dtype=torch.float32) -> torch.Tensor:
    """Square-root Hann. WOLA reconstruction is exact for hop = n_fft/4 with this window."""
    window = torch.hann_window(n_fft, periodic=True, device=device, dtype=torch.float64)
    return window.clamp_min(0.0).sqrt().to(dtype)


class SpectralFront:
    """Forward/inverse STFT plus the band slice, shared by the encoder, the losses and the decoder."""

    LOG_FLOOR = 1e-7

    def __init__(self, config: StftConfig, device=None, dtype=torch.float32) -> None:
        config.validate()
        self.config = config
        self.window = sqrt_hann(config.n_fft, device=device, dtype=dtype)

    def to(self, device) -> "SpectralFront":
        self.window = self.window.to(device)
        return self

    @property
    def band(self) -> slice:
        return slice(self.config.band_first_bin, self.config.band_last_bin + 1)

    @property
    def lead_pad(self) -> int:
        return self.config.n_fft

    def frames_for(self, samples: int) -> int:
        return -(-(self.lead_pad + samples) // self.config.hop)

    def padded_length(self, samples: int) -> int:
        return (self.frames_for(samples) - 1) * self.config.hop + self.config.n_fft

    def stft(self, audio: torch.Tensor) -> torch.Tensor:
        """(B, samples) -> complex (B, bins, T)."""
        samples = audio.shape[-1]
        tail = self.padded_length(samples) - self.lead_pad - samples
        padded = functional.pad(audio, (self.lead_pad, tail))
        return torch.stft(
            padded,
            n_fft=self.config.n_fft,
            hop_length=self.config.hop,
            win_length=self.config.n_fft,
            window=self.window.to(audio.device),
            center=False,
            normalized=False,
            onesided=True,
            return_complex=True,
        )

    def istft(self, spectrum: torch.Tensor, length: int) -> torch.Tensor:
        """WOLA resynthesis, then the lead pad is dropped so the result is in host sample time.

        Hand-rolled rather than `torch.istft`, which refuses this framing: a periodic Hann is
        exactly zero at n = 0, so the overlap-added squared window is zero at the first padded
        sample and torch's NOLA check rejects the whole call. The Rust WOLA path divides only where
        the denominator exceeds an epsilon and leaves the rest at zero; this reproduces that, and
        those samples are lead pad that the trim discards anyway.
        """
        n_fft, hop = self.config.n_fft, self.config.hop
        window = self.window.to(spectrum.device, spectrum.real.dtype)
        frames = spectrum.shape[-1]
        padded_length = (frames - 1) * hop + n_fft
        blocks = torch.fft.irfft(spectrum, n=n_fft, dim=1) * window.view(1, -1, 1)
        accumulator = functional.fold(
            blocks, output_size=(1, padded_length), kernel_size=(1, n_fft), stride=(1, hop)
        ).reshape(spectrum.shape[0], padded_length)
        squared = (window * window).view(1, -1, 1).expand(1, n_fft, frames)
        denominator = functional.fold(
            squared, output_size=(1, padded_length), kernel_size=(1, n_fft), stride=(1, hop)
        ).reshape(1, padded_length)
        padded = torch.where(denominator > 1e-8, accumulator / denominator.clamp_min(1e-8),
                             torch.zeros_like(accumulator))
        out = padded[..., self.lead_pad : self.lead_pad + length]
        if out.shape[-1] < length:
            out = functional.pad(out, (0, length - out.shape[-1]))
        return out

    def band_log_magnitude(self, spectrum: torch.Tensor) -> torch.Tensor:
        """complex (B, bins, T) -> (B, 1, 320, T) log-magnitude, the graph's input tensor."""
        magnitude = spectrum[:, self.band, :].abs()
        return magnitude.clamp_min(self.LOG_FLOOR).log().unsqueeze(1)

    def apply_band_gain(self, spectrum: torch.Tensor, log_gain: torch.Tensor) -> torch.Tensor:
        """Multiply the in-band complex bins by exp(log_gain). Phase is untouched; out-of-band bins
        are returned bit-identical, which is what spec 2.4 means by 'everything outside the band is
        untouched, exactly'."""
        gain = log_gain.squeeze(1).exp().to(spectrum.dtype)
        marked = spectrum.clone()
        marked[:, self.band, :] = spectrum[:, self.band, :] * gain
        return marked

    def frame_centre_samples(self, frames: int, device=None) -> torch.Tensor:
        """Host-sample position of each frame's centre.

        Frame t reads padded[t*hop : t*hop + n_fft], which is host samples
        [t*hop - lead_pad, t*hop), so its centre sits at t*hop - n_fft/2. Derived from the hop and
        the pad, never a hardcoded frame count, and negative for the first four frames because
        those windows lie mostly in the lead pad.
        """
        index = torch.arange(frames, device=device, dtype=torch.float32)
        return index * self.config.hop - self.config.n_fft / 2.0
