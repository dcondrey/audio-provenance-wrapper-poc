#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

KEYCHAIN_PROFILE="${APW_NOTARY_PROFILE:-audio-provenance-notary}"
DEFAULT_DMG="${PROJECT_ROOT}/build-dist/dist/AudioProvenanceCapture-${APW_VERSION:-0.9.0}.dmg"
DMG_PATH="${1:-${DEFAULT_DMG}}"
LOG_DIR="${PROJECT_ROOT}/build-dist/notarization"
STAGE_DIR="${APW_DIST_BUILD_DIR:-${PROJECT_ROOT}/build-dist}/stage/Audio Provenance Capture"
VST3_NAME="Audio Provenance Capture.vst3"
AU_NAME="Audio Provenance Capture.component"

die() { echo "ERROR: $*" >&2; exit 1; }
step() { echo; echo "==> $*"; }

# One submission carries one ticket. Stapling only the disk image leaves the bundles a
# recipient drags out of it with no embedded ticket, so Gatekeeper has to reach Apple
# online to admit them and Live's plug-in scan drops them on a blocked network.
submit_and_staple() {
    local target="$1" label="$2" payload="${LOG_DIR}/${2}.zip" json="${LOG_DIR}/${2}-submit.json"
    step "Notarizing ${label}"
    codesign --verify --strict --verbose=2 "${target}" \
        || die "${target} is not validly signed; re-run package_installer.sh."
    if [[ -d "${target}" ]]; then
        rm -f "${payload}"
        ditto -c -k --keepParent "${target}" "${payload}" \
            || die "could not zip ${target} for submission"
    else
        payload="${target}"
    fi
    local rc=0
    set +e
    xcrun notarytool submit "${payload}" \
        --keychain-profile "${KEYCHAIN_PROFILE}" \
        --output-format json --wait > "${json}"
    rc=$?
    set -e
    cat "${json}"
    local id status
    id="$(/usr/bin/python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get("id",""))' "${json}" 2>/dev/null || true)"
    status="$(/usr/bin/python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get("status",""))' "${json}" 2>/dev/null || true)"
    if [[ -n "${id}" ]]; then
        xcrun notarytool log "${id}" --keychain-profile "${KEYCHAIN_PROFILE}" \
            "${LOG_DIR}/${label}-${id}.json" >/dev/null 2>&1 || true
    fi
    if [[ "${status}" != "Accepted" || "${rc}" -ne 0 ]]; then
        [[ -f "${LOG_DIR}/${label}-${id}.json" ]] && cat "${LOG_DIR}/${label}-${id}.json" >&2
        die "notarization of ${label} did not succeed (status='${status:-unknown}', rc=${rc}). Log: ${LOG_DIR}/${label}-${id:-none}.json"
    fi
    xcrun stapler staple "${target}" || die "stapler could not attach the ticket to ${target}"
    xcrun stapler validate "${target}" || die "stapler validate failed on ${target}"
    echo "stapled: ${target}  (submission ${id})"
    SUBMISSION_ID="${id}"
}

# The recipient never uses the staging tree: they use the copy create-dmg wrote into the
# image. This is the only check that proves the ticket survived that copy, so it runs on
# bundles extracted from the finished DMG, not on the ones that were stapled.
assert_dmg_bundles_carry_tickets() {
    local mount="/Volumes/Audio Provenance Capture" out="${LOG_DIR}/from-dmg" name
    rm -rf "${out}"
    mkdir -p "${out}"
    hdiutil detach "${mount}" >/dev/null 2>&1 || true
    hdiutil attach -nobrowse -readonly "${DMG_PATH}" >/dev/null \
        || die "could not mount ${DMG_PATH} to check the bundles it actually contains"
    for name in "${VST3_NAME}" "${AU_NAME}"; do
        ditto "${mount}/${name}" "${out}/${name}" \
            || { hdiutil detach "${mount}" >/dev/null 2>&1 || true; die "${name} is missing from the mounted DMG"; }
    done
    hdiutil detach "${mount}" >/dev/null || die "could not unmount ${mount}"

    for name in "${VST3_NAME}" "${AU_NAME}"; do
        xcrun stapler validate "${out}/${name}" \
            || die "${name} copied out of the DMG carries no notarization ticket. Stapling the image does not staple what a recipient drags out of it."
        # There is no second Mac here, so quarantine it the way a download would and ask
        # Gatekeeper directly. A stapled bundle passes with no network round trip.
        xattr -w com.apple.quarantine "0083;00000000;Safari;" "${out}/${name}" 2>/dev/null || true
        spctl --assess --type install --verbose=2 "${out}/${name}" 2>&1 | tee "${LOG_DIR}/spctl-${name}.txt"
        grep -q ': accepted' "${LOG_DIR}/spctl-${name}.txt" \
            || die "Gatekeeper did not accept ${name} after it was quarantined the way a download would be. See ${LOG_DIR}/spctl-${name}.txt"
        echo "ticket + Gatekeeper OK for the ${name} a recipient copies out of the DMG"
    done
}

[[ -f "${DMG_PATH}" ]] || die "DMG not found: ${DMG_PATH}. Run ${SCRIPT_DIR}/package_installer.sh first."
DMG_PATH="$(cd "$(dirname "${DMG_PATH}")" && pwd)/$(basename "${DMG_PATH}")"
command -v xcrun >/dev/null || die "xcrun not found; install the Xcode command line tools"
mkdir -p "${LOG_DIR}"

step "Preflight"
[[ -d "${STAGE_DIR}/${VST3_NAME}" && -d "${STAGE_DIR}/${AU_NAME}" ]] \
    || die "the staged bundles are missing from ${STAGE_DIR}. Re-run package_installer.sh; the plug-ins must be stapled before the DMG is built around them."
xcrun notarytool history --keychain-profile "${KEYCHAIN_PROFILE}" >/dev/null 2>&1 \
    || die "the notarytool keychain profile '${KEYCHAIN_PROFILE}' is missing or invalid. Create it with: xcrun notarytool store-credentials '${KEYCHAIN_PROFILE}' --apple-id <id> --team-id <team> --password <app-specific-password>"

submit_and_staple "${STAGE_DIR}/${VST3_NAME}" vst3
submit_and_staple "${STAGE_DIR}/${AU_NAME}" au

step "Rebuilding the DMG around the stapled bundles"
"${SCRIPT_DIR}/package_installer.sh" --dmg-only \
    || die "could not rebuild the DMG from the stapled bundles"

submit_and_staple "${DMG_PATH}" dmg
spctl --assess --type open --context context:primary-signature --verbose=2 "${DMG_PATH}" \
    || die "Gatekeeper rejected the stapled DMG"

step "Checking the bundles a recipient actually copies out of the image"
assert_dmg_bundles_carry_tickets

step "Result"
echo "Notarized and stapled: ${DMG_PATH}"
echo "Plug-ins carry their own tickets, verified on copies taken from the mounted image."
echo "Submission id        : ${SUBMISSION_ID}"
echo "Logs                 : ${LOG_DIR}"
