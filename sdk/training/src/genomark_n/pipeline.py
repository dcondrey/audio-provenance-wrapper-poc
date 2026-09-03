"""Marking, the loss set of spec 9.4, and the blind detector of specs 3.2 / 3.5 / 4.5.

THE BLINDNESS INVARIANT, which is the point of this file: `Detector.detect` takes audio and a
FROZEN threshold record. It has no parameter through which the true payload could reach it, so the
oracle best-of-search that invalidates every published physical result (spec 4.1) is not something
this harness avoids by discipline, it is something it cannot express. Truth enters only in
`evaluate.py`, after a detection has already been accepted or refused.
"""

import json
from dataclasses import asdict, dataclass, fields
from pathlib import Path

import numpy as np
import torch
from torch import nn

from .config import BudgetConfig, Config, DetectorConfig
from .model import GenoMarkN
from .payload import Decoded, Message, MessageCodec
from .perceptual import PerceptualModel, masking_budget
from .qmark import QGrid
from .stft import SpectralFront


class Marker(nn.Module):
    """Applies the mark. Spec 2.5: the encoder chooses direction and relative magnitude, the
    perceptual budget chooses absolute magnitude, so fidelity is a property of the budget rather
    than a hoped-for outcome of a loss weight."""

    def __init__(self, model: GenoMarkN, front: SpectralFront, perceptual: PerceptualModel,
                 budget: BudgetConfig) -> None:
        super().__init__()
        self.model = model
        self.front = front
        self.perceptual = perceptual
        self.budget = budget

    def budget_for(self, log_magnitude: torch.Tensor) -> torch.Tensor:
        cfg = self.front.config
        return masking_budget(
            self.perceptual, log_magnitude, self.budget.kappa, self.budget.b_max_nepers,
            cfg.band_first_bin, cfg.band_last_bin, cfg.n_fft, cfg.sample_rate,
        )

    def forward(self, cover: torch.Tensor, bits: torch.Tensor,
                frame_mask: torch.Tensor | None = None) -> dict[str, torch.Tensor]:
        spectrum = self.front.stft(cover)
        log_magnitude = self.front.band_log_magnitude(spectrum)
        budget = self.budget_for(log_magnitude)
        raw = self.model.encoder(log_magnitude, bits)
        gain = raw * budget
        if frame_mask is not None:
            gain = gain * frame_mask.view(frame_mask.shape[0], 1, 1, -1)
        marked = self.front.apply_band_gain(spectrum, gain)
        audio = self.front.istft(marked, cover.shape[-1])
        return {
            "audio": audio,
            "log_gain": gain,
            "raw_gain": raw,
            "budget": budget,
            "cover_log_magnitude": log_magnitude,
        }


def frame_mask_from_spans(span_start: torch.Tensor, span_end: torch.Tensor, is_marked: torch.Tensor,
                          frames: int, front: SpectralFront, device) -> torch.Tensor:
    """Per-frame marked label on the STFT grid.

    IMPORTANT: the centre comes from `SpectralFront.frame_centre_samples`, which accounts for the
    2048-sample lead pad. Reading it as t*hop puts every label four hops ahead of the frames the
    gain was actually applied to, and nothing downstream would notice.
    """
    centre = front.frame_centre_samples(frames, device)
    start = span_start.to(device).unsqueeze(1).float()
    end = span_end.to(device).unsqueeze(1).float()
    inside = (centre.unsqueeze(0) >= start) & (centre.unsqueeze(0) < end)
    return (inside & is_marked.to(device).unsqueeze(1)).float()


def multi_scale_spectral_l1(cover: torch.Tensor, marked: torch.Tensor,
                            n_ffts: tuple[int, ...]) -> torch.Tensor:
    total = cover.new_zeros(())
    for n_fft in n_ffts:
        if cover.shape[-1] < n_fft:
            continue
        window = torch.hann_window(n_fft, device=cover.device, dtype=cover.dtype)
        cover_magnitude = torch.stft(cover, n_fft=n_fft, hop_length=n_fft // 4, window=window,
                                     center=True, return_complex=True).abs()
        marked_magnitude = torch.stft(marked, n_fft=n_fft, hop_length=n_fft // 4, window=window,
                                      center=True, return_complex=True).abs()
        total = total + (cover_magnitude - marked_magnitude).abs().mean()
    return total


@dataclass
class LossBreakdown:
    total: torch.Tensor
    message: torch.Tensor
    message_per_frame: torch.Tensor
    detection: torch.Tensor
    perceptual: torch.Tensor
    spectral: torch.Tensor
    q_coexistence: torch.Tensor
    bit_accuracy: float
    presence_auc_proxy: float

    def scalars(self) -> dict[str, float]:
        out: dict[str, float] = {}
        for entry in fields(self):
            value = getattr(self, entry.name)
            out[entry.name] = float(value.detach().item()) if isinstance(value, torch.Tensor) else float(value)
        return out


def compute_losses(config: Config, marker: Marker, q_grid: QGrid, cover: torch.Tensor,
                   marked: torch.Tensor, degraded: torch.Tensor, bits: torch.Tensor,
                   frame_label: torch.Tensor) -> LossBreakdown:
    front = marker.front
    decoder = marker.model.decoder
    log_magnitude = front.band_log_magnitude(front.stft(degraded))
    presence, bit_logit, traces = decoder(log_magnitude)

    frames = min(presence.shape[-1], frame_label.shape[-1])
    presence = presence[:, :frames]
    traces = traces[:, :, :frames]
    label = frame_label[:, :frames]

    bce = nn.functional.binary_cross_entropy_with_logits
    any_marked = label.amax(dim=-1)
    message_loss = (bce(bit_logit, bits, reduction="none").mean(dim=-1) * any_marked).sum() / any_marked.sum().clamp_min(1.0)

    target = bits.unsqueeze(-1).expand_as(traces)
    per_frame = bce(traces, target, reduction="none")
    weight = label.unsqueeze(1)
    # IMPORTANT: the numerator sums over (batch, 56 bits, frames) and the mask sums over
    # (batch, 1, frames), so the bit count has to enter the denominator. Without it this auxiliary
    # term is 56x its intended size and spec 9.4's weight of 0.2 within L_msg is really 11.2,
    # which makes the aux term ~85% of the total loss and drowns the pooled readout it is meant to
    # regularise.
    denominator = (weight.sum() * traces.shape[1]).clamp_min(1.0)
    per_frame_loss = (per_frame * weight).sum() / denominator

    detection_loss = bce(presence, label)

    perceptual_loss = marker.perceptual.loss(cover, marked)
    spectral_loss = multi_scale_spectral_l1(cover, marked, config.loss.spectral_n_ffts)
    q_loss = q_grid.coexistence_loss(cover, marked)

    weights = config.loss
    total = (
        weights.message * (message_loss + weights.message_per_frame * per_frame_loss)
        + weights.detection * detection_loss
        + weights.perceptual * perceptual_loss
        + weights.spectral * spectral_loss
        + weights.q_coexistence * q_loss
    )

    with torch.no_grad():
        hard = (bit_logit > 0).float()
        marked_rows = any_marked > 0
        accuracy = float((hard[marked_rows] == bits[marked_rows]).float().mean().item()) if marked_rows.any() else float("nan")
        positive = presence[label > 0.5]
        negative = presence[label <= 0.5]
        auc = float((positive.mean() - negative.mean()).item()) if positive.numel() and negative.numel() else float("nan")

    return LossBreakdown(
        total=total,
        message=message_loss,
        message_per_frame=per_frame_loss,
        detection=detection_loss,
        perceptual=perceptual_loss,
        spectral=spectral_loss,
        q_coexistence=q_loss,
        bit_accuracy=accuracy,
        presence_auc_proxy=auc,
    )


@dataclass(frozen=True)
class FrozenThresholds:
    """Spec 3.5 / 12.2. Produced by calibrate.py over unmarked audio ONLY, then never re-tuned.

    Loading a threshold that was fitted on the same trials it is later scored against is the same
    oracle bug as an offset sweep, wearing different clothes. `calibration_trials` and
    `measured_false_positive_rate` travel with the threshold so a report cannot quote a recall
    without the bound that pays for it.
    """

    presence_threshold: float
    target_false_positive_rate: float
    calibration_trials: int
    measured_false_positive_rate: float
    calibrated_at: str
    channels: tuple[str, ...]
    config_digest: str
    weights_digest: str
    calibration_clip_seconds: float
    presence_windows_per_clip: int

    @staticmethod
    def load(path: str | Path) -> "FrozenThresholds":
        record = json.loads(Path(path).read_text(encoding="utf-8"))
        record["channels"] = tuple(record["channels"])
        return FrozenThresholds(**record)

    def save(self, path: str | Path) -> None:
        Path(path).write_text(json.dumps(asdict(self), indent=2, sort_keys=True), encoding="utf-8")


@dataclass
class WindowResult:
    start_frame: int
    presence_span_frames: int
    decoded: Decoded | None


@dataclass
class Detection:
    presence_score: float
    presence_detected: bool
    locator: Message | None
    bits: np.ndarray | None
    bits_corrected: int
    windows_evaluated: int
    crc_trials: int
    confidence_class: str
    frames: int
    presence_windows: int
    findings: tuple[str, ...] = ()


def _median_smooth(x: torch.Tensor, width: int) -> torch.Tensor:
    if width <= 1:
        return x
    pad = width // 2
    padded = nn.functional.pad(x.unsqueeze(1), (pad, pad), mode="replicate")
    return padded.unfold(-1, width, 1).median(dim=-1).values.squeeze(1)[..., : x.shape[-1]]


class Detector:
    """Blind. Spec 4.5's window schedule, spec 3.2's fixed 11 CRC trials, spec 3.5's frozen
    presence threshold. Nothing here can see a true payload."""

    def __init__(self, model: GenoMarkN, front: SpectralFront, config: DetectorConfig,
                 codec: MessageCodec) -> None:
        self.model = model
        self.front = front
        self.config = config
        self.codec = codec

    @torch.no_grad()
    def detect(self, audio: torch.Tensor, thresholds: FrozenThresholds) -> Detection:
        if audio.dim() == 1:
            audio = audio.unsqueeze(0)
        if audio.shape[0] != 1:
            raise ValueError("detect operates on one clip at a time")
        stft = self.front.config
        log_magnitude = self.front.band_log_magnitude(self.front.stft(audio))
        presence, _, traces = self.model.decoder(log_magnitude)
        frames = presence.shape[-1]

        smooth_frames = max(int(self.config.presence_smooth_seconds * stft.frames_per_second), 1)
        smoothed = _median_smooth(presence, smooth_frames | 1)
        probability = torch.sigmoid(smoothed)

        presence_window = max(int(self.config.presence_window_seconds * stft.frames_per_second), 1)
        if frames >= presence_window:
            integral = probability.unfold(-1, presence_window, max(presence_window // 2, 1)).mean(dim=-1)
            score = float(integral.max().item())
            presence_windows = int(integral.shape[-1])
        else:
            score = float(probability.mean().item())
            presence_windows = 1
        presence_detected = score >= thresholds.presence_threshold

        findings: list[str] = []
        if thresholds.presence_windows_per_clip and presence_windows > thresholds.presence_windows_per_clip:
            findings.append(
                f"presence threshold was calibrated over {thresholds.presence_windows_per_clip} "
                f"window(s) per clip ({thresholds.calibration_clip_seconds:g} s); this clip has "
                f"{presence_windows}. The presence score is a maximum over windows, so its "
                f"false-positive rate here is HIGHER than the calibrated "
                f"{thresholds.measured_false_positive_rate:g} and that rate may not be quoted for "
                "this duration."
            )

        window_frames = max(int(self.config.window_seconds * stft.frames_per_second), 1)
        hop_frames = max(int(self.config.window_hop_seconds * stft.frames_per_second), 1)
        min_span = int(self.config.min_presence_span_seconds * stft.frames_per_second)
        tau = self.model.decoder.log_tau.exp().clamp_min(1e-3)

        results: list[WindowResult] = []
        duration = frames / stft.frames_per_second
        if duration < self.config.min_locator_seconds:
            # Spec 4.6: below the reported minimum duration the SDK refuses a locator read rather
            # than returning a low-confidence guess. `crates/genomark-n` gates on the same value.
            starts: list[int] = []
        else:
            starts = list(range(0, max(frames - window_frames, 0) + 1, hop_frames))[
                : self.config.max_windows
            ]
            if not starts:
                starts = [0]
        for start in starts:
            stop = min(start + window_frames, frames)
            mask = (probability[:, start:stop] > self.config.presence_frame_threshold).float()
            span = int(mask.sum().item())
            if span < min_span:
                results.append(WindowResult(start, span, None))
                continue
            segment = traces[:, :, start:stop]
            pooled = (segment * mask.unsqueeze(1)).sum(dim=-1) / mask.sum(dim=-1).clamp_min(1.0)
            logits = (pooled / tau).squeeze(0).cpu().numpy()
            results.append(WindowResult(start, span, self.codec.decode(logits)))

        accepted: list[tuple[int, Decoded]] = [
            (r.start_frame, r.decoded) for r in results if r.decoded is not None
        ]
        locator: Message | None = None
        bits: np.ndarray | None = None
        corrected = 0
        confidence = "none"
        if accepted:
            payloads: dict[bytes, list[tuple[int, Decoded]]] = {}
            for start_frame, decoded in accepted:
                payloads.setdefault(decoded.bits.tobytes(), []).append((start_frame, decoded))
            group = max(payloads.values(), key=len)
            best = group[0][1]
            locator = best.message
            bits = best.bits
            corrected = best.bits_corrected
            non_overlapping = len({start // window_frames for start, _ in group})
            confidence = "locator_multi" if non_overlapping >= 2 else "locator_single"
        elif presence_detected:
            confidence = "presence"

        return Detection(
            presence_score=score,
            presence_detected=presence_detected,
            locator=locator,
            bits=bits,
            bits_corrected=corrected,
            windows_evaluated=len(results),
            crc_trials=len(results) * self.codec.crc_trials,
            confidence_class=confidence,
            frames=frames,
            presence_windows=presence_windows,
            findings=tuple(findings),
        )
