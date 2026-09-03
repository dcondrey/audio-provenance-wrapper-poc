#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

BUILD_DIR="${APW_DIST_BUILD_DIR:-${PROJECT_ROOT}/build-dist}"
STAGE_DIR="${BUILD_DIR}/stage"
DMG_STAGE="${STAGE_DIR}/Audio Provenance Capture"
DIST_DIR="${BUILD_DIR}/dist"
PKG_WORK="${BUILD_DIR}/pkg"
VERSION="${APW_VERSION:-0.9.0}"
PKG_PATH="${DIST_DIR}/AudioProvenanceCapture-${VERSION}.pkg"

IDENTIFIER="com.audioprovenance.capture"
AGENT_IDENTIFIER="com.audioprovenance.capture.loginagent"
AGENT_LABEL="com.audioprovenance.capture.daemon"
SUPPORT_DIR="/Library/Application Support/Audio Provenance Capture"
INSTALLER_IDENTITY="${APW_INSTALLER_IDENTITY:-Developer ID Installer: David Condrey (U3PZN7P3E5)}"
NOTARY_PROFILE="${APW_NOTARY_PROFILE:-audio-provenance-notary}"

VST3_NAME="Audio Provenance Capture.vst3"
AU_NAME="Audio Provenance Capture.component"
DAEMON_DIR_NAME="apw-daemon"

die() { echo "ERROR: $*" >&2; exit 1; }
step() { echo; echo "==> $*"; }

require_staged_input() {
    step "Checking the signed payload"
    [[ -d "${DMG_STAGE}/${VST3_NAME}" ]] \
        || die "${DMG_STAGE}/${VST3_NAME} is missing. Run scripts/package_installer.sh first; this script packages what that one signed."
    [[ -d "${DMG_STAGE}/${AU_NAME}" ]] || die "${DMG_STAGE}/${AU_NAME} is missing. Run scripts/package_installer.sh first."

    # REQUIRED: pkgbuild copies payload verbatim, so an unsigned or adhoc bundle here ships
    # unsigned and Gatekeeper rejects it on the recipient's Mac with no error we could catch later.
    local bundle
    for bundle in "${DMG_STAGE}/${VST3_NAME}" "${DMG_STAGE}/${AU_NAME}"; do
        codesign --verify --deep --strict "${bundle}" 2>/dev/null \
            || die "${bundle##*/} is not validly signed. Re-run scripts/package_installer.sh."
        codesign -dv --verbose=4 "${bundle}" 2>&1 | grep -q "TeamIdentifier=U3PZN7P3E5" \
            || die "${bundle##*/} carries no Developer ID team. Re-run scripts/package_installer.sh."
    done
    echo "Both bundles are Developer ID signed."
}

build_payload_root() {
    step "Laying out the payload root"
    rm -rf "${PKG_WORK}"
    local root="${PKG_WORK}/root"
    mkdir -p "${root}/Library/Audio/Plug-Ins/VST3" \
             "${root}/Library/Audio/Plug-Ins/Components" \
             "${root}${SUPPORT_DIR}"

    ditto "${DMG_STAGE}/${VST3_NAME}" "${root}/Library/Audio/Plug-Ins/VST3/${VST3_NAME}" \
        || die "could not stage the VST3 into the payload"
    ditto "${DMG_STAGE}/${AU_NAME}" "${root}/Library/Audio/Plug-Ins/Components/${AU_NAME}" \
        || die "could not stage the AU into the payload"

    if [[ -d "${DMG_STAGE}/Documentation and Demo Project" ]]; then
        ditto "${DMG_STAGE}/Documentation and Demo Project" "${root}${SUPPORT_DIR}/Documentation and Demo Project" \
            || die "could not stage the documentation"
    else
        echo "WARNING: no documentation staged; the package will omit it." >&2
    fi

    local cmd
    for cmd in "Install Plug-Ins.command" "Start Capture Daemon.command"; do
        [[ -f "${DMG_STAGE}/${cmd}" ]] && ditto "${DMG_STAGE}/${cmd}" "${root}${SUPPORT_DIR}/${cmd}"
    done

    # The daemon already rides inside both bundles' Resources. A copy here gives the support
    # scripts one stable path that does not depend on which plug-in format was loaded.
    if [[ -d "${DMG_STAGE}/${VST3_NAME}/Contents/Resources/${DAEMON_DIR_NAME}" ]]; then
        ditto "${DMG_STAGE}/${VST3_NAME}/Contents/Resources/${DAEMON_DIR_NAME}" \
              "${root}${SUPPORT_DIR}/${DAEMON_DIR_NAME}" || die "could not stage the daemon"
    fi

    echo "Payload root: ${root}"
    du -sh "${root}" | awk '{print "Payload size: "$1}'
}

write_scripts() {
    step "Writing the postinstall script"
    local dir="${PKG_WORK}/scripts"
    mkdir -p "${dir}"
    cat > "${dir}/postinstall" <<'POSTINSTALL'
#!/bin/bash
# Exit non-zero here aborts the install and rolls the user back, so every step is best-effort
# except the ones that would leave a genuinely broken install.
set -u

SUPPORT_DIR="/Library/Application Support/Audio Provenance Capture"

chmod -R a+rX "/Library/Audio/Plug-Ins/VST3/Audio Provenance Capture.vst3" 2>/dev/null || true
chmod -R a+rX "/Library/Audio/Plug-Ins/Components/Audio Provenance Capture.component" 2>/dev/null || true
[ -d "${SUPPORT_DIR}" ] && chmod -R a+rX "${SUPPORT_DIR}" 2>/dev/null || true

# An installed .component is invisible to hosts until the registrar re-scans. It respawns on
# demand, so killing it is the supported way to force that without a logout.
killall -9 AudioComponentRegistrar 2>/dev/null || true

exit 0
POSTINSTALL
    chmod +x "${dir}/postinstall"
    echo "postinstall written"
}

build_agent_component() {
    step "Building the optional login-agent component"
    local plist_src="${PROJECT_ROOT}/packaging/launchagent/${AGENT_LABEL}.plist"
    [[ -f "${plist_src}" ]] || die "missing ${plist_src}"

    local root="${PKG_WORK}/agent-root"
    local scripts="${PKG_WORK}/agent-scripts"
    mkdir -p "${root}/Library/LaunchAgents" "${scripts}"
    ditto "${plist_src}" "${root}/Library/LaunchAgents/${AGENT_LABEL}.plist" \
        || die "could not stage the LaunchAgent plist"
    plutil -lint "${root}/Library/LaunchAgents/${AGENT_LABEL}.plist" >/dev/null \
        || die "the LaunchAgent plist is malformed"

    cat > "${scripts}/postinstall" <<POSTINSTALL
#!/bin/bash
set -u
LABEL="${AGENT_LABEL}"
PLIST="/Library/LaunchAgents/\${LABEL}.plist"
chown root:wheel "\${PLIST}" 2>/dev/null || true
chmod 644 "\${PLIST}" 2>/dev/null || true

# The installer runs as root, but a LaunchAgent belongs to the logged-in user's GUI domain.
# Without this the agent would not start until the next login.
CONSOLE_UID="\$(stat -f %u /dev/console 2>/dev/null || echo 0)"
if [ "\${CONSOLE_UID}" -gt 0 ]; then
    launchctl bootout "gui/\${CONSOLE_UID}/\${LABEL}" 2>/dev/null || true
    launchctl bootstrap "gui/\${CONSOLE_UID}" "\${PLIST}" 2>/dev/null || true
fi
exit 0
POSTINSTALL
    chmod +x "${scripts}/postinstall"

    pkgbuild --root "${root}" \
             --scripts "${scripts}" \
             --identifier "${AGENT_IDENTIFIER}" \
             --version "${VERSION}" \
             --install-location "/" \
             "${PKG_WORK}/agent.pkg" || die "pkgbuild failed for the login agent"
    echo "Agent component: ${PKG_WORK}/agent.pkg"
}

build_component_pkg() {
    step "Building the component package"
    local root="${PKG_WORK}/root"
    local plist="${PKG_WORK}/component.plist"
    local component="${PKG_WORK}/component.pkg"

    pkgbuild --analyze --root "${root}" "${plist}" >/dev/null \
        || die "pkgbuild --analyze failed"

    # IMPORTANT: pkgbuild --analyze omits BundleIsRelocatable entirely, and its absence leaves the
    # relocatable default in force: Installer then redirects the payload onto an older copy found
    # anywhere on disk, so the install reports success while the stale bundle keeps loading. The
    # key has to be ADDED, not set. BundleIsVersionChecked is disabled for the same class of
    # silent no-op, where a same-or-newer bundle already present makes Installer skip the write.
    local i=0
    while /usr/libexec/PlistBuddy -c "Print :${i}:RootRelativeBundlePath" "${plist}" >/dev/null 2>&1; do
        /usr/libexec/PlistBuddy -c "Add :${i}:BundleIsRelocatable bool false" "${plist}" >/dev/null 2>&1 \
            || /usr/libexec/PlistBuddy -c "Set :${i}:BundleIsRelocatable false" "${plist}" \
            || die "could not pin BundleIsRelocatable on component ${i}"
        /usr/libexec/PlistBuddy -c "Add :${i}:BundleIsVersionChecked bool false" "${plist}" >/dev/null 2>&1 \
            || /usr/libexec/PlistBuddy -c "Set :${i}:BundleIsVersionChecked false" "${plist}" \
            || die "could not clear BundleIsVersionChecked on component ${i}"
        i=$((i + 1))
    done
    [[ "${i}" -gt 0 ]] || die "pkgbuild --analyze found no bundles; the payload root is wrong."
    echo "Pinned BundleIsRelocatable=false and BundleIsVersionChecked=false on ${i} bundle(s)."

    pkgbuild --root "${root}" \
             --component-plist "${plist}" \
             --scripts "${PKG_WORK}/scripts" \
             --identifier "${IDENTIFIER}" \
             --version "${VERSION}" \
             --install-location "/" \
             "${component}" || die "pkgbuild failed"
    echo "Component: ${component}"
}

build_product() {
    step "Building the distribution package"
    local component="${PKG_WORK}/component.pkg"
    local dist="${PKG_WORK}/distribution.xml"
    local resources="${PKG_WORK}/resources"
    mkdir -p "${resources}"

    cat > "${resources}/welcome.html" <<HTML
<!doctype html><meta charset="utf-8">
<body style="font:13px -apple-system,BlinkMacSystemFont,sans-serif;margin:0;color:#1d1d1f">
<p>This installs the Audio Provenance Capture plug-in for Ableton Live and other hosts.</p>
<ul>
  <li><b>VST3</b> into /Library/Audio/Plug-Ins/VST3</li>
  <li><b>Audio Unit</b> into /Library/Audio/Plug-Ins/Components</li>
  <li>Demo project, sample clips and the QuickStart guide into Application Support</li>
</ul>
<p>The bundled signing key is a <b>self-issued demo identity</b>. It proves possession of that key
and nothing else: it is not a verified identity and attests to no person or organisation.</p>
</body>
HTML

    cat > "${dist}" <<XML
<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
    <title>Audio Provenance Capture ${VERSION}</title>
    <welcome file="welcome.html" mime-type="text/html"/>
    <options customize="allow" require-scripts="false" hostArchitectures="arm64,x86_64"/>
    <volume-check>
        <allowed-os-versions><os-version min="11.0"/></allowed-os-versions>
    </volume-check>
    <choices-outline>
        <line choice="plugins"/>
        <line choice="loginagent"/>
    </choices-outline>
    <choice id="plugins"
            title="Plug-ins and demo project"
            enabled="false"
            description="Installs the VST3 and Audio Unit into /Library/Audio/Plug-Ins, plus the demo Ableton project, sample clips and the QuickStart guide. Required.">
        <pkg-ref id="${IDENTIFIER}"/>
    </choice>
    <choice id="loginagent"
            title="Start the capture service at login (optional)"
            start_selected="false"
            description="Runs the local capture service automatically so it is always ready when you open your DAW. Without it you must start it by hand before loading the plug-in, and if you forget, the plug-in records nothing and gives no warning. The service runs only on this Mac, listens on a local port, sends nothing anywhere, and records nothing until a capture plug-in sends it events. You can turn it off later with: launchctl bootout gui/\$UID/${AGENT_LABEL}">
        <pkg-ref id="${AGENT_IDENTIFIER}"/>
    </choice>
    <pkg-ref id="${IDENTIFIER}" version="${VERSION}" onConclusion="none">component.pkg</pkg-ref>
    <pkg-ref id="${AGENT_IDENTIFIER}" version="${VERSION}" onConclusion="none">agent.pkg</pkg-ref>
</installer-gui-script>
XML

    mkdir -p "${DIST_DIR}"
    rm -f "${PKG_WORK}/unsigned.pkg"
    productbuild --distribution "${dist}" \
                 --package-path "${PKG_WORK}" \
                 --resources "${resources}" \
                 "${PKG_WORK}/unsigned.pkg" || die "productbuild failed"
    echo "Unsigned product: ${PKG_WORK}/unsigned.pkg"
}

sign_product() {
    step "Signing the package"
    rm -f "${PKG_PATH}"
    if ! security find-identity -v 2>/dev/null | grep -q "Developer ID Installer"; then
        cp "${PKG_WORK}/unsigned.pkg" "${PKG_PATH}"
        cat >&2 <<'MISSING'
WARNING: no "Developer ID Installer" certificate is in the keychain, so the package is UNSIGNED.
An unsigned .pkg cannot be notarized and Gatekeeper will refuse it on another Mac.
Create one (free, no App ID needed):
  Xcode > Settings > Accounts > select the team > Manage Certificates > + > Developer ID Installer
Then re-run this script. A "3rd Party Mac Developer Installer" certificate is the Mac App Store
variant and CANNOT be used here.
MISSING
        echo "Unsigned package: ${PKG_PATH}"
        return 1
    fi

    productsign --sign "${INSTALLER_IDENTITY}" "${PKG_WORK}/unsigned.pkg" "${PKG_PATH}" \
        || die "productsign failed with identity: ${INSTALLER_IDENTITY}"
    pkgutil --check-signature "${PKG_PATH}" || die "the signed package failed its own signature check"
    echo "Signed package: ${PKG_PATH}"
}

notarize_product() {
    step "Notarizing the package"
    xcrun notarytool submit "${PKG_PATH}" --keychain-profile "${NOTARY_PROFILE}" --wait \
        || die "notarytool submission failed"
    xcrun stapler staple "${PKG_PATH}" || die "stapling failed"
    xcrun stapler validate "${PKG_PATH}" || die "staple validation failed"
    spctl -a -vvv -t install "${PKG_PATH}" 2>&1 | sed 's/^/  /'
    echo "Notarized and stapled: ${PKG_PATH}"
}

main() {
    require_staged_input
    build_payload_root
    write_scripts
    build_component_pkg
    build_agent_component
    build_product
    if sign_product; then
        if [[ "${APW_SKIP_NOTARIZE:-0}" == "1" ]]; then
            echo "APW_SKIP_NOTARIZE=1, leaving the package unnotarized."
        else
            notarize_product
        fi
    else
        echo
        echo "Stopped before notarization: the package is unsigned."
        exit 2
    fi
    echo
    echo "==> Result"
    echo "PKG     : ${PKG_PATH}"
    ls -lh "${PKG_PATH}" | awk '{print "Size    : "$5}'
}

main "$@"
