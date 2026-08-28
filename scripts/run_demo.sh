#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
DEMO_ROOT="${1:-${PROJECT_ROOT}/demo-output}"
SOURCE_CATEGORY="${2:-unknown}"
PROJECT_PATH="${3:-}"
PORT="${APW_UDP_PORT:-9876}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
SESSION_ID="capture-${STAMP}-$$"
SESSION_DIR="${DEMO_ROOT}/sessions/${SESSION_ID}"

mkdir -p "${SESSION_DIR}"
cd "${PROJECT_ROOT}"
env python3 -m daemon.preflight "${SESSION_DIR}" --port "${PORT}"
printf '%s\n' "${SESSION_DIR}" > "${DEMO_ROOT}/latest-session.txt"

DAEMON_ARGS=(
    --port "${PORT}"
    --session-id "${SESSION_ID}"
    --stem-id "stem-${SESSION_ID##*-}"
    --evidence-dir "${SESSION_DIR}/evidence"
    --sample-dir "${SESSION_DIR}/samples"
    --export-dir "${SESSION_DIR}/exports"
    --manifest-dir "${SESSION_DIR}/manifests"
    --source-category "${SOURCE_CATEGORY}"
    --open-artifacts
)

if [[ -n "${PROJECT_PATH}" ]]; then
    DAEMON_ARGS+=(--project "${PROJECT_PATH}")
fi

echo
echo "Routed Audio Evidence Adapter"
echo "  Session:   ${SESSION_ID}"
echo "  Export to: ${SESSION_DIR}/exports"
echo "  Dashboard: ${SESSION_DIR}/dashboard.html"
echo "  Boundary:  routed observations only; identity, rights, consent, and bypassed paths remain unestablished"
echo

env python3 -m daemon "${DAEMON_ARGS[@]}" &
DAEMON_PID=$!
cleanup() {
    trap - INT TERM EXIT
    if kill -0 "${DAEMON_PID}" 2>/dev/null; then
        kill -TERM "${DAEMON_PID}" 2>/dev/null || true
        wait "${DAEMON_PID}" 2>/dev/null || true
    fi
}
trap cleanup INT TERM HUP EXIT
for _ in {1..50}; do
    [[ -f "${SESSION_DIR}/dashboard.html" ]] && break
    sleep 0.1
done
open "${SESSION_DIR}/dashboard.html"
open "${SESSION_DIR}/exports"
wait "${DAEMON_PID}"
trap - INT TERM HUP EXIT
