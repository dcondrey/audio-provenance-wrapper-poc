"""Phase A's own precondition, run in three minutes instead of 60k steps.

Spec 9.3: clean-channel bit accuracy > 0.999 is the goal of Phase A, and "a model that cannot do
this cannot do anything harder; a Phase A failure is a bug, not a result." This script is the
smallest experiment that separates the two. One clip, one payload, the full marked span, THE
DISTORTION CHAIN ENTIRELY BYPASSED. The only thing under test is

    encode -> budget -> encoder -> gain -> ISTFT -> STFT -> decoder -> pooled logits -> decode

so a failure is a wiring bug in that path -- bit ordering, the tau division, the frame mask, the
positive/negative split in the message head -- and not a statement about whether the mark can
survive a room. Passing it does not mean the design works. Failing it means nothing downstream is
worth measuring.
"""

import argparse
import json
import time
from pathlib import Path

import numpy as np
import torch
from torch import nn

from .config import load_config
from .data.synthetic import SyntheticCorpus
from .model import GenoMarkN
from .payload import MessageCodec
from .perceptual import PerceptualModel
from .pipeline import Marker
from .seeding import resolve_device, seed_everything
from .stft import SpectralFront


def run(config_path: str, steps: int, lr: float, seconds: float, device_name: str,
        target_accuracy: float) -> dict:
    config = load_config(config_path, {"run": {"device": device_name}} if device_name else None)
    device = resolve_device(config.run.device)
    seed_everything(config.run.seed, device, deterministic=False)

    model = GenoMarkN(config.encoder, config.decoder, config.payload, config.stft).to(device)
    front = SpectralFront(config.stft).to(device)
    perceptual = PerceptualModel(config.perceptual, config.stft.sample_rate).to(device)
    marker = Marker(model, front, perceptual, config.budget).to(device)
    codec = MessageCodec(config.payload)

    corpus = SyntheticCorpus(items=1, seconds=seconds, sample_rate=config.stft.sample_rate,
                             base_seed=config.run.seed)
    cover = torch.from_numpy(corpus.load(0)).unsqueeze(0).to(device)
    truth = codec.encode(codec.random_message(np.random.default_rng(config.run.seed)))
    bits = torch.from_numpy(truth).float().unsqueeze(0).to(device)
    frames = front.frames_for(cover.shape[-1])
    mask = torch.ones(1, frames, device=device)

    optimiser = torch.optim.AdamW(model.parameters(), lr=lr, betas=(0.9, 0.99))
    bce = nn.functional.binary_cross_entropy_with_logits
    history = []
    started = time.time()
    for step in range(steps):
        marked = marker(cover, bits, mask)
        log_magnitude = front.band_log_magnitude(front.stft(marked["audio"]))
        presence, bit_logit, traces = model.decoder(log_magnitude)
        usable = min(presence.shape[-1], frames)
        loss = (
            bce(bit_logit, bits)
            + 0.2 * bce(traces[:, :, :usable], bits.unsqueeze(-1).expand(-1, -1, usable))
            + bce(presence[:, :usable], mask[:, :usable])
        )
        optimiser.zero_grad(set_to_none=True)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        optimiser.step()

        if step % 20 == 0 or step == steps - 1:
            with torch.no_grad():
                accuracy = float(((bit_logit > 0).float() == bits).float().mean().item())
            history.append({"step": step, "loss": float(loss.item()), "bit_accuracy": accuracy})
            print(f"step {step:4d} loss {loss.item():8.4f} bit_accuracy {accuracy:.4f}", flush=True)

    with torch.no_grad():
        marked = marker(cover, bits, mask)
        log_magnitude = front.band_log_magnitude(front.stft(marked["audio"]))
        _, bit_logit, _ = model.decoder(log_magnitude)
        accuracy = float(((bit_logit > 0).float() == bits).float().mean().item())
        decoded = codec.decode(bit_logit.squeeze(0).cpu().numpy())
        residual = float((marked["audio"] - cover).abs().max().item())
        measurement = perceptual.measure(cover, marked["audio"])

    result = {
        "steps": steps,
        "wall_seconds": time.time() - started,
        "device": str(device),
        "final_bit_accuracy": accuracy,
        "target_bit_accuracy": target_accuracy,
        "crc_decoded": decoded is not None,
        "crc_bits_match_truth": bool(decoded is not None and np.array_equal(decoded.bits, truth)),
        "bits_corrected": decoded.bits_corrected if decoded else None,
        "peak_residual": residual,
        "segmental_snr_db": measurement["segmental_snr_db"],
        "noise_to_mask_max_db": measurement["noise_to_mask_max_db"],
        "history": history,
    }
    result["pass"] = bool(accuracy >= target_accuracy and result["crc_bits_match_truth"])
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Overfit one clip with the distortion chain bypassed: wiring bug vs architecture."
    )
    parser.add_argument("--config", default="configs/smoke.yaml")
    parser.add_argument("--steps", type=int, default=400)
    parser.add_argument("--lr", type=float, default=1e-3)
    parser.add_argument("--seconds", type=float, default=2.0)
    parser.add_argument("--device", default=None)
    parser.add_argument("--target", type=float, default=0.9)
    parser.add_argument("--out", default=None)
    args = parser.parse_args(argv)

    result = run(args.config, args.steps, args.lr, args.seconds, args.device, args.target)
    if args.out:
        Path(args.out).write_text(json.dumps(result, indent=2), encoding="utf-8")
    print()
    print(f"final bit accuracy   {result['final_bit_accuracy']:.4f}  (target {result['target_bit_accuracy']})")
    print(f"CRC decoded          {result['crc_decoded']}")
    print(f"CRC bits == truth    {result['crc_bits_match_truth']}  bits_corrected {result['bits_corrected']}")
    print(f"peak residual        {result['peak_residual']:.6f}")
    print(f"segmental SNR        {result['segmental_snr_db']:.2f} dB")
    print(f"noise-to-mask max    {result['noise_to_mask_max_db']:.2f} dB")
    print(f"wall                 {result['wall_seconds']:.1f} s on {result['device']}")
    print(f"RESULT               {'PASS' if result['pass'] else 'FAIL'}")
    return 0 if result["pass"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
