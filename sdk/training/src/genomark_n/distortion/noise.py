"""Spec 6 D7's noise floor.

D7 reads "a MUSAN noise excerpt plus a Gaussian floor". MUSAN is CC BY 4.0 and is the intended
source, but it is a download, and a run with no corpus configured must not silently degrade to
white noise: an unoccupied room's background is dominated by HVAC and structural rumble and falls
roughly 3 to 6 dB per octave above a few hundred Hz, so white noise puts far too much power at the
top of the 211-7688 Hz band and far too little at the bottom. Since the band's low end is exactly
where the mark is hardest to place, training against a white floor trains against the wrong
channel in the direction that flatters the result.

This synthesises the tilt instead. It is a SUBSTITUTE, not a replacement: when a licence-cleared
noise manifest is configured, `DistortionChain`'s `noise_provider` supplies real excerpts and this
remains the Gaussian floor beneath them.
"""

import torch

REFERENCE_HZ = 250.0
FLAT_BELOW_HZ = 50.0


def spectral_tilt(noise: torch.Tensor, sample_rate: int, db_per_octave: float) -> torch.Tensor:
    """Apply a per-octave tilt about 250 Hz, flat below 50 Hz so the shaping cannot diverge at DC."""
    samples = noise.shape[-1]
    size = 1 << (2 * samples - 1).bit_length()
    frequency = torch.fft.rfftfreq(size, 1.0 / sample_rate, device=noise.device)
    octaves = torch.log2((frequency / REFERENCE_HZ).clamp_min(FLAT_BELOW_HZ / REFERENCE_HZ))
    response = torch.pow(10.0, db_per_octave * octaves / 20.0)
    spectrum = torch.fft.rfft(noise, n=size, dim=-1) * response
    return torch.fft.irfft(spectrum, n=size, dim=-1)[..., :samples]


def room_noise(shape: tuple[int, ...], rng: torch.Generator, sample_rate: int,
               db_per_octave: float, gaussian_floor_db: float = -20.0) -> torch.Tensor:
    """A tilted room floor plus an independent Gaussian floor beneath it, unit RMS."""
    tilted = spectral_tilt(
        torch.randn(shape, generator=rng, device=rng.device), sample_rate, db_per_octave
    )
    tilted = tilted / tilted.pow(2).mean(dim=-1, keepdim=True).sqrt().clamp_min(1e-9)
    gaussian = torch.randn(shape, generator=rng, device=rng.device)
    mixed = tilted + gaussian * (10.0 ** (gaussian_floor_db / 20.0))
    return mixed / mixed.pow(2).mean(dim=-1, keepdim=True).sqrt().clamp_min(1e-9)
