#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TARGET="${1:-${PROJECT_ROOT}/demo-output/preflight}"
cd "${PROJECT_ROOT}"
exec env python3 -m daemon.preflight "${TARGET}" --port "${APW_UDP_PORT:-9876}"
