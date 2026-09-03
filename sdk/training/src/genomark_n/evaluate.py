"""Blind evaluation, mirroring what crates/audio-provenance-bench measures.

THE RULE THIS FILE EXISTS TO ENFORCE. `Detector.detect` is called with the audio and a frozen
threshold record and nothing else. Only AFTER it has returned does this module look at the true
payload, and only to score. There is no offset sweep, no best-of-N over the payload, and no
threshold fitted on these trials. Spec 4.1 names the oracle best-of-search as the reason no
published physical number means what it appears to mean; a harness that reproduced it would produce
the same worthless number.
"""

import argparse
import json
import time
from pathlib import Path

import numpy as np
import torch

from .channels import ChannelBank
from .config import Config, load_config
from .data.synthetic import SyntheticCorpus
from .model import GenoMarkN
from .payload import MessageCodec, false_accept_bound
from .perceptual import PerceptualModel
from .pipeline import Detector, FrozenThresholds, Marker
from .seeding import resolve_device
from .stft import SpectralFront

SCHEMA = "genomark-n-training-eval/1"

BLINDNESS_NOTE = (
    "Every detection in this report was produced by a detector that received the audio and a "
    "threshold record frozen before the run, and no ground truth. No offset was searched, no window "
    "was selected by its agreement with a known payload, and no threshold was fitted on these "
    "trials. The published acoustic literature reports best-of-search maxima computed with "
    "knowledge of the true bits; those numbers and these are not the same quantity."
)

FALSE_POSITIVE_NOTE = (
    "The false-positive arm runs the detector over UNMARKED audio through the same channel. A rate "
    "of 0.0 over N trials bounds the rate at roughly 3/N with 95% confidence; it does not establish "
    "that the rate is zero. The trial count is reported next to it so the bound can be computed "
    "rather than assumed."
)


def load_model(checkpoint: str | Path, config: Config, device: torch.device,
               use_ema: bool = True) -> GenoMarkN:
    payload = torch.load(checkpoint, map_location="cpu", weights_only=False)
    model = GenoMarkN(config.encoder, config.decoder, config.payload, config.stft)
    state = payload.get("ema") if use_ema and payload.get("ema") else payload["model"]
    model.load_state_dict(state)
    return model.to(device).eval()


def build_harness(config: Config, model: GenoMarkN, device: torch.device):
    front = SpectralFront(config.stft).to(device)
    perceptual = PerceptualModel(config.perceptual, config.stft.sample_rate).to(device)
    marker = Marker(model, front, perceptual, config.budget).to(device)
    codec = MessageCodec(config.payload)
    detector = Detector(model, front, config.detector, codec)
    bank = ChannelBank(config.stft.sample_rate, config.codec, config.rir)
    return marker, detector, codec, bank


@torch.no_grad()
def mark_clip(marker: Marker, audio: torch.Tensor, bits: torch.Tensor) -> torch.Tensor:
    frames = marker.front.frames_for(audio.shape[-1])
    mask = torch.ones(1, frames, device=audio.device)
    return marker(audio, bits, mask)["audio"]


def evaluate(config: Config, model: GenoMarkN, thresholds: FrozenThresholds,
             device: torch.device, corpus: SyntheticCorpus | None = None) -> dict:
    marker, detector, codec, bank = build_harness(config, model, device)
    corpus = corpus or SyntheticCorpus(
        items=config.evaluation.corpus_items,
        seconds=config.evaluation.clip_seconds,
        sample_rate=config.stft.sample_rate,
        base_seed=config.run.seed + 991,
    )
    truth_rng = np.random.default_rng(config.run.seed + 7717)
    samples = int(config.evaluation.clip_seconds * config.stft.sample_rate)

    clips: list[tuple[torch.Tensor, np.ndarray, str]] = []
    for index in range(config.evaluation.corpus_items):
        cover = torch.from_numpy(corpus.load(index)[:samples]).unsqueeze(0).to(device)
        bits = codec.encode(codec.random_message(truth_rng))
        marked = mark_clip(marker, cover, torch.from_numpy(bits).float().unsqueeze(0).to(device))
        clips.append((marked, bits, corpus.content_class(index)))

    negatives = [
        torch.from_numpy(corpus.load(index + config.evaluation.corpus_items)[:samples])
        .unsqueeze(0).to(device)
        for index in range(config.evaluation.false_positive_trials)
    ]

    channels = []
    for name in config.evaluation.channels:
        channel = bank.build(name)
        rows = []
        detect_seconds: list[float] = []
        exact = 0
        returned = 0
        bit_errors: list[float] = []
        presence_hits = 0
        for index, (marked, truth, content_class) in enumerate(clips):
            degraded = channel.apply(marked, seed=1_000 + index)
            started = time.perf_counter()
            detection = detector.detect(degraded, thresholds)
            detect_seconds.append(time.perf_counter() - started)

            is_exact = detection.bits is not None and np.array_equal(detection.bits, truth)
            exact += int(is_exact)
            returned += int(detection.bits is not None)
            presence_hits += int(detection.presence_detected)
            if detection.bits is not None:
                bit_errors.append(float(np.mean(detection.bits != truth)))
            rows.append({
                "item": index,
                "content_class": content_class,
                "confidence_class": detection.confidence_class,
                "exact": is_exact,
                "payload_returned": detection.bits is not None,
                "bits_corrected": detection.bits_corrected,
                "presence_score": detection.presence_score,
                "presence_detected": detection.presence_detected,
                "windows_evaluated": detection.windows_evaluated,
                "crc_trials": detection.crc_trials,
            })

        locator_accepts = 0
        presence_accepts = 0
        for index, cover in enumerate(negatives):
            degraded = channel.apply(cover, seed=90_000 + index)
            detection = detector.detect(degraded, thresholds)
            locator_accepts += int(detection.bits is not None)
            presence_accepts += int(detection.presence_detected)

        trials = len(clips)
        fp_trials = len(negatives)
        channels.append({
            "name": name,
            "family": channel.family,
            "params": channel.params,
            "trials": trials,
            "exact_recoveries": exact,
            "exact_recovery_rate": exact / trials if trials else None,
            "payload_returned_rate": returned / trials if trials else None,
            "mean_bit_error_rate": float(np.mean(bit_errors)) if bit_errors else None,
            "presence_recall": presence_hits / trials if trials else None,
            "mean_detect_seconds": float(np.mean(detect_seconds)) if detect_seconds else None,
            "p95_detect_seconds": float(np.percentile(detect_seconds, 95)) if detect_seconds else None,
            "false_positive": {
                "trials": fp_trials,
                "locator_accepts": locator_accepts,
                "presence_accepts": presence_accepts,
                "locator_rate": locator_accepts / fp_trials if fp_trials else None,
                "presence_rate": presence_accepts / fp_trials if fp_trials else None,
                "upper_bound_95": 3.0 / fp_trials if fp_trials else None,
            },
            "rows": rows,
        })

    return {
        "schema": SCHEMA,
        "algorithm_id": "genomark-neural-v1",
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "device": str(device),
        "clip_seconds": config.evaluation.clip_seconds,
        "corpus": corpus.provenance(),
        "thresholds": {
            "presence_threshold": thresholds.presence_threshold,
            "calibration_trials": thresholds.calibration_trials,
            "measured_false_positive_rate": thresholds.measured_false_positive_rate,
            "calibrated_at": thresholds.calibrated_at,
            "calibration_channels": list(thresholds.channels),
        },
        "crc_trials_per_window": MessageCodec(config.payload).crc_trials,
        "max_false_accepts_per_file": false_accept_bound(
            config.detector.max_windows, MessageCodec(config.payload)
        ),
        "channels": channels,
        "notes": [BLINDNESS_NOTE, FALSE_POSITIVE_NOTE],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Blind evaluation of a GenoMark-N checkpoint.")
    parser.add_argument("--config", required=True)
    parser.add_argument("--checkpoint", required=True)
    parser.add_argument("--thresholds", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--device", default=None)
    args = parser.parse_args(argv)

    overrides = {"run": {"device": args.device}} if args.device else None
    config = load_config(args.config, overrides)
    device = resolve_device(config.run.device)
    model = load_model(args.checkpoint, config, device)
    thresholds = FrozenThresholds.load(args.thresholds)
    report = evaluate(config, model, thresholds, device)
    Path(args.out).write_text(json.dumps(report, indent=2), encoding="utf-8")
    for channel in report["channels"]:
        fp = channel["false_positive"]
        print(
            f"{channel['name']:22s} exact {channel['exact_recovery_rate']:.3f} "
            f"returned {channel['payload_returned_rate']:.3f} "
            f"presence {channel['presence_recall']:.3f} "
            f"fp_locator {fp['locator_accepts']}/{fp['trials']} "
            f"fp_presence {fp['presence_accepts']}/{fp['trials']} "
            f"detect {channel['mean_detect_seconds']:.3f}s"
        )
    print(f"report {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
