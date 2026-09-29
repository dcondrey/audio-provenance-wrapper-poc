#!/usr/bin/env bash
# Shared plug-in format table for the macOS packaging scripts. Source it; do not execute it.
# Format ids are the lower-case names users pass in APW_INSTALL_FORMATS and that become the
# pkg choice ids ("fmt-<id>").

APW_PRODUCT_NAME="Audio Provenance Capture"
APW_KNOWN_FORMATS=(vst3 au clap lv2 vst2 auv3 aax)
APW_DEFAULT_FORMATS="vst3,au"

# Directory name JUCE / clap-juce-extensions use under <target>_artefacts/<config>/.
apw_fmt_artefact_subdir() {
    case "$1" in
        vst3) echo VST3 ;; au) echo AU ;; clap) echo CLAP ;; lv2) echo LV2 ;;
        vst2) echo VST ;; auv3) echo Standalone ;; aax) echo AAX ;;
        *) return 1 ;;
    esac
}
apw_fmt_bundle_name() {
    case "$1" in
        vst3) echo "${APW_PRODUCT_NAME}.vst3" ;;
        au) echo "${APW_PRODUCT_NAME}.component" ;;
        clap) echo "${APW_PRODUCT_NAME}.clap" ;;
        lv2) echo "${APW_PRODUCT_NAME}.lv2" ;;
        vst2) echo "${APW_PRODUCT_NAME}.vst" ;;
        auv3) echo "${APW_PRODUCT_NAME}.app" ;;   # container app that embeds the .appex
        aax) echo "${APW_PRODUCT_NAME}.aaxplugin" ;;
        *) return 1 ;;
    esac
}
# Absolute macOS install directory (the pkg payload is rooted at /).
apw_fmt_install_dir() {
    case "$1" in
        vst3) echo "/Library/Audio/Plug-Ins/VST3" ;;
        au) echo "/Library/Audio/Plug-Ins/Components" ;;
        clap) echo "/Library/Audio/Plug-Ins/CLAP" ;;
        lv2) echo "/Library/Audio/Plug-Ins/LV2" ;;
        vst2) echo "/Library/Audio/Plug-Ins/VST" ;;
        auv3) echo "/Applications" ;;
        aax) echo "/Library/Application Support/Avid/Audio/Plug-Ins" ;;
        *) return 1 ;;
    esac
}
apw_fmt_title() {
    case "$1" in
        vst3) echo "VST3" ;; au) echo "Audio Unit (AU)" ;; clap) echo "CLAP" ;; lv2) echo "LV2" ;;
        vst2) echo "VST2 (legacy)" ;; auv3) echo "Audio Unit v3 (AUv3)" ;; aax) echo "AAX (Pro Tools)" ;;
        *) return 1 ;;
    esac
}
# Formats that macOS Installer selects by default when the package carries them.
apw_fmt_default_on() { [[ "$1" == vst3 || "$1" == au ]]; }
# Formats whose payload is a plain directory of files rather than a code-signable bundle.
apw_fmt_is_plain_dir() { [[ "$1" == lv2 ]]; }

# Where a format is staged after package_installer.sh signed it. vst3/au stay where the DMG
# expects them; every other format lives under extra/ so the DMG does not grow.
# Needs DMG_STAGE and STAGE_DIR from the caller.
apw_fmt_staged_path() {
    local fmt="$1" name
    name="$(apw_fmt_bundle_name "${fmt}")" || return 1
    case "${fmt}" in
        vst3|au) echo "${DMG_STAGE}/${name}" ;;
        *) echo "${STAGE_DIR}/extra/${fmt}/${name}" ;;
    esac
}

# Parse a comma/space separated list into the global array APW_FORMATS: validated,
# lower-cased, de-duplicated, in table order. Exits with a message on an unknown id.
apw_parse_formats() {
    local raw="${1:-${APW_DEFAULT_FORMATS}}" tok known found
    local -a want=()
    raw="${raw//,/ }"
    for tok in ${raw}; do
        tok="$(printf '%s' "${tok}" | tr '[:upper:]' '[:lower:]')"
        found=0
        for known in "${APW_KNOWN_FORMATS[@]}"; do
            [[ "${tok}" == "${known}" ]] && found=1
        done
        if [[ "${found}" -ne 1 ]]; then
            echo "ERROR: unknown plug-in format '${tok}'. Known: ${APW_KNOWN_FORMATS[*]}" >&2
            return 1
        fi
        want+=("${tok}")
    done
    [[ "${#want[@]}" -gt 0 ]] || { echo "ERROR: APW_INSTALL_FORMATS selects no formats" >&2; return 1; }
    APW_FORMATS=()
    for known in "${APW_KNOWN_FORMATS[@]}"; do
        for tok in "${want[@]}"; do
            if [[ "${tok}" == "${known}" ]]; then APW_FORMATS+=("${known}"); break; fi
        done
    done
}
