#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
DEMO_ROOT="${1:-${PROJECT_ROOT}/demo-output}"
SOURCE_CATEGORY="${2:-unknown}"
PROJECT_PATH="${3:-}"

EVIDENCE_DIR="${DEMO_ROOT}/evidence"
SAMPLE_DIR="${DEMO_ROOT}/samples"
EXPORT_DIR="${DEMO_ROOT}/exports"
MANIFEST_DIR="${DEMO_ROOT}/manifests"

mkdir -p "${EVIDENCE_DIR}" "${SAMPLE_DIR}" "${EXPORT_DIR}" "${MANIFEST_DIR}"

echo "Audio Provenance Capture demo"
echo "  Export WAV/AIFF to: ${EXPORT_DIR}"
echo "  Fight cards appear in: ${MANIFEST_DIR}"
echo "  Source category: ${SOURCE_CATEGORY} (producer-declared unless unknown)"
echo
echo "Start Ableton, insert Audio Provenance Capture on one track, play audio,"
echo "then export a new or overwritten WAV/AIFF into the export folder above."
echo

DAEMON_ARGS=(
    --evidence-dir "${EVIDENCE_DIR}"
    --sample-dir "${SAMPLE_DIR}"
    --export-dir "${EXPORT_DIR}"
    --manifest-dir "${MANIFEST_DIR}"
    --source-category "${SOURCE_CATEGORY}"
)

if [[ -n "${PROJECT_PATH}" ]]; then
    DAEMON_ARGS+=(--project "${PROJECT_PATH}")
fi

cd "${PROJECT_ROOT}"
exec python3 -m daemon "${DAEMON_ARGS[@]}"
