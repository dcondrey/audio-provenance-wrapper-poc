"""Differentiable DSP primitives.

Every filter here is applied by multiplying the analytic biquad frequency response onto a
zero-padded rFFT rather than by running the difference equation. Two reasons: a 48 kHz sample-rate
recursion in Python is unusably slow inside a training step, and the frequency-domain form is
exactly differentiable with no truncated-IR approximation. Padding to the next power of two above
2*N keeps the circular convolution equal to the linear one.
"""

import math

import torch


def _next_fast_len(n: int) -> int:
    return 1 << (n - 1).bit_length()


def biquad_response(
    b: tuple[float, float, float],
    a: tuple[float, float, float],
    bins: int,
    sample_rate: int,
    device,
) -> torch.Tensor:
    omega = torch.linspace(0.0, math.pi, bins, device=device, dtype=torch.float64)
    z1 = torch.exp(-1j * omega)
    z2 = z1 * z1
    numerator = b[0] + b[1] * z1 + b[2] * z2
    denominator = a[0] + a[1] * z1 + a[2] * z2
    return (numerator / denominator).to(torch.complex64)


def _rbj(kind: str, sample_rate: int, frequency: float, q: float, gain_db: float = 0.0):
    """RBJ audio-EQ cookbook coefficients, the same forms audio-provenance-audio's biquad.rs uses."""
    omega = 2.0 * math.pi * min(frequency, sample_rate * 0.49) / sample_rate
    sin_w, cos_w = math.sin(omega), math.cos(omega)
    alpha = sin_w / (2.0 * max(q, 1e-4))
    if kind == "low_pass":
        b = ((1.0 - cos_w) / 2.0, 1.0 - cos_w, (1.0 - cos_w) / 2.0)
        a = (1.0 + alpha, -2.0 * cos_w, 1.0 - alpha)
    elif kind == "high_pass":
        b = ((1.0 + cos_w) / 2.0, -(1.0 + cos_w), (1.0 + cos_w) / 2.0)
        a = (1.0 + alpha, -2.0 * cos_w, 1.0 - alpha)
    elif kind == "peaking":
        amplitude = 10.0 ** (gain_db / 40.0)
        b = (1.0 + alpha * amplitude, -2.0 * cos_w, 1.0 - alpha * amplitude)
        a = (1.0 + alpha / amplitude, -2.0 * cos_w, 1.0 - alpha / amplitude)
    else:
        raise ValueError(f"unknown biquad kind {kind!r}")
    return b, a


def low_pass(sample_rate: int, frequency: float, q: float = 0.707):
    return _rbj("low_pass", sample_rate, frequency, q)


def high_pass(sample_rate: int, frequency: float, q: float = 0.707):
    return _rbj("high_pass", sample_rate, frequency, q)


def peaking(sample_rate: int, frequency: float, q: float, gain_db: float):
    return _rbj("peaking", sample_rate, frequency, q, gain_db)


def fft_filter(audio: torch.Tensor, sections, sample_rate: int) -> torch.Tensor:
    """Apply a cascade of biquad sections. `sections` is a sequence of (b, a) coefficient triples."""
    if not sections:
        return audio
    samples = audio.shape[-1]
    size = _next_fast_len(2 * samples)
    bins = size // 2 + 1
    response = torch.ones(bins, dtype=torch.complex64, device=audio.device)
    for b, a in sections:
        response = response * biquad_response(b, a, bins, sample_rate, audio.device)
    spectrum = torch.fft.rfft(audio, n=size, dim=-1)
    return torch.fft.irfft(spectrum * response, n=size, dim=-1)[..., :samples]


def fft_convolve(audio: torch.Tensor, impulse: torch.Tensor) -> torch.Tensor:
    """Linear convolution, truncated to the input length.

    IMPORTANT: truncating to the input length and NOT compensating the impulse response's own
    pre-delay is deliberate. The Rust bench states it in METRIC_DEFINITIONS: it never realigns
    before detection, because sync is the detector's job. A training chain that quietly removed the
    propagation delay would train a detector that cannot survive one.
    """
    samples = audio.shape[-1]
    size = _next_fast_len(samples + impulse.shape[-1])
    spectrum = torch.fft.rfft(audio, n=size, dim=-1) * torch.fft.rfft(impulse, n=size, dim=-1)
    return torch.fft.irfft(spectrum, n=size, dim=-1)[..., :samples]


def resample_linear(audio: torch.Tensor, ratio: float, keep_length: bool = True) -> torch.Tensor:
    """Resample by `ratio` (output rate / input rate) with linear interpolation.

    Linear interpolation is a first-order approximation to a polyphase resampler and is used here
    because it is cheap and exactly differentiable. It imposes a mild low-pass that a real
    resampler does not; the deviation is recorded in the run notes rather than hidden.
    """
    samples = audio.shape[-1]
    target = max(int(round(samples * ratio)), 2)
    positions = torch.arange(target, device=audio.device, dtype=audio.dtype) / ratio
    positions = positions.clamp(0.0, samples - 1.0)
    lower = positions.floor().long()
    upper = (lower + 1).clamp_max(samples - 1)
    frac = (positions - lower.to(positions.dtype)).unsqueeze(0)
    resampled = audio[..., lower] * (1.0 - frac) + audio[..., upper] * frac
    if not keep_length:
        return resampled
    if resampled.shape[-1] >= samples:
        return resampled[..., :samples]
    pad = samples - resampled.shape[-1]
    return torch.nn.functional.pad(resampled, (0, pad))


def one_pole_envelope(audio: torch.Tensor, time_constant_s: float, sample_rate: int) -> torch.Tensor:
    """Causal exponential smoothing of |audio|, as an FFT convolution with a truncated kernel."""
    coefficient = math.exp(-1.0 / (time_constant_s * sample_rate))
    length = min(int(5.0 * time_constant_s * sample_rate) + 1, audio.shape[-1])
    index = torch.arange(length, device=audio.device, dtype=audio.dtype)
    kernel = (1.0 - coefficient) * coefficient**index
    return fft_convolve(audio.abs(), kernel.unsqueeze(0))


def soft_clip(audio: torch.Tensor, third: float, fifth: float) -> torch.Tensor:
    """Memoryless odd-order polynomial soft clip. Bounded by a tanh guard so a large drive cannot
    make the polynomial diverge and produce a NaN gradient."""
    x = torch.tanh(audio)
    return x - third * x**3 + fifth * x**5


def thd_to_coefficients(thd: float) -> tuple[float, float]:
    """Choose third/fifth-order coefficients giving approximately `thd` total harmonic distortion
    on a full-scale sine. The third-order term dominates; the fifth is held at a fifth of it."""
    third = 4.0 * thd
    return third, third / 5.0
