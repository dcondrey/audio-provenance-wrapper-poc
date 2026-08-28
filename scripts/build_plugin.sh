#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_DIR="${APW_BUILD_DIR:-${PROJECT_ROOT}/build}"
BUILD_TYPE="${APW_BUILD_TYPE:-Release}"
MACOS_DEPLOYMENT_TARGET="${APW_MACOS_DEPLOYMENT_TARGET:-12.0}"

cmake -S "${PROJECT_ROOT}" -B "${BUILD_DIR}" \
    -DCMAKE_BUILD_TYPE="${BUILD_TYPE}" \
    -DCMAKE_OSX_DEPLOYMENT_TARGET="${MACOS_DEPLOYMENT_TARGET}"
cmake --build "${BUILD_DIR}" --target AudioProvenanceCapture_VST3 --config "${BUILD_TYPE}"

PLUGIN_BUNDLE="${BUILD_DIR}/AudioProvenanceCapture_artefacts/${BUILD_TYPE}/VST3/Audio Provenance Capture.vst3"
if [[ ! -d "${PLUGIN_BUNDLE}" ]]; then
    echo "Build completed, but the expected VST3 bundle was not found:" >&2
    echo "  ${PLUGIN_BUNDLE}" >&2
    exit 1
fi

# JUCE's VST3 manifest helper writes moduleinfo.json after the linker's initial
# ad-hoc signature. Re-sign the completed bundle so its resource seal includes
# that generated manifest. This is local ad-hoc signing, not distribution signing.
codesign --force --deep --sign - --timestamp=none "${PLUGIN_BUNDLE}"
codesign --verify --deep --strict "${PLUGIN_BUNDLE}"
echo "VST3 ready: ${PLUGIN_BUNDLE}"

if [[ "${1:-}" == "--install" ]]; then
    INSTALL_DIR="${HOME}/Library/Audio/Plug-Ins/VST3"
    DESTINATION="${INSTALL_DIR}/Audio Provenance Capture.vst3"
    mkdir -p "${INSTALL_DIR}"
    ditto "${PLUGIN_BUNDLE}" "${DESTINATION}"
    codesign --verify --deep --strict "${DESTINATION}"
    echo "Installed: ${DESTINATION}"
    echo "Rescan VST3 plug-ins in Ableton Live before the demo."
fi
