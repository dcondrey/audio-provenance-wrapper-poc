#!/bin/bash
# PyTorch -> ONNX -> Rust, end to end, over a checkpoint that already exists.
#
# Calibrates thresholds on UNMARKED audio, evaluates blind, exports the graphs, writes BOTH cards
# (the audit card and the loader card), then hands the loader card to `crates/apw-watermark-neural` and runs
# its detector. `DetectionClass::None` from an untrained model is the CORRECT result; the point is
# that the card validated, both graph digests matched, both ONNX sessions built with the declared
# shapes, and the decoder ran.
#
#   scripts/loop_closure.sh configs/smoke_300.yaml runs/smoke300 300
set -euo pipefail
cd "$(dirname "$0")/.."

CONFIG=${1:-configs/smoke_300.yaml}
RUN=${2:-runs/smoke300}
STEPS=${3:-0}
CHECKPOINT="$RUN/checkpoints/latest.pt"
SDK=$(cd .. && pwd)

echo "=== 1/6 calibrate thresholds on UNMARKED audio only ==="
uv run python -m apw_watermark_neural.calibrate --config "$CONFIG" \
  --checkpoint "$CHECKPOINT" --out "$RUN/thresholds.json"

echo
echo "=== 2/6 blind evaluation ==="
uv run python -m apw_watermark_neural.evaluate --config "$CONFIG" \
  --checkpoint "$CHECKPOINT" --thresholds "$RUN/thresholds.json" --out "$RUN/eval.json"

echo
echo "=== 3/6 ONNX export and numeric round trip ==="
uv run python -m apw_watermark_neural.export_onnx --config "$CONFIG" \
  --checkpoint "$CHECKPOINT" --out "$RUN/export"

echo
echo "=== 4/6 audit model card ==="
uv run python -m apw_watermark_neural.model_card --config "$CONFIG" --run-dir "$RUN" \
  --contract "$RUN/export/onnx_contract.json" --thresholds "$RUN/thresholds.json" \
  --eval "$RUN/eval.json" --out "$RUN/model_card.json"

echo
echo "=== 5/6 loader model card (apw-watermark-neural-model-card/1) ==="
uv run python -m apw_watermark_neural.rust_card --config "$CONFIG" --export "$RUN/export" \
  --thresholds "$RUN/thresholds.json" --model-id apw-watermark-neural-smoke --steps "$STEPS"

echo
echo "=== 6/6 Rust load and run ==="
CARD="$(cd "$RUN/export" && pwd)/apw-watermark-neural-smoke.card.json"
CARGO_TARGET_DIR=/Volumes/C/rust-target cargo run --quiet --manifest-path "$SDK/Cargo.toml" \
  -p apw-watermark-neural --example run_card -- "$CARD" 12
