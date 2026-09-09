"""Spec 2.5 / 2.6. The op set is a design constraint (spec 2.9): Conv2d, nearest Upsample,
GroupNorm, GELU, Linear, tanh, sigmoid, mean, concat. Nothing else appears here, so the graph ports
to candle without an op-coverage investigation and checkerboard artifacts are structurally
impossible.

ONE DELIBERATE DEVIATION FROM THE SPEC TEXT, documented because it changes an exported tensor:
spec 2.6(b) writes the bit output as tanh((mean a+ - mean a-) / tau) and spec 9.4 then applies BCE
to it "on the 56 pooled bit logits". A tanh-bounded value used as a BCE logit caps the attainable
probability at sigmoid(1) = 0.731 and flattens the gradient exactly where the model is confident.
The graph therefore emits the PRE-tanh quantity (mean a+ - mean a-) / tau as `bit_logit`. tanh is
strictly increasing, so neither the hard decision (its sign) nor the confidence ordering the flip
search of spec 3.2 uses (|.|) changes; only the BCE saturation does. A consumer wanting the spec's
soft bit applies tanh itself.
"""

import torch
from torch import nn

from .config import DecoderConfig, EncoderConfig, PayloadConfig, StftConfig


def _groups(channels: int, requested: int) -> int:
    groups = min(requested, channels)
    while channels % groups != 0:
        groups -= 1
    return max(groups, 1)


class ConvBlock(nn.Module):
    def __init__(self, in_channels: int, out_channels: int, groups: int,
                 stride: tuple[int, int] = (1, 1), dilation: tuple[int, int] = (1, 1),
                 kernel: tuple[int, int] = (3, 3)) -> None:
        super().__init__()
        padding = (
            dilation[0] * (kernel[0] - 1) // 2,
            dilation[1] * (kernel[1] - 1) // 2,
        )
        self.conv = nn.Conv2d(in_channels, out_channels, kernel, stride=stride,
                              padding=padding, dilation=dilation)
        self.norm = nn.GroupNorm(_groups(out_channels, groups), out_channels)
        self.act = nn.GELU()

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.act(self.norm(self.conv(x)))


class Encoder(nn.Module):
    """U-Net over log|X|. Downsampling is on FREQUENCY ONLY: time resolution is preserved end to
    end at 93.75 frames/s, which is what keeps the mark a per-frame quantity the presence head can
    localise."""

    def __init__(self, config: EncoderConfig, payload: PayloadConfig) -> None:
        super().__init__()
        self.config = config
        embed = config.message_embed_dim
        self.message_table = nn.Parameter(torch.randn(payload.message_bits, embed) * 0.05)
        self.stem = ConvBlock(1 + embed, config.stem_channels, config.group_norm_groups)

        channels = [config.stem_channels, *config.stage_channels]
        self.down = nn.ModuleList()
        for index in range(len(config.stage_channels)):
            self.down.append(
                nn.Sequential(
                    ConvBlock(channels[index], channels[index + 1], config.group_norm_groups,
                              stride=(2, 1)),
                    ConvBlock(channels[index + 1], channels[index + 1], config.group_norm_groups),
                )
            )
        bottleneck = channels[-1]
        self.film = nn.Linear(embed, 2 * bottleneck)
        self.bottleneck = nn.ModuleList(
            [
                ConvBlock(bottleneck, bottleneck, config.group_norm_groups,
                          dilation=(1, config.bottleneck_time_dilation))
                for _ in range(2)
            ]
        )
        self.upsample = nn.Upsample(scale_factor=(2.0, 1.0), mode="nearest")
        self.up = nn.ModuleList()
        for index in reversed(range(len(config.stage_channels))):
            skip = channels[index]
            current = channels[index + 1]
            self.up.append(ConvBlock(current + skip, skip, config.group_norm_groups))
        self.head = nn.Conv2d(config.stem_channels, 1, kernel_size=1)

    def forward(self, log_magnitude: torch.Tensor, message_bits: torch.Tensor) -> torch.Tensor:
        embedding = message_bits.to(self.message_table.dtype) @ self.message_table
        plane = embedding.unsqueeze(-1).unsqueeze(-1).expand(
            -1, -1, log_magnitude.shape[2], log_magnitude.shape[3]
        )
        x = self.stem(torch.cat([log_magnitude, plane], dim=1))
        skips = [x]
        for stage in self.down:
            x = stage(x)
            skips.append(x)
        film = self.film(embedding)
        scale, shift = film.chunk(2, dim=-1)
        scale = scale.unsqueeze(-1).unsqueeze(-1)
        shift = shift.unsqueeze(-1).unsqueeze(-1)
        for block in self.bottleneck:
            x = block(x) * (1.0 + scale) + shift
        for index, stage in enumerate(self.up):
            x = self.upsample(x)
            x = stage(torch.cat([x, skips[-(index + 2)]], dim=1))
        return torch.tanh(self.head(x))


class Decoder(nn.Module):
    """Fully convolutional and shift-equivariant on the time axis. There is no frame grid to align
    to, so there is no offset search: spec 4.2."""

    def __init__(self, config: DecoderConfig, payload: PayloadConfig) -> None:
        super().__init__()
        self.config = config
        self.message_bits = payload.message_bits
        self.stem = ConvBlock(1, config.stem_channels, config.group_norm_groups, kernel=(5, 7))
        channels = [config.stem_channels, *config.stage_channels]
        self.trunk = nn.ModuleList()
        for index in range(len(config.stage_channels)):
            self.trunk.append(
                nn.Sequential(
                    ConvBlock(channels[index], channels[index + 1], config.group_norm_groups,
                              stride=(2, 1)),
                    ConvBlock(channels[index + 1], channels[index + 1], config.group_norm_groups,
                              dilation=(1, 2**index)),
                )
            )
        trunk_channels = channels[-1]
        self.presence = nn.Sequential(
            nn.Conv2d(trunk_channels, config.presence_hidden, kernel_size=1),
            nn.GELU(),
            nn.Conv2d(config.presence_hidden, 1, kernel_size=1),
        )
        self.message = nn.Conv2d(trunk_channels, 2 * payload.message_bits, kernel_size=1)
        self.log_tau = nn.Parameter(torch.tensor(float(config.tau_init)).log())

    def trunk_features(self, log_magnitude: torch.Tensor) -> torch.Tensor:
        x = self.stem(log_magnitude)
        for stage in self.trunk:
            x = stage(x)
        return x.mean(dim=2, keepdim=True)

    def forward(self, log_magnitude: torch.Tensor) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor]:
        """Returns (presence_logit [B, T], bit_logit [B, 56], bit_trace [B, 56, T])."""
        collapsed = self.trunk_features(log_magnitude)
        presence = self.presence(collapsed).squeeze(1).squeeze(1)
        traces = self.message(collapsed).squeeze(2)
        positive, negative = traces.chunk(2, dim=1)
        weight = torch.sigmoid(presence).unsqueeze(1)
        denominator = weight.sum(dim=-1).clamp_min(1e-6)
        pooled_positive = (positive * weight).sum(dim=-1) / denominator
        pooled_negative = (negative * weight).sum(dim=-1) / denominator
        tau = self.log_tau.exp().clamp_min(1e-3)
        return presence, (pooled_positive - pooled_negative) / tau, positive - negative


class NeuralWatermark(nn.Module):
    """Encoder plus decoder. The STFT is deliberately NOT part of this module (spec 7.2)."""

    def __init__(self, encoder: EncoderConfig, decoder: DecoderConfig,
                 payload: PayloadConfig, stft: StftConfig) -> None:
        super().__init__()
        self.encoder = Encoder(encoder, payload)
        self.decoder = Decoder(decoder, payload)
        self.stft_config = stft
        self.payload_config = payload

    def parameter_report(self) -> dict[str, int]:
        encoder = sum(p.numel() for p in self.encoder.parameters())
        decoder = sum(p.numel() for p in self.decoder.parameters())
        return {
            "encoder": encoder,
            "decoder": decoder,
            "total": encoder + decoder,
            "fp32_bytes": (encoder + decoder) * 4,
        }
