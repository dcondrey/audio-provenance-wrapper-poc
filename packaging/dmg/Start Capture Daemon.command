#!/bin/bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DAEMON_DIR_NAME="apw-daemon"
PORT="${APW_UDP_PORT:-9876}"
SESSION_ROOT="${APW_SESSION_ROOT:-${HOME}/Documents/Audio Provenance Capture}"
SHIPPED_STORE="${HERE}/Documentation and Demo Project/demo-provenance-store"
LIVE_STORE="${HOME}/.apw/demo-provenance-store"

die() { echo "ERROR: $*" >&2; echo; echo "Press return to close."; read -r _; exit 1; }

find_daemon() {
    local bundle
    for bundle in \
        "${HOME}/Library/Audio/Plug-Ins/VST3/Audio Provenance Capture.vst3" \
        "/Library/Audio/Plug-Ins/VST3/Audio Provenance Capture.vst3" \
        "${HOME}/Library/Audio/Plug-Ins/Components/Audio Provenance Capture.component" \
        "/Library/Audio/Plug-Ins/Components/Audio Provenance Capture.component" \
        "${HERE}/Audio Provenance Capture.vst3"
    do
        if [[ -x "${bundle}/Contents/Resources/${DAEMON_DIR_NAME}/${DAEMON_DIR_NAME}" ]]; then
            printf '%s\n' "${bundle}/Contents/Resources/${DAEMON_DIR_NAME}/${DAEMON_DIR_NAME}"
            return 0
        fi
    done
    return 1
}

DAEMON="$(find_daemon)" \
    || die "the capture daemon was not found. Run 'Install Plug-Ins.command' on this disk image first."

if lsof -nP -iUDP:"${PORT}" >/dev/null 2>&1; then
    die "UDP port ${PORT} is already in use. Another capture daemon is probably running."
fi

# REQUIRED: the daemon signs C2PA claims with material issued by this store, so it must
# be the store whose root ships as demo-root-ca.pem, or a recipient's own signatures
# chain to a root they never installed and read as untrusted. The disk image is
# read-only and the store issues leaves as it runs, so it is copied out once.
if [[ ! -d "${LIVE_STORE}" ]]; then
    [[ -d "${SHIPPED_STORE}" ]] \
        || die "the demo signing store is missing. Run this from the disk image, which carries 'Documentation and Demo Project/demo-provenance-store'."
    mkdir -p "$(dirname "${LIVE_STORE}")"
    ditto "${SHIPPED_STORE}" "${LIVE_STORE}" || die "could not copy the demo signing store to ${LIVE_STORE}"
    chmod -R u+w "${LIVE_STORE}"
    echo "Copied the demo signing store to ${LIVE_STORE}."
fi

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
SESSION_ID="capture-${STAMP}-$$"
SESSION_DIR="${SESSION_ROOT}/sessions/${SESSION_ID}"
mkdir -p "${SESSION_DIR}" || die "could not create ${SESSION_DIR}"

echo "Audio Provenance Capture — capture daemon"
echo "  Daemon:    ${DAEMON}"
echo "  Session:   ${SESSION_ID}"
echo "  Export to: ${SESSION_DIR}/exports"
echo "  Dashboard: ${SESSION_DIR}/dashboard.html"
echo "  Anchor:    'Documentation and Demo Project/demo-root-ca.pem' verifies what this session signs"
echo "  Boundary:  routed observations only; identity, rights, consent, and bypassed paths remain unestablished"
echo
echo "Leave this window open for the whole session. Close it or press Ctrl-C to stop."
echo

"${DAEMON}" \
    --port "${PORT}" \
    --session-id "${SESSION_ID}" \
    --stem-id "stem-${SESSION_ID##*-}" \
    --evidence-dir "${SESSION_DIR}/evidence" \
    --sample-dir "${SESSION_DIR}/samples" \
    --export-dir "${SESSION_DIR}/exports" \
    --manifest-dir "${SESSION_DIR}/manifests" \
    --provenance-store "${LIVE_STORE}" \
    --open-artifacts &
DAEMON_PID=$!

cleanup() {
    trap - INT TERM EXIT
    if kill -0 "${DAEMON_PID}" 2>/dev/null; then
        kill -TERM "${DAEMON_PID}" 2>/dev/null || true
        wait "${DAEMON_PID}" 2>/dev/null || true
    fi
}
trap cleanup INT TERM HUP EXIT

for _ in $(seq 1 100); do
    [[ -f "${SESSION_DIR}/dashboard.html" ]] && break
    sleep 0.1
done
[[ -f "${SESSION_DIR}/dashboard.html" ]] && open "${SESSION_DIR}/dashboard.html"
mkdir -p "${SESSION_DIR}/exports" && open "${SESSION_DIR}/exports"

wait "${DAEMON_PID}"
trap - INT TERM HUP EXIT
