#!/bin/bash
# Phase A's precondition in three minutes. One clip, one payload, distortion chain BYPASSED.
# A failure here is a wiring bug in encode -> encoder -> gain -> ISTFT -> STFT -> decoder -> decode,
# not a statement about whether the mark survives a room. Passing does not mean the design works.
set -euo pipefail
cd "$(dirname "$0")/.."
uv run python -m genomark_n.overfit_check \
  --config configs/smoke.yaml --steps "${1:-400}" --lr 1e-3 --seconds 2.0 \
  --out runs/overfit_check.json
