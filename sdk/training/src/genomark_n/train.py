"""Training loop for GenoMark-N.

Curriculum, losses, optimiser and schedule are spec 9.3 / 9.4 / 9.5. Nothing here selects a
threshold or touches the evaluation set: thresholds come from calibrate.py over unmarked audio and
are frozen before evaluate.py runs.
"""

import argparse
import math
import time
from pathlib import Path

import numpy as np
import torch

from .config import Config, load_config
from .data.dataset import GenoMarkNDataset, collate
from .data.synthetic import SyntheticCorpus
from .distortion.chain import DistortionChain
from .model import GenoMarkN
from .payload import MessageCodec
from .perceptual import PerceptualModel
from .pipeline import Marker, compute_losses, frame_mask_from_spans
from .qmark import QGrid
from .rir.corpus import RirCorpus
from .runlog import Ema, RunDirectory
from .seeding import resolve_device, seed_everything
from .stft import SpectralFront

REPO_ROOT = Path(__file__).resolve().parents[3]


def phase_for(step: int, config: Config) -> tuple[str, float]:
    curriculum = config.curriculum
    if step < curriculum.phase_a_end:
        return "A", 0.0
    if step < curriculum.phase_b_end:
        span = max(curriculum.phase_b_end - curriculum.phase_a_end, 1)
        return "B", (step - curriculum.phase_a_end) / span
    return "C", 1.0


def learning_rate(step: int, config: Config) -> float:
    optim = config.optim
    if step < optim.warmup_steps:
        return optim.lr * (step + 1) / max(optim.warmup_steps, 1)
    span = max(optim.total_steps - optim.warmup_steps, 1)
    progress = min((step - optim.warmup_steps) / span, 1.0)
    return optim.min_lr + 0.5 * (optim.lr - optim.min_lr) * (1.0 + math.cos(math.pi * progress))


def build(config: Config, device: torch.device):
    model = GenoMarkN(config.encoder, config.decoder, config.payload, config.stft).to(device)
    front = SpectralFront(config.stft).to(device)
    perceptual = PerceptualModel(config.perceptual, config.stft.sample_rate).to(device)
    marker = Marker(model, front, perceptual, config.budget).to(device)
    q_grid = QGrid(config.q, config.stft.sample_rate, device=device)
    rir = RirCorpus(config.rir, config.stft.sample_rate)
    chain = DistortionChain(config.distortion, config.codec, config.stft.sample_rate, rir, q_grid)
    return model, marker, q_grid, chain, rir


def train(config: Config, steps: int | None = None, force_codec_first_step: bool = True) -> Path:
    device = resolve_device(config.run.device)
    seed = seed_everything(config.run.seed, device, config.run.deterministic)
    run = RunDirectory(config.run.out_dir, config, seed, REPO_ROOT)
    total_steps = steps or config.run.max_steps or config.optim.total_steps

    model, marker, q_grid, chain, rir = build(config, device)
    run.log({"event": "model", **model.parameter_report(), "rir": rir.provenance()})

    synthetic = (
        SyntheticCorpus(
            items=config.data.synthetic_items,
            seconds=config.data.source_clip_seconds,
            sample_rate=config.stft.sample_rate,
            base_seed=config.run.seed,
        )
        if config.data.synthetic_items > 0
        else None
    )
    dataset = GenoMarkNDataset(
        config.data, config.distortion, config.codec, config.payload,
        config.stft.sample_rate, epoch_size=max(total_steps, 1) * config.optim.batch_size,
        base_seed=config.run.seed, unmarked_fraction=config.optim.unmarked_fraction,
        synthetic=synthetic,
    )

    optimiser = torch.optim.AdamW(
        model.parameters(), lr=config.optim.lr, betas=config.optim.betas,
        weight_decay=config.optim.weight_decay,
    )
    ema = Ema(model, config.optim.ema_decay)
    generator = torch.Generator(device="cpu").manual_seed(config.run.seed)
    batch_rng = np.random.default_rng(config.run.seed)
    codec = MessageCodec(config.payload)
    run.log({"event": "codec", "crc_trials_per_window": codec.crc_trials})

    started = time.time()
    for step in range(total_steps):
        for group in optimiser.param_groups:
            group["lr"] = learning_rate(step, config)
        indices = range(step * config.optim.batch_size, (step + 1) * config.optim.batch_size)
        batch = collate([dataset[i] for i in indices], config.data.analysis_seconds,
                        config.stft.sample_rate, batch_rng)

        cover = batch.audio.to(device)
        bits = batch.bits.to(device)
        frames = marker.front.frames_for(cover.shape[-1])
        label = frame_mask_from_spans(batch.span_start, batch.span_end, batch.is_marked,
                                      frames, marker.front, device)

        marked = marker(cover, bits, label)
        phase, progress = phase_for(step, config)
        outcome = chain(marked["audio"], generator, phase, progress,
                        force_capture_codec=force_codec_first_step and step == 0)
        losses = compute_losses(config, marker, q_grid, cover, marked["audio"], outcome.audio,
                                bits, label)

        optimiser.zero_grad(set_to_none=True)
        losses.total.backward()
        grad_norm = torch.nn.utils.clip_grad_norm_(model.parameters(), config.optim.grad_clip)
        optimiser.step()
        ema.update(model)

        if step % config.run.log_every == 0 or step == total_steps - 1:
            record = {
                "event": "step",
                "step": step,
                "phase": phase,
                "lr": learning_rate(step, config),
                "grad_norm": float(grad_norm),
                "seconds_per_step": (time.time() - started) / max(step + 1, 1),
                "stages": outcome.stages,
                "analysis_seconds": batch.analysis_samples / config.stft.sample_rate,
                "budget_mean_nepers": float(marked["budget"].mean().item()),
                **losses.scalars(),
            }
            run.log(record)
            print(
                f"step {step:6d} {phase} loss {record['total']:8.4f} msg {record['message']:6.4f} "
                f"det {record['detection']:6.4f} perc {record['perceptual']:8.4f} "
                f"acc {record['bit_accuracy']:.3f} {record['seconds_per_step']:.2f}s/step",
                flush=True,
            )
        if config.run.checkpoint_every and (step + 1) % config.run.checkpoint_every == 0:
            run.checkpoint(step + 1, model, optimiser, ema.state_dict())

    path = run.checkpoint(total_steps, model, optimiser, ema.state_dict())
    elapsed = time.time() - started
    run.log({
        "event": "finished",
        "steps": total_steps,
        "wall_seconds": elapsed,
        "seconds_per_step": elapsed / max(total_steps, 1),
        "projected_320k_days": elapsed / max(total_steps, 1) * 320_000 / 86_400,
        "codec_calls": chain.codec.calls,
        "codec_failures": chain.codec_failures,
    })
    print(
        f"finished {total_steps} steps in {elapsed:.1f}s "
        f"({elapsed / max(total_steps, 1):.2f} s/step, "
        f"projected {elapsed / max(total_steps, 1) * 320_000 / 86_400:.1f} days for 320k steps)"
    )
    return path


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Train GenoMark-N (genomark-neural-v1).")
    parser.add_argument("--config", required=True)
    parser.add_argument("--steps", type=int, default=None)
    parser.add_argument("--out", default=None)
    parser.add_argument("--device", default=None)
    args = parser.parse_args(argv)
    overrides: dict = {}
    if args.out:
        overrides.setdefault("run", {})["out_dir"] = args.out
    if args.device:
        overrides.setdefault("run", {})["device"] = args.device
    config = load_config(args.config, overrides)
    path = train(config, steps=args.steps)
    print(f"checkpoint {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
