#!/bin/bash
# End-to-end smoke: train a handful of steps on synthesized audio, freeze thresholds on unmarked
# audio, evaluate blind, export ONNX and verify it under onnxruntime, then write the model card.
# It proves every stage RUNS. It proves nothing about whether the mark works.
set -euo pipefail
cd "$(dirname "$0")/.."

CONFIG=configs/smoke.yaml
RUN=runs/smoke
rm -rf "$RUN"

echo "=== environment ==="
uv run python -c "
import torch, platform
print('torch', torch.__version__, '| mps available', torch.backends.mps.is_available(),
      '| cuda', torch.cuda.is_available())
print(platform.platform(), platform.machine())
"

echo
echo "=== 1/6 train ==="
uv run python -m apw_watermark_neural.train --config "$CONFIG"

echo
echo "=== 2/6 calibrate thresholds on UNMARKED audio only ==="
uv run python -m apw_watermark_neural.calibrate --config "$CONFIG" \
  --checkpoint "$RUN/checkpoints/latest.pt" --out "$RUN/thresholds.json"

echo
echo "=== 3/6 blind evaluation ==="
uv run python -m apw_watermark_neural.evaluate --config "$CONFIG" \
  --checkpoint "$RUN/checkpoints/latest.pt" --thresholds "$RUN/thresholds.json" \
  --out "$RUN/eval.json"

echo
echo "=== 4/6 ONNX export and numeric round trip ==="
uv run python -m apw_watermark_neural.export_onnx --config "$CONFIG" \
  --checkpoint "$RUN/checkpoints/latest.pt" --out "$RUN/export" 2>/dev/null

echo
echo "=== 5/6 N-B12 cross-language parity fixtures ==="
uv run python -m apw_watermark_neural.parity --config "$CONFIG" \
  --checkpoint "$RUN/checkpoints/latest.pt" --out "$RUN/parity"

echo
echo "=== 6/6 model card ==="
uv run python -m apw_watermark_neural.model_card --config "$CONFIG" --run-dir "$RUN" \
  --contract "$RUN/export/onnx_contract.json" --thresholds "$RUN/thresholds.json" \
  --eval "$RUN/eval.json" --out "$RUN/model_card.json"

echo
echo "=== artifacts ==="
find "$RUN" -type f | sort
