"""ONNX export with the STFT OUTSIDE the graph (spec 7.2).

The exported graphs are pure convolution over log-magnitudes. Everything else -- windowing, FFT,
band slicing, the log, the perceptual budget, applying the gain to the complex bins, and WOLA
resynthesis -- belongs to the Rust side, which already owns realfft, rubato and the square-root
Hann WOLA path Watermark-Q uses. The consequence is that the model file contains no control flow, no
dynamic shape beyond the frame count, and nothing that can differ between runtimes except
arithmetic.

`budget_tables.json` ships the constants the Rust needs to reproduce the perceptual budget bit for
bit. The budget is the fidelity guarantee; leaving it out of the graph is deliberate, because a
fidelity bound buried in opaque weights cannot be audited.

Export SUCCEEDING is not export being CORRECT: every run here loads the result back under
onnxruntime and prints the measured max absolute deviation from PyTorch.
"""

import argparse
import json
import time
from pathlib import Path

import numpy as np
import torch
from safetensors.torch import save_file

from .config import Config, load_config
from .model import NeuralWatermark
from .perceptual import PerceptualModel, bark, spreading_db
from .runlog import digest_file
from .seeding import resolve_device

CONTRACT_VERSION = "apw-watermark-neural-onnx-contract/1"
DEFAULT_OPSET = 17


def contract(config: Config) -> dict:
    stft = config.stft
    return {
        "contract": CONTRACT_VERSION,
        "algorithm_id": "apw-watermark-neural-v1",
        "sample_rate": stft.sample_rate,
        "stft": {
            "n_fft": stft.n_fft,
            "hop": stft.hop,
            "window": "sqrt_hann_periodic",
            "synthesis": "wola",
            "lead_pad_samples": stft.n_fft,
            "frame_count": "ceil((n_fft + samples) / hop)",
            "location": "outside_the_graph",
        },
        "band": {
            "first_bin": stft.band_first_bin,
            "last_bin_inclusive": stft.band_last_bin,
            "bins": stft.band_bins,
            "bin_hz": stft.bin_hz,
            "low_hz": stft.band_low_hz,
            "high_hz": stft.band_high_hz,
        },
        "frames_per_second": stft.frames_per_second,
        "input_transform": "log(max(|X[band, t]|, 1e-7))",
        "graphs": {
            "encoder": {
                "file": "encoder.onnx",
                "inputs": [
                    {"name": "log_magnitude", "shape": [1, 1, stft.band_bins, "frames"], "dtype": "float32"},
                    {"name": "message_bits", "shape": [1, config.payload.message_bits],
                     "dtype": "float32", "domain": "0.0 or 1.0"},
                ],
                "outputs": [
                    {"name": "log_gain_raw", "shape": [1, 1, stft.band_bins, "frames"],
                     "dtype": "float32", "range": [-1.0, 1.0]}
                ],
                "apply": (
                    "g[t,f] = log_gain_raw[t,f] * B[t,f]; |X'| = |X| * exp(g); phase unchanged; "
                    "bins outside the band are returned bit-identical."
                ),
            },
            "decoder": {
                "file": "decoder.onnx",
                "inputs": [
                    {"name": "log_magnitude", "shape": [1, 1, stft.band_bins, "frames"], "dtype": "float32"}
                ],
                "outputs": [
                    {"name": "presence_logit", "shape": [1, "frames"], "dtype": "float32"},
                    {"name": "bit_logit", "shape": [1, config.payload.message_bits], "dtype": "float32"},
                    {"name": "bit_trace", "shape": [1, config.payload.message_bits, "frames"],
                     "dtype": "float32"},
                ],
                "note": (
                    "bit_logit is the PRE-tanh pooled difference (mean a+ - mean a-)/tau. Spec 2.6(b) "
                    "writes tanh(.) of this; tanh is strictly increasing so the sign and the |.| "
                    "ordering the flip search uses are identical, while an unbounded logit avoids the "
                    "BCE saturation a bounded one would impose. Apply tanh if a soft bit in (-1,1) is "
                    "wanted for display."
                ),
            },
        },
        "budget": {
            "file": "budget_tables.json",
            "formula": "B[t,f] = clamp(kappa * 10^((M[t,f] - L[t,f]) / 20), 0, b_max_nepers)",
            "kappa": config.budget.kappa,
            "b_max_nepers": config.budget.b_max_nepers,
            "note": (
                "Computed on the COVER alone, in Rust, not in the graph. Spec 2.9 restricts the graph "
                "op set to Conv2d / Upsample / GroupNorm / GELU / Linear / tanh / sigmoid / mean / "
                "concat, which the budget's log and power terms are not in, and a fidelity bound "
                "should be auditable rather than baked into weights."
            ),
        },
        "detector": {
            "window_seconds": config.detector.window_seconds,
            "window_hop_seconds": config.detector.window_hop_seconds,
            "presence_window_seconds": config.detector.presence_window_seconds,
            "presence_smooth_seconds": config.detector.presence_smooth_seconds,
            "presence_frame_threshold": config.detector.presence_frame_threshold,
            "min_presence_span_seconds": config.detector.min_presence_span_seconds,
            "max_windows": config.detector.max_windows,
            "crc_trials_per_window": 11,
            "flip_search": "ordered-statistics, k=2 over the 4 least-confident bits, 11 fixed patterns",
            "blindness": (
                "The detector receives audio and a frozen threshold record only. No offset search, no "
                "window chosen by agreement with a known payload."
            ),
        },
        "payload": {
            "message_bits": config.payload.message_bits,
            "layout": [
                {"bits": "0..2", "field": "version", "value": config.payload.version_value},
                {"bits": "3..6", "field": "namespace"},
                {"bits": "7..31", "field": "locator_prefix",
                 "note": "leading 25 bits of the same SHA-256 Watermark-Q's 48-bit locator prefixes"},
                {"bits": "32..55", "field": "crc24",
                 "poly": hex(config.payload.crc_poly), "init": "0xB704CE", "over": "bits 0..31"},
            ],
        },
    }


def budget_tables(config: Config) -> dict:
    """The Schroeder / half-Bark constants on the MODEL's grid, so Rust reproduces B exactly."""
    perceptual = PerceptualModel(config.perceptual, config.stft.sample_rate)
    stft = config.stft
    index = torch.arange(stft.band_first_bin, stft.band_last_bin + 1, dtype=torch.float64)
    frequency = index * stft.sample_rate / stft.n_fft
    width = config.perceptual.bark_partition_width
    band_of_bin = torch.floor(bark(frequency) / width).clamp_min(0.0).long()
    band_count = int(band_of_bin.max().item()) + 1
    centres = torch.arange(band_count, dtype=torch.float64) * width + 0.25
    delta = centres.unsqueeze(1) - centres.unsqueeze(0)
    spread = torch.pow(10.0, spreading_db(delta) / 10.0)
    alpha = config.perceptual.tonality_alpha
    offset_db = alpha * (14.5 + centres) + (1.0 - alpha) * 5.5
    return {
        "grid": {
            "sample_rate": stft.sample_rate,
            "n_fft": stft.n_fft,
            "band_first_bin": stft.band_first_bin,
            "band_last_bin_inclusive": stft.band_last_bin,
            "bins": stft.band_bins,
        },
        "bark_partition_width": width,
        "tonality_alpha": alpha,
        "full_scale_spl_db": config.perceptual.full_scale_spl_db,
        "band_count": band_count,
        "band_of_bin": band_of_bin.tolist(),
        "band_centres_bark": centres.tolist(),
        "spreading_matrix_linear": spread.tolist(),
        "masking_offset_linear": torch.pow(10.0, offset_db / 10.0).tolist(),
        "kappa": config.budget.kappa,
        "b_max_nepers": config.budget.b_max_nepers,
        "source": "crates/audio-provenance-bench/src/perceptual.rs, ported in /apw-watermark-neural/perceptual.py",
        "perceptual_model_bands": perceptual.band_count,
    }


class _EncoderGraph(torch.nn.Module):
    def __init__(self, model: NeuralWatermark) -> None:
        super().__init__()
        self.encoder = model.encoder

    def forward(self, log_magnitude, message_bits):
        return self.encoder(log_magnitude, message_bits)

    ARG_NAMES = ("log_magnitude", "message_bits")


class _DecoderGraph(torch.nn.Module):
    def __init__(self, model: NeuralWatermark) -> None:
        super().__init__()
        self.decoder = model.decoder

    def forward(self, log_magnitude):
        return self.decoder(log_magnitude)

    ARG_NAMES = ("log_magnitude",)


def _export(module: torch.nn.Module, args: tuple, path: Path, input_names, output_names,
            dynamic_axes: dict, dynamic_shapes: dict, opset: int) -> tuple[int, str]:
    """Prefer the torch.export-based exporter; fall back to the legacy TorchScript one.

    Both are tried because the dynamo path is the supported one from PyTorch 2.9 and the legacy one
    is deprecated, but the fallback keeps the harness working on a toolchain where the dynamo path
    trips over an op. Which one produced the file is recorded in the contract, because the two
    emit different node names and a parity investigation needs to know which it is looking at.
    """
    import contextlib
    import io
    import onnx

    noise = io.StringIO()
    try:
        # The onnxscript down-converter prints a traceback to stdout when it cannot reach the
        # requested opset and then falls back; it is not an exception and cannot be caught. The
        # outcome that matters is the opset actually emitted, which is recorded in the contract.
        with contextlib.redirect_stdout(noise):
            torch.onnx.export(
                module, args, str(path), input_names=list(input_names),
                output_names=list(output_names), dynamic_shapes=dynamic_shapes,
                opset_version=opset, dynamo=True, external_data=False,
            )
        return int(onnx.load(str(path)).opset_import[0].version), "dynamo"
    except Exception as error:  # noqa: BLE001 - the fallback is the point
        print(f"dynamo export failed ({type(error).__name__}: {str(error)[:160]}); "
              "falling back to the legacy exporter")
    torch.onnx.export(
        module, args, str(path), input_names=list(input_names), output_names=list(output_names),
        dynamic_axes=dynamic_axes, opset_version=opset, do_constant_folding=True, dynamo=False,
    )
    return int(onnx.load(str(path)).opset_import[0].version), "torchscript"


def _verify(path: Path, feeds: dict, expected: list[np.ndarray]) -> list[float]:
    import onnxruntime

    session = onnxruntime.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    outputs = session.run(None, feeds)
    return [float(np.abs(a - b).max()) for a, b in zip(outputs, expected, strict=True)]


def export(config: Config, checkpoint: str, out_dir: str, opset: int = DEFAULT_OPSET,
           frames: int = 128, use_ema: bool = True) -> dict:
    device = torch.device("cpu")
    payload = torch.load(checkpoint, map_location="cpu", weights_only=False)
    model = NeuralWatermark(config.encoder, config.decoder, config.payload, config.stft)
    state = payload.get("ema") if use_ema and payload.get("ema") else payload["model"]
    model.load_state_dict(state)
    model.eval().to(device)

    root = Path(out_dir)
    root.mkdir(parents=True, exist_ok=True)

    # REQUIRED: the parity errors below ship in the ONNX contract as measured evidence, so the
    # probes they are measured on must be reproducible from (config, seed) like every other artifact.
    probe = torch.Generator(device="cpu").manual_seed(config.run.seed)
    log_magnitude = torch.randn(1, 1, config.stft.band_bins, frames, generator=probe)
    bits = torch.randint(0, 2, (1, config.payload.message_bits), generator=probe).float()

    encoder_path = root / "encoder.onnx"
    decoder_path = root / "decoder.onnx"
    frame_dim = torch.export.Dim("frames", min=8, max=1 << 16)
    encoder_opset, encoder_exporter = _export(
        _EncoderGraph(model).eval(), (log_magnitude, bits), encoder_path,
        ["log_magnitude", "message_bits"], ["log_gain_raw"],
        {"log_magnitude": {3: "frames"}, "log_gain_raw": {3: "frames"}},
        {"log_magnitude": {3: frame_dim}, "message_bits": None}, opset,
    )
    decoder_opset, decoder_exporter = _export(
        _DecoderGraph(model).eval(), (log_magnitude,), decoder_path,
        ["log_magnitude"], ["presence_logit", "bit_logit", "bit_trace"],
        {"log_magnitude": {3: "frames"}, "presence_logit": {1: "frames"}, "bit_trace": {2: "frames"}},
        {"log_magnitude": {3: frame_dim}}, opset,
    )

    with torch.no_grad():
        expected_encoder = [model.encoder(log_magnitude, bits).numpy()]
        expected_decoder = [t.numpy() for t in model.decoder(log_magnitude)]
    encoder_error = _verify(
        encoder_path, {"log_magnitude": log_magnitude.numpy(), "message_bits": bits.numpy()},
        expected_encoder,
    )
    decoder_error = _verify(decoder_path, {"log_magnitude": log_magnitude.numpy()}, expected_decoder)

    other = frames + 37
    other_input = torch.randn(1, 1, config.stft.band_bins, other, generator=probe)
    with torch.no_grad():
        expected_other = [t.numpy() for t in model.decoder(other_input)]
    dynamic_error = _verify(decoder_path, {"log_magnitude": other_input.numpy()}, expected_other)

    weights_path = root / "apw-watermark-neural-v1.safetensors"
    save_file({name: tensor.contiguous() for name, tensor in model.state_dict().items()},
              str(weights_path))

    (root / "budget_tables.json").write_text(json.dumps(budget_tables(config), indent=2), encoding="utf-8")
    contract_record = contract(config)
    contract_record["exported_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    opset_record: dict[str, int | str] = {
        "requested": opset, "encoder": encoder_opset, "decoder": decoder_opset,
    }
    contract_record["opset"] = opset_record
    contract_record["exporter"] = {"encoder": encoder_exporter, "decoder": decoder_exporter}
    if encoder_opset != opset or decoder_opset != opset:
        opset_record["deviation"] = (
            f"Spec 7.1 names opset {opset}. The exporter emitted encoder {encoder_opset} / decoder "
            f"{decoder_opset}: onnxscript's down-conversion of the ReduceMean axes-as-input form "
            "fails, so the graph is left at the version torch produced rather than silently "
            "shipping an unverified conversion. `ort` is an offline cross-check only (spec 7.1); "
            "the shipped runtime is candle reading the safetensors file, which carries no opset."
        )
    contract_record["artifacts"] = {
        path.name: {"sha256": digest_file(path), "bytes": path.stat().st_size}
        for path in sorted(root.iterdir())
        if path.is_file() and path.suffix in {".onnx", ".safetensors", ".data"} or path.name.endswith(".onnx.data")
    }
    contract_record["shipped_weights"] = {
        "file": weights_path.name,
        "sha256": digest_file(weights_path),
        "bytes": weights_path.stat().st_size,
        "note": (
            "Spec 7.5: this digest is what the Rust crate pins in source and verifies before "
            "deserialising. The model file is trusted code-equivalent; a swapped model is a forgery "
            "vector."
        ),
    }
    contract_record["parity"] = {
        "encoder_max_abs_error": encoder_error,
        "decoder_max_abs_error": decoder_error,
        "decoder_max_abs_error_other_frame_count": dynamic_error,
        "frames_tested": [frames, other],
        "note": "PyTorch fp32 against onnxruntime CPUExecutionProvider on the same input.",
    }
    (root / "onnx_contract.json").write_text(json.dumps(contract_record, indent=2), encoding="utf-8")
    return contract_record


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Export Watermark-N to ONNX, STFT outside the graph.")
    parser.add_argument("--config", required=True)
    parser.add_argument("--checkpoint", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--opset", type=int, default=DEFAULT_OPSET)
    parser.add_argument("--frames", type=int, default=128)
    args = parser.parse_args(argv)

    config = load_config(args.config)
    resolve_device("cpu")
    record = export(config, args.checkpoint, args.out, args.opset, args.frames)
    parity = record["parity"]
    print(f"opset requested {record['opset']['requested']} -> encoder {record['opset']['encoder']}, "
          f"decoder {record['opset']['decoder']}")
    print(f"encoder max abs error {max(parity['encoder_max_abs_error']):.3e}")
    print(f"decoder max abs error {max(parity['decoder_max_abs_error']):.3e}")
    print(f"decoder max abs error at a different frame count "
          f"{max(parity['decoder_max_abs_error_other_frame_count']):.3e}")
    for name, info in record["artifacts"].items():
        print(f"{name:32s} {info['bytes']:>10d} bytes  sha256 {info['sha256'][:16]}...")
    print(f"contract {Path(args.out) / 'onnx_contract.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
