"""Emit the `genomark-n-model-card/1` record that `crates/genomark-n` loads.

TWO CARDS, DELIBERATELY, BECAUSE THEY ANSWER DIFFERENT QUESTIONS. `model_card.py` writes the audit
artifact: the eval, the limits, the calibration-insufficiency warning, the honest-envelope block
full of explicit nulls. This writes the LOADER's card, which is a machine contract: the Rust
`ModelCard::validate` checks every transform field against its own compiled constants and refuses
the model rather than running it on a disagreement, and `read_graph` refuses a graph whose SHA-256
is not the one pinned here.

Nothing in this file may soften a gate. `operating_envelope` is always null, because the only thing
that may populate it is spec 12's Stage 2 gate on measured physical captures, and a null forces
`AcousticRerecording::Unsupported` on the Rust side by construction.
"""

import argparse
import json
from pathlib import Path

from .config import Config, load_config
from .runlog import digest_file

CARD_FORMAT = "genomark-n-model-card/1"
ALGORITHM_ID = "genomark-neural-v1"


def thresholds_block(config: Config, presence_accept_score: float) -> dict:
    """The detector's frozen decision boundaries, in the loader's field names.

    IMPORTANT: these are THIS RUN's operating point, not spec 4.5's. Writing the spec's 30 s locator
    window beside a presence threshold calibrated over 1.5 s windows would mix two operating points
    into one record, and the score is a maximum over windows so the mix is not conservative.
    """
    detector = config.detector
    return {
        "presence_accept_score": presence_accept_score,
        "presence_frame_gate": detector.presence_frame_threshold,
        "frame_gate_median_seconds": detector.presence_smooth_seconds,
        "presence_window_seconds": detector.presence_window_seconds,
        "presence_window_hop_seconds": detector.presence_window_seconds / 2.0,
        "min_presence_seconds": detector.presence_window_seconds,
        "locator_window_seconds": detector.window_seconds,
        "locator_window_hop_seconds": detector.window_hop_seconds,
        "min_locator_seconds": detector.min_locator_seconds,
        "min_presence_span_seconds": detector.min_presence_span_seconds,
        "max_windows": detector.max_windows,
    }


def build(config: Config, export_dir: Path, contract: dict, thresholds: dict | None,
          model_id: str, epoch: int, steps: int, calibration_note: str) -> dict:
    stft = config.stft
    graphs = {
        "decoder": {"file": "decoder.onnx", "sha256": digest_file(export_dir / "decoder.onnx")},
        "encoder": {"file": "encoder.onnx", "sha256": digest_file(export_dir / "encoder.onnx")},
    }
    accept = float(thresholds["presence_threshold"]) if thresholds else 0.5
    if not 0.0 < accept < 1.0:
        # The pooled presence score is a mean of sigmoid(p[t]), so it lives in the open unit
        # interval and so must its threshold. `calibrate.choose_threshold` returns max(score) + eps
        # when no accept is affordable, which is a REFUSAL to certify an operating point, not an
        # operating point. Writing it into a loader card would ship a detector that can never fire.
        raise ValueError(
            f"presence_accept_score {accept!r} is not in (0, 1): the calibration did not produce a "
            "usable operating point, and a card must not paper over that"
        )
    return {
        "card_format": CARD_FORMAT,
        "algorithm_id": ALGORITHM_ID,
        "model_id": model_id,
        "epoch": epoch,
        "fixture": False,
        "graphs": graphs,
        "transform": {
            "sample_rate_hz": stft.sample_rate,
            "n_fft": stft.n_fft,
            "hop": stft.hop,
            "window": "sqrt_hann_periodic",
            "lead_pad_samples": stft.n_fft,
            "band_bin_low": stft.band_first_bin,
            "band_bin_high": stft.band_last_bin,
            "magnitude": "natural_log",
            "log_floor": 1e-7,
            "tensor_layout": "nchw_frequency_major",
        },
        "io": {
            "decoder_input": "log_magnitude",
            "decoder_presence_output": "presence_logit",
            "decoder_message_output": "bit_logit",
            "encoder_log_mag_input": "log_magnitude",
            "encoder_message_input": "message_bits",
            "encoder_output": "log_gain_raw",
        },
        "budget": {"kappa": config.budget.kappa, "max_nepers": config.budget.b_max_nepers},
        "thresholds": thresholds_block(config, accept),
        "training_provenance": {
            "status": "UNTRAINED_SMOKE_MODEL",
            "steps": steps,
            "thresholds": "UNCALIBRATED",
            "detects": "nothing. This model exists to prove the PyTorch -> ONNX -> Rust path loads "
                       "and runs. Any accept it produces is arithmetic, not a detection.",
            "threshold_note": calibration_note,
            "onnx_contract": contract.get("contract"),
            "opset": contract.get("opset"),
            "parity": contract.get("parity"),
            "shipped_weights": contract.get("shipped_weights"),
            "curriculum": {
                "phase_a_end": config.curriculum.phase_a_end,
                "phase_b_end": config.curriculum.phase_b_end,
                "phase_c_end": config.curriculum.phase_c_end,
            },
        },
        "operating_envelope": None,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Write the genomark-n-model-card/1 record the Rust crate loads."
    )
    parser.add_argument("--config", required=True)
    parser.add_argument("--export", required=True, help="directory holding encoder.onnx/decoder.onnx")
    parser.add_argument("--thresholds", default=None)
    parser.add_argument("--model-id", default="genomark-n-smoke")
    parser.add_argument("--epoch", type=int, default=0)
    parser.add_argument("--steps", type=int, default=0)
    parser.add_argument("--out", default=None)
    args = parser.parse_args(argv)

    config = load_config(args.config)
    export_dir = Path(args.export)
    contract = json.loads((export_dir / "onnx_contract.json").read_text(encoding="utf-8"))

    thresholds = None
    note = "no calibration artifact was supplied; the accept score is a placeholder 0.5"
    if args.thresholds:
        thresholds = json.loads(Path(args.thresholds).read_text(encoding="utf-8"))
        note = (
            f"calibrated over {thresholds['calibration_trials']} unmarked trials against a target "
            f"false-positive rate of {thresholds['target_false_positive_rate']:g}. Spec 3.5 requires "
            "at least 5000; below that the threshold does not establish the rate and the bound is "
            "3/N with N stated."
        )

    card = build(config, export_dir, contract, thresholds, args.model_id, args.epoch,
                 args.steps, note)
    out = Path(args.out) if args.out else export_dir / f"{args.model_id}.card.json"
    out.write_text(json.dumps(card, indent=2) + "\n", encoding="utf-8")
    print(f"card {out}")
    print(f"decoder sha256 {card['graphs']['decoder']['sha256']}")
    print(f"encoder sha256 {card['graphs']['encoder']['sha256']}")
    print(f"presence_accept_score {card['thresholds']['presence_accept_score']:.6f} ({note})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
