"""The model card the Rust crate loads.

Spec 8.6 / 11 / 12.4 constrain what may appear here. Two rules are enforced in code rather than
left to the writer's discipline:

  1. THE MEASURED ENVELOPE IS null UNTIL IT IS MEASURED. Every physical field is emitted as an
     explicit null under status "unmeasured". A model card carrying plausible-looking distance and
     detection-rate numbers that nobody measured is the exact artifact this spec was written
     against, and a default value is indistinguishable from a measurement once it is in a JSON file.
  2. A RATE NEVER TRAVELS WITHOUT ITS TRIAL COUNT. Any false-positive figure carries the trials that
     bound it and the 3/N bound that count implies.

`acousticRerecording` stays the literal "unsupported" here, because only section 12's Stage 2 gate
on measured physical captures can change it, and this file is not that gate.
"""

import argparse
import json
import time
from pathlib import Path

from .config import ALGORITHM_ID, SPEC, Config, load_config
from .runlog import digest_file, digest_text

SCHEMA = "genomark-n-model-card/1"

UNMEASURED_ENVELOPE = {
    "status": "unmeasured",
    "maxDistanceM": None,
    "minDurationS": None,
    "roomsMeasured": 0,
    "speakersMeasured": 0,
    "microphonesMeasured": 0,
    "blindDetectionRate": None,
    "falsePositiveRate": None,
    "falsePositiveTrials": 0,
    "note": (
        "No physical speaker-to-microphone capture has been run. Spec 10's Stage 0 campaign and "
        "spec 12's Stage 2 gate are what populate these fields; until then every one of them is "
        "null and capabilities().acousticRerecording is the literal \"unsupported\"."
    ),
}

LIMITS = [
    "Simulated acoustic rows are RIR convolution plus a filter cascade plus Gaussian noise. They "
    "are not a speaker-to-microphone measurement and may not be reported as one.",
    "Mean bit accuracy is not a detection rate and is not reported as one.",
    "The presence tier has no CRC behind it; its false-positive rate is a purely empirical "
    "threshold property and is the weakest number in the design.",
    "A locator hit resolves a registry BUCKET, not an identity. At 2^24 registered works over 2^25 "
    "buckets, 39.3% of works share a bucket with at least one other.",
    "Neural codecs are a declared failure: RAW-Bench measures 0.00 full-message accuracy for every "
    "method tested and retraining does not fix it.",
    "Playback speed or pitch changes beyond +/- 0.5% will not decode; there is no rate grid.",
    "The mark is removable in the public namespace, exactly as GenoMark-Q's is, and more so: a "
    "learned encoder's residual is estimable by anyone holding the public model.",
]


def build_card(config: Config, run_dir: Path, contract_path: Path, thresholds_path: Path | None,
               eval_path: Path | None, parameter_report: dict) -> dict:
    contract = json.loads(contract_path.read_text(encoding="utf-8"))
    export_root = contract_path.parent
    run_record = json.loads((run_dir / "run.json").read_text(encoding="utf-8"))

    thresholds = None
    threshold_notes = None
    if thresholds_path and thresholds_path.exists():
        thresholds = json.loads(thresholds_path.read_text(encoding="utf-8"))
        notes_path = Path(str(thresholds_path) + ".notes.json")
        if notes_path.exists():
            threshold_notes = json.loads(notes_path.read_text(encoding="utf-8"))

    simulated = None
    if eval_path and eval_path.exists():
        report = json.loads(eval_path.read_text(encoding="utf-8"))
        simulated = {
            "schema": report["schema"],
            "generated_at": report["generated_at"],
            "clip_seconds": report["clip_seconds"],
            "corpus": report["corpus"],
            "channels": [
                {
                    "name": channel["name"],
                    "family": channel["family"],
                    "trials": channel["trials"],
                    "exact_recovery_rate": channel["exact_recovery_rate"],
                    "payload_returned_rate": channel["payload_returned_rate"],
                    "mean_bit_error_rate": channel["mean_bit_error_rate"],
                    "presence_recall": channel["presence_recall"],
                    "false_positive": channel["false_positive"],
                    "mean_detect_seconds": channel["mean_detect_seconds"],
                }
                for channel in report["channels"]
            ],
            "notes": report["notes"],
        }

    weights = contract["shipped_weights"]
    size_limit_bytes = 40 * 1024 * 1024
    return {
        "schema": SCHEMA,
        "algorithm_id": ALGORITHM_ID,
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "spec": SPEC,
        "status": "gated_feasibility_experiment",
        "capabilities": {
            "acousticRerecording": "unsupported",
            "note": (
                "Only spec 12's Stage 2 gate on measured physical captures may change this, and it "
                "becomes a structured record, never a boolean and never true."
            ),
        },
        "architecture": {
            "domain": "spectral_magnitude",
            "sample_rate": config.stft.sample_rate,
            "stft": contract["stft"],
            "band": contract["band"],
            "parameters": parameter_report,
            "fp32_bytes": parameter_report["fp32_bytes"],
            "k7_size_ceiling_bytes": size_limit_bytes,
            "k7_size_pass": parameter_report["fp32_bytes"] <= size_limit_bytes,
            "op_set": ["Conv2d", "Upsample(nearest)", "GroupNorm", "GELU", "Linear", "tanh",
                       "sigmoid", "mean", "concat"],
        },
        "payload": contract["payload"],
        "detector": contract["detector"],
        "budget": contract["budget"],
        "onnx": {
            "contract": contract["contract"],
            "opset": contract["opset"],
            "exporter": contract["exporter"],
            "parity": contract["parity"],
            "artifacts": contract["artifacts"],
        },
        "weights": {
            "file": weights["file"],
            "sha256": weights["sha256"],
            "bytes": weights["bytes"],
            "format": "safetensors",
            "pinning": weights["note"],
        },
        "training_provenance": {
            **run_record,
            "config_digest": digest_text(config.to_json()),
            "curriculum": {
                "phase_a_end": config.curriculum.phase_a_end,
                "phase_b_end": config.curriculum.phase_b_end,
                "phase_c_end": config.curriculum.phase_c_end,
            },
            "optimiser": {
                "lr": config.optim.lr,
                "batch_size": config.optim.batch_size,
                "total_steps": config.optim.total_steps,
                "ema_decay": config.optim.ema_decay,
            },
            "loss_weights": {
                "message": config.loss.message,
                "detection": config.loss.detection,
                "perceptual": config.loss.perceptual,
                "spectral": config.loss.spectral,
                "q_coexistence": config.loss.q_coexistence,
                "adversarial_enabled": config.loss.adversarial_enabled,
            },
            "audio_corpora": {
                "manifests": list(config.data.manifests),
                "licence_allowlist": ["CC0-1.0", "CC-BY-*", "CC-BY-SA-*", "audio-provenance-owned",
                                      "audio-provenance-licensed", "public-domain"],
                "excluded": ["unfiltered FMA", "OpenSLR-28 RIRS_NOISES", "Aachen AIR standalone"],
            },
            "impulse_responses": {
                "measured_corpus_dirs": list(config.rir.corpus_dirs),
                "synthesizer": "image_source, Eyring absorption, windowed-sinc fractional delay",
                "measured_files": 0 if not config.rir.corpus_dirs else None,
            },
        },
        "frozen_thresholds": thresholds,
        "threshold_calibration_notes": threshold_notes,
        "measured_envelope": UNMEASURED_ENVELOPE,
        "simulated_evaluation": simulated,
        "limits": LIMITS,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Write the GenoMark-N model card.")
    parser.add_argument("--config", required=True)
    parser.add_argument("--run-dir", required=True)
    parser.add_argument("--contract", required=True)
    parser.add_argument("--thresholds", default=None)
    parser.add_argument("--eval", dest="evaluation", default=None)
    parser.add_argument("--out", required=True)
    args = parser.parse_args(argv)

    config = load_config(args.config)
    from .model import GenoMarkN

    model = GenoMarkN(config.encoder, config.decoder, config.payload, config.stft)
    card = build_card(
        config,
        Path(args.run_dir),
        Path(args.contract),
        Path(args.thresholds) if args.thresholds else None,
        Path(args.evaluation) if args.evaluation else None,
        model.parameter_report(),
    )
    Path(args.out).write_text(json.dumps(card, indent=2), encoding="utf-8")
    card_digest = digest_file(args.out)
    print(f"model card {args.out} sha256 {card_digest[:16]}...")
    print(f"  algorithm {card['algorithm_id']} acousticRerecording "
          f"{card['capabilities']['acousticRerecording']!r}")
    print(f"  parameters {card['architecture']['parameters']['total']:,} "
          f"({card['architecture']['fp32_bytes'] / 1e6:.1f} MB fp32) K7 size gate "
          f"{'pass' if card['architecture']['k7_size_pass'] else 'FAIL'}")
    print(f"  measured envelope status {card['measured_envelope']['status']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
