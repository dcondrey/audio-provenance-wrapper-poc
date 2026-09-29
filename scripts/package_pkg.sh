#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

BUILD_DIR="${APW_DIST_BUILD_DIR:-${PROJECT_ROOT}/build-dist}"
STAGE_DIR="${BUILD_DIR}/stage"
DMG_STAGE="${STAGE_DIR}/Audio Provenance Capture"
DIST_DIR="${BUILD_DIR}/dist"
VERSION="${APW_VERSION:-0.9.0}"

# APW_PKG_DEV_UNSIGNED=1 builds a structurally complete pkg from unsigned/ad-hoc bundles so
# the choice layout can be inspected (pkgutil --expand, installer -showChoicesXML). It writes
# a differently named pkg into a separate work dir, never signs or notarizes, and is refused
# when APW_RELEASE=1. A DEV pkg must never be distributed.
DEV_UNSIGNED="${APW_PKG_DEV_UNSIGNED:-0}"
if [[ "${DEV_UNSIGNED}" == "1" ]]; then
    if [[ "${APW_RELEASE:-0}" == "1" ]]; then
        echo "ERROR: APW_PKG_DEV_UNSIGNED=1 is refused when APW_RELEASE=1; a release pkg must be Developer ID signed." >&2
        exit 1
    fi
    PKG_WORK="${BUILD_DIR}/pkg-dev"
    PKG_PATH="${DIST_DIR}/AudioProvenanceCapture-${VERSION}-DEV-UNSIGNED.pkg"
else
    PKG_WORK="${BUILD_DIR}/pkg"
    PKG_PATH="${DIST_DIR}/AudioProvenanceCapture-${VERSION}.pkg"
fi
COMP_DIR="${PKG_WORK}/components"

IDENTIFIER="com.audioprovenance.capture"
AGENT_IDENTIFIER="com.audioprovenance.capture.loginagent"
AGENT_LABEL="com.audioprovenance.capture.daemon"
SUPPORT_DIR="/Library/Application Support/Audio Provenance Capture"
INSTALLER_IDENTITY="${APW_INSTALLER_IDENTITY:-Developer ID Installer: David Condrey (U3PZN7P3E5)}"
NOTARY_PROFILE="${APW_NOTARY_PROFILE:-audio-provenance-notary}"

DAEMON_DIR_NAME="apw-daemon"
DEVELOPER_TEAM="U3PZN7P3E5"

# Which plug-in formats are BUILT INTO the pkg (the user then picks among them at install
# time): APW_INSTALL_FORMATS=vst3,au,clap,lv2,vst2,auv3,aax. Default vst3,au.
# shellcheck source=../packaging/lib/formats.sh
source "${PROJECT_ROOT}/packaging/lib/formats.sh"
apw_parse_formats "${APW_INSTALL_FORMATS:-}" || exit 1

# Installer choice ids (also the -applyChoiceChangesXML identifiers):
#   vst3 au clap lv2 vst2 auv3 aax   one per format built in
#   daemon                            frozen capture daemon + helper scripts (support dir)
#   docs                              demo project, sample clips, QuickStart
#   loginagent                        LaunchAgent that starts the daemon at login (needs daemon)
COMPONENT_IDS=()   # every component package built, as choice ids

die() { echo "ERROR: $*" >&2; exit 1; }
step() { echo; echo "==> $*"; }

fmt_pkg_id() { echo "${IDENTIFIER}.$1"; }

# Every selected component must be present and Developer ID signed; pkgbuild copies the
# payload verbatim, so an unsigned bundle here ships unsigned and Gatekeeper rejects it on the
# recipient's Mac with no error we could catch later.
assert_bundle_signed() {
    local target="$1" label="$2"
    if [[ "${DEV_UNSIGNED}" == "1" ]]; then
        echo "DEV-UNSIGNED: not requiring a Developer ID signature on ${label}" >&2
        return 0
    fi
    codesign --verify --deep --strict "${target}" 2>/dev/null \
        || die "${label} is not validly signed. Re-run scripts/package_installer.sh."
    codesign -dv --verbose=4 "${target}" 2>&1 | grep -q "TeamIdentifier=${DEVELOPER_TEAM}" \
        || die "${label} carries no Developer ID team. Re-run scripts/package_installer.sh."
}

# A plain directory (LV2) has no bundle signature; every Mach-O in it must verify instead.
assert_plain_dir_signed() {
    local dir="$1" label="$2" f count=0
    while IFS= read -r -d '' f; do
        lipo -archs "${f}" >/dev/null 2>&1 || continue
        count=$((count + 1))
        if [[ "${DEV_UNSIGNED}" == "1" ]]; then continue; fi
        codesign --verify --strict "${f}" 2>/dev/null \
            || die "${label}: ${f##*/} is not validly signed. Re-run scripts/package_installer.sh."
        codesign -dv --verbose=4 "${f}" 2>&1 | grep -q "TeamIdentifier=${DEVELOPER_TEAM}" \
            || die "${label}: ${f##*/} carries no Developer ID team. Re-run scripts/package_installer.sh."
    done < <(find "${dir}" -type f -print0)
    [[ "${count}" -gt 0 ]] || die "${label} contains no Mach-O file"
}

require_staged_input() {
    step "Checking the signed payload (formats: ${APW_FORMATS[*]})"
    [[ "${DEV_UNSIGNED}" == "1" ]] && echo "WARNING: APW_PKG_DEV_UNSIGNED=1, signatures are NOT checked. This pkg is not distributable." >&2
    local fmt staged
    for fmt in "${APW_FORMATS[@]}"; do
        staged="$(apw_fmt_staged_path "${fmt}")"
        [[ -d "${staged}" ]] \
            || die "${staged} is missing. Run scripts/package_installer.sh with APW_INSTALL_FORMATS including ${fmt} first; this script packages what that one signed."
        if apw_fmt_is_plain_dir "${fmt}"; then
            assert_plain_dir_signed "${staged}" "${staged##*/}"
        else
            assert_bundle_signed "${staged}" "${staged##*/}"
        fi
    done
    echo "All ${#APW_FORMATS[@]} selected format(s) present and signed."
}

# Locate the frozen daemon inside any staged bundle; it is copied into the daemon component.
find_staged_daemon() {
    local fmt cand
    for fmt in "${APW_FORMATS[@]}"; do
        cand="$(apw_fmt_staged_path "${fmt}")/Contents/Resources/${DAEMON_DIR_NAME}"
        if [[ -x "${cand}/${DAEMON_DIR_NAME}" ]]; then printf '%s\n' "${cand}"; return 0; fi
    done
    return 1
}

# Root cause, traced with an interposer on write(2): while pkgbuild frees the BOM it built
# (PKBOMDirectoryEnumerator dealloc -> BOMStorageCommit), Apple's PackageKit flushes the
# temporary NSIRD_*/package.bom through a descriptor whose write returns EACCES. That
# prints "write: Permission denied" four times per call. The BOM in the finished package is
# complete (lsbom lists every payload entry), so the line is noise, but only that exact line
# is dropped and only after the output package reads back cleanly; every other pkgbuild
# message and any nonzero exit still surface.
pkgbuild_checked() {
    local errfile status noise out
    out="${!#}"
    errfile="$(mktemp "${TMPDIR:-/tmp}/pkgbuild-err.XXXXXX")" || die "could not create a pkgbuild log"
    status=0
    pkgbuild "$@" 2>"${errfile}" || status=$?
    noise="$(grep -c -x 'write: Permission denied' "${errfile}" || true)"
    grep -v -x 'write: Permission denied' "${errfile}" >&2 || true
    command rm -f -- "${errfile}"
    if [[ ${status} -eq 0 && "${noise}" -gt 0 && "$1" != "--analyze" ]]; then
        pkgutil --payload-files "${out}" >/dev/null 2>&1 \
            || die "pkgbuild reported success but ${out##*/} does not read back"
        echo "note: ${noise} PackageKit BOM-flush warnings suppressed for ${out##*/} (package verified readable)"
    fi
    return ${status}
}

# pkgbuild --analyze omits BundleIsRelocatable entirely, and its absence leaves the
# relocatable default in force: Installer then redirects the payload onto an older copy found
# anywhere on disk, so the install reports success while the stale bundle keeps loading. The
# key has to be ADDED, not set. BundleIsVersionChecked is disabled for the same class of
# silent no-op, where a same-or-newer bundle already present makes Installer skip the write.
# Prints the number of bundles pinned.
pin_bundles() {
    local root="$1" plist="$2" i=0
    pkgbuild_checked --analyze --root "${root}" "${plist}" >/dev/null || die "pkgbuild --analyze failed"
    while /usr/libexec/PlistBuddy -c "Print :${i}:RootRelativeBundlePath" "${plist}" >/dev/null 2>&1; do
        /usr/libexec/PlistBuddy -c "Add :${i}:BundleIsRelocatable bool false" "${plist}" >/dev/null 2>&1 \
            || /usr/libexec/PlistBuddy -c "Set :${i}:BundleIsRelocatable false" "${plist}" \
            || die "could not pin BundleIsRelocatable on component ${i}"
        /usr/libexec/PlistBuddy -c "Add :${i}:BundleIsVersionChecked bool false" "${plist}" >/dev/null 2>&1 \
            || /usr/libexec/PlistBuddy -c "Set :${i}:BundleIsVersionChecked false" "${plist}" \
            || die "could not clear BundleIsVersionChecked on component ${i}"
        i=$((i + 1))
    done
    echo "${i}"
}

# build_component <choice-id> <root> <scripts-dir> <expects-bundles: yes|no>
build_component() {
    local id="$1" root="$2" scripts="$3" bundles="$4"
    local plist="${PKG_WORK}/${id}.plist" out n
    out="${COMP_DIR}/$(fmt_pkg_id "${id}").pkg"
    local -a args=(--root "${root}" --identifier "$(fmt_pkg_id "${id}")" --version "${VERSION}" --install-location "/")
    [[ -n "${scripts}" ]] && args+=(--scripts "${scripts}")
    # Every component is analyzed, not just the plug-ins: the daemon tree can carry nested
    # bundles (frameworks), and pkgbuild's inferred defaults leave those relocatable.
    n="$(pin_bundles "${root}" "${plist}")"
    if [[ "${n}" -gt 0 ]]; then
        echo "${id}: pinned BundleIsRelocatable=false and BundleIsVersionChecked=false on ${n} bundle(s)."
        args+=(--component-plist "${plist}")
    elif [[ "${bundles}" == yes ]]; then
        die "pkgbuild --analyze found no bundles in the ${id} payload; the payload root is wrong."
    fi
    pkgbuild_checked "${args[@]}" "${out}" || die "pkgbuild failed for ${id}"
    COMPONENT_IDS+=("${id}")
}

write_format_postinstall() {
    local fmt="$1" dir="$2" bundle_path
    bundle_path="$(apw_fmt_install_dir "${fmt}")/$(apw_fmt_bundle_name "${fmt}")"
    {
        echo '#!/bin/bash'
        echo '# Best-effort: a non-zero exit here aborts the install and rolls the user back.'
        echo 'set -u'
        printf 'chmod -R a+rX %q 2>/dev/null || true\n' "${bundle_path}"
        if [[ "${fmt}" == au || "${fmt}" == auv3 ]]; then
            echo '# An installed Audio Unit is invisible to hosts until the registrar re-scans. It respawns'
            echo '# on demand, so killing it is the supported way to force that without a logout.'
            echo 'killall -9 AudioComponentRegistrar 2>/dev/null || true'
        fi
        echo 'exit 0'
    } > "${dir}/postinstall"
    chmod +x "${dir}/postinstall"
}

build_format_components() {
    step "Building one component package per format"
    rm -rf "${PKG_WORK}"
    mkdir -p "${COMP_DIR}"
    local fmt staged root scripts dest
    for fmt in "${APW_FORMATS[@]}"; do
        staged="$(apw_fmt_staged_path "${fmt}")"
        root="${PKG_WORK}/root-${fmt}"
        scripts="${PKG_WORK}/scripts-${fmt}"
        dest="${root}$(apw_fmt_install_dir "${fmt}")"
        mkdir -p "${dest}" "${scripts}"
        ditto "${staged}" "${dest}/$(apw_fmt_bundle_name "${fmt}")" || die "could not stage ${fmt} into its payload"
        write_format_postinstall "${fmt}" "${scripts}"
        if apw_fmt_is_plain_dir "${fmt}"; then
            build_component "${fmt}" "${root}" "${scripts}" no
        else
            build_component "${fmt}" "${root}" "${scripts}" yes
        fi
    done
}

build_daemon_component() {
    step "Building the daemon component"
    local daemon_src root scripts
    daemon_src="$(find_staged_daemon)" || {
        echo "WARNING: no selected format is a bundle carrying the frozen daemon; the daemon component (and login agent) are omitted." >&2
        HAVE_DAEMON=0
        return 0
    }
    assert_bundle_signed "${daemon_src}/${DAEMON_DIR_NAME}" "the staged daemon executable"
    root="${PKG_WORK}/root-daemon"
    scripts="${PKG_WORK}/scripts-daemon"
    mkdir -p "${root}${SUPPORT_DIR}" "${scripts}"
    ditto "${daemon_src}" "${root}${SUPPORT_DIR}/${DAEMON_DIR_NAME}" || die "could not stage the daemon"
    local cmd
    for cmd in "Install Plug-Ins.command" "Start Capture Daemon.command"; do
        [[ -f "${DMG_STAGE}/${cmd}" ]] && ditto "${DMG_STAGE}/${cmd}" "${root}${SUPPORT_DIR}/${cmd}"
    done
    cat > "${scripts}/postinstall" <<'POSTINSTALL'
#!/bin/bash
set -u
chmod -R a+rX "/Library/Application Support/Audio Provenance Capture/apw-daemon" 2>/dev/null || true
exit 0
POSTINSTALL
    chmod +x "${scripts}/postinstall"
    build_component daemon "${root}" "${scripts}" no
    HAVE_DAEMON=1
}

build_docs_component() {
    step "Building the documentation component"
    if [[ ! -d "${DMG_STAGE}/Documentation and Demo Project" ]]; then
        echo "WARNING: no documentation staged; the package will omit it." >&2
        return 0
    fi
    local root="${PKG_WORK}/root-docs"
    mkdir -p "${root}${SUPPORT_DIR}"
    ditto "${DMG_STAGE}/Documentation and Demo Project" "${root}${SUPPORT_DIR}/Documentation and Demo Project" \
        || die "could not stage the documentation"
    build_component docs "${root}" "" no
}

build_agent_component() {
    step "Building the optional login-agent component"
    if [[ "${HAVE_DAEMON}" != 1 ]]; then
        echo "Skipped: the login agent launches the daemon component, which is not in this package."
        return 0
    fi
    local plist_src="${PROJECT_ROOT}/packaging/launchagent/${AGENT_LABEL}.plist"
    [[ -f "${plist_src}" ]] || die "missing ${plist_src}"

    local root="${PKG_WORK}/root-loginagent"
    local scripts="${PKG_WORK}/scripts-loginagent"
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

# The agent launches the daemon component. Installer does not enforce choice dependencies on
# the command line (-applyChoiceChangesXML can select this without the daemon), so enforce it
# here: no daemon, no agent.
if [ ! -x "${SUPPORT_DIR}/${DAEMON_DIR_NAME}/${DAEMON_DIR_NAME}" ]; then
    rm -f "\${PLIST}"
    echo "capture daemon component not installed; skipping the login agent" >&2
    exit 0
fi
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
    # The agent IDENTIFIER keeps its historical value so upgrades match the old receipt.
    local out="${COMP_DIR}/${AGENT_IDENTIFIER}.pkg"
    pkgbuild_checked --root "${root}" --scripts "${scripts}" --identifier "${AGENT_IDENTIFIER}" \
             --version "${VERSION}" --install-location "/" "${out}" \
        || die "pkgbuild failed for the login agent"
    COMPONENT_IDS+=(loginagent)
}

# pkg-ref id for a choice id
choice_pkg_ref() {
    if [[ "$1" == loginagent ]]; then echo "${AGENT_IDENTIFIER}"; else fmt_pkg_id "$1"; fi
}

has_component() {
    local c
    for c in "${COMPONENT_IDS[@]}"; do [[ "${c}" == "$1" ]] && return 0; done
    return 1
}

choice_xml() { # id title start_selected description [enabled-expression]
    local id="$1" title="$2" sel="$3" desc="$4" enabled="${5:-}"
    printf '    <choice id="%s" title="%s" start_selected="%s"' "${id}" "${title}" "${sel}"
    # A dependent choice is both greyed out and forced off while its prerequisite is off.
    [[ -n "${enabled}" ]] && printf ' enabled="%s" selected="my.choice.selected &amp;&amp; %s"' "${enabled}" "${enabled}"
    printf '\n            description="%s">\n        <pkg-ref id="%s"/>\n    </choice>\n' "${desc}" "$(choice_pkg_ref "${id}")"
}

format_description() {
    local fmt="$1"
    echo "Installs ${APW_PRODUCT_NAME}.$(apw_fmt_bundle_name "${fmt}" | sed 's/.*\.//') into $(apw_fmt_install_dir "${fmt}")."
}

build_product() {
    step "Building the distribution package"
    local dist="${PKG_WORK}/distribution.xml" resources="${PKG_WORK}/resources" fmt
    mkdir -p "${resources}"
    local title_suffix=""
    [[ "${DEV_UNSIGNED}" == "1" ]] && title_suffix=" (DEV UNSIGNED, DO NOT DISTRIBUTE)"

    {
        echo '<!doctype html><meta charset="utf-8">'
        echo '<body style="font:13px -apple-system,BlinkMacSystemFont,sans-serif;margin:0;color:#1d1d1f">'
        echo '<p>This installs the Audio Provenance Capture plug-in for Ableton Live and other hosts.'
        echo 'Choose Customize to install only the plug-in formats you use.</p>'
        echo '<ul>'
        for fmt in "${APW_FORMATS[@]}"; do
            echo "  <li><b>$(apw_fmt_title "${fmt}")</b> into $(apw_fmt_install_dir "${fmt}")</li>"
        done
        has_component daemon && echo '  <li>The capture daemon and helper scripts into Application Support</li>'
        has_component docs && echo '  <li>Demo project, sample clips and the QuickStart guide into Application Support</li>'
        echo '</ul>'
        echo '<p>The bundled signing key is a <b>self-issued demo identity</b>. It proves possession of that key'
        echo 'and nothing else: it is not a verified identity and attests to no person or organisation.</p>'
        echo '</body>'
    } > "${resources}/welcome.html"

    {
        echo '<?xml version="1.0" encoding="utf-8"?>'
        echo '<installer-gui-script minSpecVersion="2">'
        echo "    <title>Audio Provenance Capture ${VERSION}${title_suffix}</title>"
        echo '    <welcome file="welcome.html" mime-type="text/html"/>'
        echo '    <options customize="allow" require-scripts="false" hostArchitectures="arm64,x86_64"/>'
        echo '    <volume-check>'
        echo '        <allowed-os-versions><os-version min="11.0"/></allowed-os-versions>'
        echo '    </volume-check>'
        echo '    <choices-outline>'
        echo '        <line choice="formats">'
        for fmt in "${APW_FORMATS[@]}"; do echo "            <line choice=\"${fmt}\"/>"; done
        echo '        </line>'
        # Not nested: a parent choice's selection follows its children, which would show the
        # daemon as deselected whenever the (default-off) login agent is.
        has_component daemon && echo '        <line choice="daemon"/>'
        has_component loginagent && echo '        <line choice="loginagent"/>'
        has_component docs && echo '        <line choice="docs"/>'
        echo '    </choices-outline>'
        echo '    <choice id="formats" title="Plug-in formats" start_selected="true"'
        echo '            description="Choose the plug-in formats to install. Install only the ones your host uses."/>'
        for fmt in "${APW_FORMATS[@]}"; do
            local sel=false
            apw_fmt_default_on "${fmt}" && sel=true
            choice_xml "${fmt}" "$(apw_fmt_title "${fmt}")" "${sel}" "$(format_description "${fmt}")"
        done
        has_component daemon && choice_xml daemon "Capture daemon and helper scripts" true \
            "The local capture service the plug-ins send events to, plus the Start Capture Daemon helper. The plug-ins record nothing until it is running."
        has_component docs && choice_xml docs "Demo project and documentation" true \
            "The demo Ableton project, sample clips, trust anchor and the QuickStart guide, installed into Application Support."
        has_component loginagent && choice_xml loginagent "Start the capture service at login (optional)" false \
            "Runs the local capture service automatically so it is always ready when you open your DAW. Without it you must start it by hand before loading the plug-in, and if you forget, the plug-in records nothing and gives no warning. The service runs only on this Mac, listens on a local port, sends nothing anywhere, and records nothing until a capture plug-in sends it events. Requires the capture daemon. Turn it off later with: launchctl bootout gui/\$UID/${AGENT_LABEL}" \
            "choices.daemon.selected"
        for id in "${COMPONENT_IDS[@]}"; do
            echo "    <pkg-ref id=\"$(choice_pkg_ref "${id}")\" version=\"${VERSION}\" onConclusion=\"none\">$(choice_pkg_ref "${id}").pkg</pkg-ref>"
        done
        echo '</installer-gui-script>'
    } > "${dist}"
    xmllint --noout "${dist}" 2>/dev/null || die "generated distribution.xml is not well-formed: ${dist}"

    mkdir -p "${DIST_DIR}"
    rm -f "${PKG_WORK}/unsigned.pkg"
    productbuild --distribution "${dist}" \
                 --package-path "${COMP_DIR}" \
                 --resources "${resources}" \
                 "${PKG_WORK}/unsigned.pkg" || die "productbuild failed"
    echo "Unsigned product: ${PKG_WORK}/unsigned.pkg"
}

# Ask Installer itself which choices the product offers; every built component must be there.
verify_product_choices() {
    step "Verifying the product's choices with Installer"
    local xml="${PKG_WORK}/choices.xml" id
    installer -pkg "${PKG_WORK}/unsigned.pkg" -showChoicesXML -target / > "${xml}" 2>/dev/null \
        || die "installer could not read the product's choices"
    for id in "${COMPONENT_IDS[@]}"; do
        grep -q "<string>${id}</string>" "${xml}" || die "choice '${id}' is missing from the product (see ${xml})"
    done
    echo "Choices offered: ${COMPONENT_IDS[*]}"
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
    HAVE_DAEMON=0
    require_staged_input
    build_format_components
    build_daemon_component
    build_docs_component
    build_agent_component
    build_product
    verify_product_choices
    if [[ "${DEV_UNSIGNED}" == "1" ]]; then
        rm -f "${PKG_PATH}"
        cp "${PKG_WORK}/unsigned.pkg" "${PKG_PATH}"
        echo
        echo "==> DEV-UNSIGNED result (NOT for distribution: unsigned, unnotarized)"
        echo "PKG     : ${PKG_PATH}"
        return 0
    fi
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
    echo "Formats : ${APW_FORMATS[*]}"
    echo "Choices : ${COMPONENT_IDS[*]}  (see docs/INSTALL.md for installer -applyChoiceChangesXML)"
}

main "$@"
