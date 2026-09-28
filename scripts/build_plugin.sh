#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_DIR="${APW_BUILD_DIR:-${PROJECT_ROOT}/build}"
BUILD_TYPE="${APW_BUILD_TYPE:-Release}"
OS_NAME="$(uname -s)"
MACOS_DEPLOYMENT_TARGET="${APW_MACOS_DEPLOYMENT_TARGET:-12.0}"
CODESIGN_IDENTITY="${APW_CODESIGN_IDENTITY:--}"
JUCE_SOURCE_DIR="${APW_JUCE_DIR:-}"

CMAKE_ARGS=(
    -S "${PROJECT_ROOT}"
    -B "${BUILD_DIR}"
    -DCMAKE_BUILD_TYPE="${BUILD_TYPE}"
)
if [[ "${OS_NAME}" == "Darwin" ]]; then
    CMAKE_ARGS+=(-DCMAKE_OSX_DEPLOYMENT_TARGET="${MACOS_DEPLOYMENT_TARGET}")
fi
if [[ -n "${JUCE_SOURCE_DIR}" ]]; then
    CMAKE_ARGS+=(-DAPW_JUCE_DIR="${JUCE_SOURCE_DIR}")
fi
cmake "${CMAKE_ARGS[@]}"
cmake --build "${BUILD_DIR}" --target AudioProvenanceCapture_VST3 --config "${BUILD_TYPE}"

PLUGIN_BUNDLE="${BUILD_DIR}/AudioProvenanceCapture_artefacts/${BUILD_TYPE}/VST3/Audio Provenance Capture.vst3"
if [[ ! -d "${PLUGIN_BUNDLE}" ]]; then
    echo "Build completed, but the expected VST3 bundle was not found:" >&2
    echo "  ${PLUGIN_BUNDLE}" >&2
    exit 1
fi

if [[ "${OS_NAME}" == "Darwin" ]]; then
    # JUCE's VST3 manifest helper writes moduleinfo.json after the linker's initial
    # signature. Re-sign the completed bundle so its resource seal includes that
    # generated manifest. The default remains local ad-hoc signing; a meeting build
    # can set APW_CODESIGN_IDENTITY to an available Developer ID identity.
    if [[ "${CODESIGN_IDENTITY}" == "-" ]]; then
        codesign --force --deep --sign - --timestamp=none "${PLUGIN_BUNDLE}"
        SIGNING_DESCRIPTION="ad-hoc local development signature"
    else
        codesign --force --deep --options runtime --sign "${CODESIGN_IDENTITY}" --timestamp "${PLUGIN_BUNDLE}"
        SIGNING_DESCRIPTION="${CODESIGN_IDENTITY} with hardened runtime and secure timestamp"
    fi
    codesign --verify --deep --strict "${PLUGIN_BUNDLE}"
    echo "VST3 ready: ${PLUGIN_BUNDLE}"
    echo "Signing: ${SIGNING_DESCRIPTION}"
else
    echo "VST3 ready: ${PLUGIN_BUNDLE}"
    echo "Signing: none (code signing is only implemented for macOS)"
fi

if [[ "${1:-}" == "--install" ]]; then
    if [[ "${OS_NAME}" == "Darwin" ]]; then
        INSTALL_DIR="${HOME}/Library/Audio/Plug-Ins/VST3"
        DESTINATION="${INSTALL_DIR}/Audio Provenance Capture.vst3"
        mkdir -p "${INSTALL_DIR}"
        ditto "${PLUGIN_BUNDLE}" "${DESTINATION}"
        codesign --verify --deep --strict "${DESTINATION}"
        echo "Installed: ${DESTINATION}"
        echo "Rescan VST3 plug-ins in your host before use."
    elif [[ "${OS_NAME}" == "Linux" ]]; then
        INSTALL_DIR="${HOME}/.vst3"
        mkdir -p "${INSTALL_DIR}"
        cp -R "${PLUGIN_BUNDLE}" "${INSTALL_DIR}/"
        echo "Installed: ${INSTALL_DIR}/Audio Provenance Capture.vst3"
        echo "Rescan VST3 plug-ins in your host before use."
    else
        echo "--install is not implemented for ${OS_NAME}; copy the bundle to your VST3 folder manually." >&2
        exit 1
    fi
fi
