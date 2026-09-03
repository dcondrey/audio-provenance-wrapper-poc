"""Threshold calibration on UNMARKED audio only, and the kappa sweep of build unit N-C0.

Spec 3.5 and 12.2: the presence threshold is fixed at the operating point where the false-positive
rate over >= 5000 unmarked simulated trials meets the target, and it is NEVER re-tuned upward to
recover recall. The FPR is the constraint and the recall is the result. This script therefore never
sees a marked clip; recall is measured later, by evaluate.py, at whatever threshold this produced.
"""

import argparse
import dataclasses
import json
import time
from pathlib import Path

import numpy as np
import torch

from .channels import ChannelBank
from .config import Config, load_config
from .data.synthetic import SyntheticCorpus
from .evaluate import build_harness, load_model, mark_clip
from .payload import MessageCodec
from .pipeline import FrozenThresholds
from .runlog import digest_text
from .seeding import resolve_device

UNREACHABLE = float("inf")


def presence_scores(config: Config, model, device: torch.device, trials: int,
                    channels: tuple[str, ...]) -> tuple[np.ndarray, int, int]:
    """Presence scores over unmarked audio. Also counts CRC-gated locator accepts, which is the
    other half of the false-positive picture and has no threshold to tune."""
    _, detector, _, bank = build_harness(config, model, device)
    corpus = SyntheticCorpus(
        items=trials,
        seconds=config.evaluation.clip_seconds,
        sample_rate=config.stft.sample_rate,
        base_seed=config.run.seed + 40_009,
    )
    open_gate = FrozenThresholds(
        presence_threshold=UNREACHABLE, target_false_positive_rate=1.0, calibration_trials=0,
        measured_false_positive_rate=1.0, calibrated_at="", channels=(), config_digest="",
        weights_digest="", calibration_clip_seconds=config.evaluation.clip_seconds,
        presence_windows_per_clip=0,
    )
    samples = int(config.evaluation.clip_seconds * config.stft.sample_rate)
    scores: list[float] = []
    locator_accepts = 0
    windows = 0
    for name in channels:
        channel = bank.build(name)
        for index in range(trials):
            cover = torch.from_numpy(corpus.load(index)[:samples]).unsqueeze(0).to(device)
            degraded = channel.apply(cover, seed=500_000 + index)
            detection = detector.detect(degraded, open_gate)
            scores.append(detection.presence_score)
            windows = max(windows, detection.presence_windows)
            locator_accepts += int(detection.bits is not None)
    return np.asarray(scores), locator_accepts, windows


def choose_threshold(scores: np.ndarray, target_rate: float) -> tuple[float, float]:
    """Smallest threshold whose accept count is within the target rate. Never a quantile of the
    marked distribution, and never adjusted afterwards."""
    if scores.size == 0:
        raise ValueError("no calibration trials")
    ordered = np.sort(scores)[::-1]
    allowed = int(np.floor(target_rate * scores.size))
    if allowed <= 0:
        threshold = float(ordered[0]) + 1e-6
    else:
        threshold = float(ordered[allowed - 1]) + 1e-9
    measured = float((scores >= threshold).mean())
    return threshold, measured


def calibrate(config: Config, checkpoint: str, out: str, device: torch.device,
              trials: int | None = None) -> FrozenThresholds:
    model = load_model(checkpoint, config, device)
    channels = tuple(
        name for name in config.evaluation.channels if name.startswith("acoustic")
    ) or config.evaluation.channels
    count = trials or config.evaluation.false_positive_trials
    scores, locator_accepts, windows = presence_scores(config, model, device, count, channels)
    threshold, measured = choose_threshold(scores, config.evaluation.target_false_positive_rate)

    weights = torch.load(checkpoint, map_location="cpu", weights_only=False)
    frozen = FrozenThresholds(
        presence_threshold=threshold,
        target_false_positive_rate=config.evaluation.target_false_positive_rate,
        calibration_trials=int(scores.size),
        measured_false_positive_rate=measured,
        calibrated_at=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        channels=channels,
        config_digest=digest_text(config.to_json()),
        weights_digest=digest_text(str(sorted(weights["model"].keys()))),
        calibration_clip_seconds=config.evaluation.clip_seconds,
        presence_windows_per_clip=windows,
    )
    frozen.save(out)

    sufficient = scores.size >= 3.0 / config.evaluation.target_false_positive_rate
    Path(str(out) + ".notes.json").write_text(
        json.dumps(
            {
                "trials": int(scores.size),
                "trials_required_for_stated_target": int(np.ceil(3.0 / config.evaluation.target_false_positive_rate)),
                "sufficient_trials": bool(sufficient),
                "bound_3_over_n": 3.0 / max(scores.size, 1),
                "locator_accepts_on_unmarked": locator_accepts,
                "locator_crc_trials_per_window": MessageCodec(config.payload).crc_trials,
                "score_percentiles": {
                    str(p): float(np.percentile(scores, p)) for p in (50, 90, 99, 99.9, 100)
                },
                "warning": None if sufficient else (
                    "TRIAL COUNT IS BELOW 3/target. This threshold does NOT establish the stated "
                    "false-positive rate; spec 3.5 requires >= 5000 unmarked simulated trials before "
                    "any physical run. Treat it as a smoke-test artifact only."
                ),
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    return frozen


def kappa_sweep(config: Config, checkpoint: str, device: torch.device,
                values: tuple[float, ...]) -> list[dict]:
    """Build unit N-C0. Reports the perceptual measurement the bench reports, at each kappa, so the
    budget is calibrated against the same function spec 9.4 optimises."""
    model = load_model(checkpoint, config, device)
    corpus = SyntheticCorpus(
        items=config.evaluation.corpus_items,
        seconds=config.evaluation.clip_seconds,
        sample_rate=config.stft.sample_rate,
        base_seed=config.run.seed + 991,
    )
    codec = MessageCodec(config.payload)
    rng = np.random.default_rng(config.run.seed + 3)
    samples = int(config.evaluation.clip_seconds * config.stft.sample_rate)
    rows = []
    for kappa in values:
        adjusted = dataclasses.replace(config, budget=dataclasses.replace(config.budget, kappa=kappa))
        adjusted.validate()
        marker, _, _, _ = build_harness(adjusted, model, device)
        worst = {"noise_to_mask_max_db": -1e9, "frames_above_mask_fraction": 0.0,
                 "segmental_snr_db": 1e9}
        for index in range(config.evaluation.corpus_items):
            cover = torch.from_numpy(corpus.load(index)[:samples]).unsqueeze(0).to(device)
            bits = torch.from_numpy(codec.encode(codec.random_message(rng))).float().unsqueeze(0).to(device)
            marked = mark_clip(marker, cover, bits)
            measured = marker.perceptual.measure(cover, marked)
            if measured["noise_to_mask_max_db"] is not None:
                worst["noise_to_mask_max_db"] = max(worst["noise_to_mask_max_db"], measured["noise_to_mask_max_db"])
                worst["frames_above_mask_fraction"] = max(
                    worst["frames_above_mask_fraction"], measured["frames_above_mask_fraction"]
                )
            if measured["segmental_snr_db"] is not None:
                worst["segmental_snr_db"] = min(worst["segmental_snr_db"], measured["segmental_snr_db"])
        rows.append({
            "kappa": kappa,
            **worst,
            "k2_audibility_pass": (
                worst["frames_above_mask_fraction"] <= 0.10
                and worst["noise_to_mask_max_db"] <= 3.0
                and worst["segmental_snr_db"] >= 22.0
            ),
        })
    return rows


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Freeze GenoMark-N thresholds on unmarked audio.")
    parser.add_argument("--config", required=True)
    parser.add_argument("--checkpoint", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--trials", type=int, default=None)
    parser.add_argument("--device", default=None)
    parser.add_argument("--kappa-sweep", default=None,
                        help="comma-separated kappa values; runs build unit N-C0 instead of freezing")
    args = parser.parse_args(argv)

    overrides = {"run": {"device": args.device}} if args.device else None
    config = load_config(args.config, overrides)
    device = resolve_device(config.run.device)

    if args.kappa_sweep:
        values = tuple(float(v) for v in args.kappa_sweep.split(","))
        rows = kappa_sweep(config, args.checkpoint, device, values)
        Path(args.out).write_text(json.dumps({"n_c0_kappa_sweep": rows}, indent=2), encoding="utf-8")
        for row in rows:
            print(
                f"kappa {row['kappa']:.3f} nmr_max {row['noise_to_mask_max_db']:7.2f} dB "
                f"above_mask {row['frames_above_mask_fraction']:.3f} "
                f"segSNR {row['segmental_snr_db']:6.2f} dB K2 {'pass' if row['k2_audibility_pass'] else 'FAIL'}"
            )
        return 0

    frozen = calibrate(config, args.checkpoint, args.out, device, args.trials)
    notes = json.loads(Path(str(args.out) + ".notes.json").read_text(encoding="utf-8"))
    print(
        f"presence_threshold {frozen.presence_threshold:.6f} over {frozen.calibration_trials} "
        f"unmarked trials, measured FPR {frozen.measured_false_positive_rate:.6f}, "
        f"3/N bound {notes['bound_3_over_n']:.6f}"
    )
    if notes["warning"]:
        print(f"WARNING: {notes['warning']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
