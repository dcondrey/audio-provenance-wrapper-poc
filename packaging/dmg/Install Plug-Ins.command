#!/bin/bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VST3_NAME="Audio Provenance Capture.vst3"
AU_NAME="Audio Provenance Capture.component"
VST3_DIR="${HOME}/Library/Audio/Plug-Ins/VST3"
AU_DIR="${HOME}/Library/Audio/Plug-Ins/Components"

die() { echo "ERROR: $*" >&2; echo; echo "Press return to close."; read -r _; exit 1; }

echo "Audio Provenance Capture — installer"
echo

[[ -d "${HERE}/${VST3_NAME}" ]] || die "${VST3_NAME} is missing from ${HERE}."
[[ -d "${HERE}/${AU_NAME}" ]] || die "${AU_NAME} is missing from ${HERE}."

# REQUIRED: stock macOS ships no VST3 directory at either scope, so a drag-and-drop
# alias to one dangles. Both directories are created here before the copy.
mkdir -p "${VST3_DIR}" "${AU_DIR}" || die "could not create the plug-in directories"

ditto "${HERE}/${VST3_NAME}" "${VST3_DIR}/${VST3_NAME}" || die "could not install the VST3"
ditto "${HERE}/${AU_NAME}" "${AU_DIR}/${AU_NAME}" || die "could not install the Audio Unit"

xattr -dr com.apple.quarantine "${VST3_DIR}/${VST3_NAME}" 2>/dev/null || true
xattr -dr com.apple.quarantine "${AU_DIR}/${AU_NAME}" 2>/dev/null || true

echo "Installed:"
echo "  ${VST3_DIR}/${VST3_NAME}"
echo "  ${AU_DIR}/${AU_NAME}"
echo
echo "Next:"
echo "  1. Double-click 'Start Capture Daemon.command' on this disk image."
echo "     The plug-in streams its observations to that daemon; with nothing"
echo "     listening, every observation is dropped and no manifest is produced."
echo "  2. In Live: Preferences > Plug-Ins, enable VST3 and rescan."
echo "  3. Insert 'Audio Provenance Capture' on the track you want observed."
echo
echo "Press return to close."
read -r _
