"""Cross-language parity fixtures for bench row N-B12.

Spec 7.4 makes the port a GATE, not a hope: the Rust decoder's 56 bit logits and per-frame presence
logits must agree with this PyTorch reference to max absolute error < 1e-3 in fp32, and the
accept/reject decision must agree on 100% of trials. A silently diverging port is the classic
failure in this class of work and it is cheap to measure.

This writes the reference side of that comparison in a format with no Python in it: raw
little-endian f32 blobs plus a JSON manifest carrying shapes and SHA-256 digests. The Rust reads
the input blob, runs its own decoder, and diffs against the expected blob.
"""

import argparse
import json
from pathlib import Path

import numpy as np
import torch

from .channels import ChannelBank
from .config import load_config
from .data.synthetic import SyntheticCorpus
from .evaluate import build_harness, load_model, mark_clip
from .payload import MessageCodec
from .runlog import digest_file
from .seeding import resolve_device

SCHEMA = "genomark-n-parity-fixtures/1"
TOLERANCE = 1e-3


def write_fixtures(config, checkpoint: str, out_dir: str, items: int, channels: tuple[str, ...],
                   device: torch.device) -> dict:
    root = Path(out_dir)
    root.mkdir(parents=True, exist_ok=True)
    model = load_model(checkpoint, config, device)
    marker, _, codec_obj, bank = build_harness(config, model, device)
    codec = MessageCodec(config.payload)
    corpus = SyntheticCorpus(
        items=items, seconds=config.evaluation.clip_seconds,
        sample_rate=config.stft.sample_rate, base_seed=config.run.seed + 991,
    )
    rng = np.random.default_rng(config.run.seed + 4242)
    samples = int(config.evaluation.clip_seconds * config.stft.sample_rate)

    inputs: list[np.ndarray] = []
    presence: list[np.ndarray] = []
    bit_logits: list[np.ndarray] = []
    cases: list[dict] = []
    for name in channels:
        channel = bank.build(name)
        for index in range(items):
            cover = torch.from_numpy(corpus.load(index)[:samples]).unsqueeze(0).to(device)
            bits = codec.encode(codec.random_message(rng))
            marked = mark_clip(marker, cover, torch.from_numpy(bits).float().unsqueeze(0).to(device))
            degraded = channel.apply(marked, seed=7_000 + index)
            with torch.no_grad():
                log_magnitude = marker.front.band_log_magnitude(marker.front.stft(degraded))
                p, b, _ = model.decoder(log_magnitude)
            inputs.append(log_magnitude.squeeze(0).squeeze(0).cpu().numpy().astype(np.float32))
            presence.append(p.squeeze(0).cpu().numpy().astype(np.float32))
            bit_logits.append(b.squeeze(0).cpu().numpy().astype(np.float32))
            cases.append({
                "channel": name,
                "item": index,
                "content_class": corpus.content_class(index),
                "frames": int(log_magnitude.shape[-1]),
            })

    files = {}
    for label, arrays in (("log_magnitude", inputs), ("presence_logit", presence),
                          ("bit_logit", bit_logits)):
        path = root / f"{label}.f32"
        with path.open("wb") as handle:
            for array in arrays:
                handle.write(np.ascontiguousarray(array, dtype="<f4").tobytes())
        files[label] = {
            "file": path.name,
            "layout": "concatenated row-major f32 little-endian, one case after another",
            "sha256": digest_file(path),
            "bytes": path.stat().st_size,
        }

    manifest = {
        "schema": SCHEMA,
        "algorithm_id": "genomark-neural-v1",
        "bench_row": "N-B12",
        "tolerance_max_abs_error": TOLERANCE,
        "band_bins": config.stft.band_bins,
        "message_bits": config.payload.message_bits,
        "cases": cases,
        "files": files,
        "shapes": {
            "log_magnitude": ["band_bins", "frames"],
            "presence_logit": ["frames"],
            "bit_logit": ["message_bits"],
        },
        "how_to_use": (
            "Read case i's log_magnitude block (band_bins * frames f32), feed it to the Rust "
            "decoder, and compare against the presence_logit (frames f32) and bit_logit "
            "(message_bits f32) blocks at the same case index. The gate is max absolute error "
            f"< {TOLERANCE} on both, and 100% agreement on the accept/reject decision."
        ),
    }
    (root / "parity_manifest.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    return manifest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Write N-B12 cross-language parity fixtures.")
    parser.add_argument("--config", required=True)
    parser.add_argument("--checkpoint", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--items", type=int, default=None)
    parser.add_argument("--channels", default=None, help="comma-separated; defaults to the eval set")
    parser.add_argument("--device", default=None)
    args = parser.parse_args(argv)

    overrides = {"run": {"device": args.device}} if args.device else None
    config = load_config(args.config, overrides)
    device = resolve_device(config.run.device)
    channels = tuple(args.channels.split(",")) if args.channels else config.evaluation.channels
    manifest = write_fixtures(
        config, args.checkpoint, args.out, args.items or config.evaluation.corpus_items,
        channels, device,
    )
    print(f"{len(manifest['cases'])} cases over {len(channels)} channels")
    for label, info in manifest["files"].items():
        print(f"{label:16s} {info['bytes']:>12d} bytes  sha256 {info['sha256'][:16]}...")
    print(f"manifest {Path(args.out) / 'parity_manifest.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
