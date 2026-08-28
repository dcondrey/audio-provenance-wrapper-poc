#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
OUTPUT_ROOT="${1:-${PROJECT_ROOT}/demo-output/presenter-fallback}"

cd "${PROJECT_ROOT}"
exec env python3 "${SCRIPT_DIR}/synthetic_rehearsal.py" --output "${OUTPUT_ROOT}" --open
