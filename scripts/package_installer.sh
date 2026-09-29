#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

BUILD_TYPE="${APW_BUILD_TYPE:-Release}"
BUILD_DIR="${APW_DIST_BUILD_DIR:-${PROJECT_ROOT}/build-dist}"
ARTEFACT_DIR="${BUILD_DIR}/AudioProvenanceCapture_artefacts/${BUILD_TYPE}"
FREEZE_VENV="${BUILD_DIR}/freeze-venv"
FREEZE_WORK="${BUILD_DIR}/pyinstaller"
STAGE_DIR="${BUILD_DIR}/stage"
DMG_STAGE="${STAGE_DIR}/Audio Provenance Capture"
ASSET_DIR="${PROJECT_ROOT}/packaging/assets"
DIST_DIR="${BUILD_DIR}/dist"
DMG_PATH="${DIST_DIR}/AudioProvenanceCapture-${APW_VERSION:-0.9.0}.dmg"

FREEZE_PYTHON="${APW_FREEZE_PYTHON:-/Library/Frameworks/Python.framework/Versions/3.12/bin/python3.12}"
# REQUIRED: cryptography dropped macOS universal2 wheels after 48.0.1, and the repo's
# requirements.txt pins cryptography>=45,<50, so 48.0.1 is both universal and in range.
CRYPTOGRAPHY_PIN="${APW_CRYPTOGRAPHY_PIN:-48.0.1}"
# cryptography's Rust openssl module still imports _cffi_backend, and cffi ships no
# universal2 wheel at all, so the two thin wheels are lipo-merged by hand.
CFFI_PIN="${APW_CFFI_PIN:-2.1.1}"
# REQUIRED: the frozen daemon must carry the same C2PA engine the repo tests, so
# both pins are read from requirements.txt rather than restated here. c2pa-python
# publishes only thin per-arch wheels and cbor2 publishes no x86_64 macOS wheel at
# all, so both need the lipo merge install_universal_dist performs.
C2PA_PIN=""
CBOR2_PIN=""
DAEMON_DIR_NAME="apw-daemon"
REQUIRED_ASSETS=(quickstart.pdf demo-root-ca.pem demo-provenance-store demo-project)
DEMO_STORE_NAME="demo-provenance-store"
DMG_SCRIPT_DIR="${PROJECT_ROOT}/packaging/dmg"
DAEMON_ENTITLEMENTS="${PROJECT_ROOT}/packaging/apw-daemon.entitlements"
DMG_SCRIPTS=("Install Plug-Ins.command" "Start Capture Daemon.command")
SOURCE_REVISION_FILE=".apw-source-revision"
FATWHEEL_DIR="${BUILD_DIR}/fatwheel"
SOURCE_REVISION=""
DMG_ENABLED=1

VST3_NAME="Audio Provenance Capture.vst3"
AU_NAME="Audio Provenance Capture.component"

# Which plug-in formats to build, sign and stage: APW_INSTALL_FORMATS=vst3,au,clap,lv2,...
# vst3 and au also feed the DMG; every other format is staged under stage/extra/ for the pkg.
# shellcheck source=../packaging/lib/formats.sh
source "${PROJECT_ROOT}/packaging/lib/formats.sh"
apw_parse_formats "${APW_INSTALL_FORMATS:-}" || exit 1

die() { echo "ERROR: $*" >&2; exit 1; }
step() { echo; echo "==> $*"; }

requirement_pin() {
    local name="$1" pin
    pin="$(sed -nE "s/^${name}==([^[:space:];]+).*/\\1/p" "${PROJECT_ROOT}/requirements.txt" | head -1)"
    [[ -n "${pin}" ]] || die "requirements.txt has no '${name}==<version>' pin; the frozen daemon cannot be locked to a tested version."
    printf '%s\n' "${pin}"
}

resolve_pins() {
    C2PA_PIN="${APW_C2PA_PIN:-$(requirement_pin c2pa-python)}"
    CBOR2_PIN="${APW_CBOR2_PIN:-$(requirement_pin cbor2)}"
    echo "Engine pins from requirements.txt: c2pa-python==${C2PA_PIN}, cbor2==${CBOR2_PIN}"
}

# The exact inputs the plug-in binaries are built from. A cached artefact whose
# revision differs from this is rebuilt rather than shipped.
source_revision() {
    {
        printf '%s\n' "${BUILD_TYPE}" "arm64;x86_64" "11.0"
        shasum -a 256 "${PROJECT_ROOT}/CMakeLists.txt"
        find "${PROJECT_ROOT}/src" -type f -print0 | sort -z | xargs -0 shasum -a 256
    } | shasum -a 256 | cut -d' ' -f1
}

# ---------------------------------------------------------------- identity ---
resolve_identity() {
    if [[ -n "${APW_CODESIGN_IDENTITY:-}" ]]; then
        IDENTITY="${APW_CODESIGN_IDENTITY}"
        echo "Signing identity (from APW_CODESIGN_IDENTITY): ${IDENTITY}"
        return
    fi
    local hashes
    hashes="$(security find-identity -v -p codesigning 2>/dev/null \
        | grep 'Developer ID Application' \
        | sed -E 's/^[[:space:]]*[0-9]+\)[[:space:]]+([0-9A-F]{40}).*/\1/')" || true
    [[ -n "${hashes}" ]] || die "no 'Developer ID Application' identity in the keychain. Set APW_CODESIGN_IDENTITY."
    local count
    count="$(printf '%s\n' "${hashes}" | wc -l | tr -d ' ')"
    IDENTITY="$(printf '%s\n' "${hashes}" | tail -1)"
    if [[ "${count}" -gt 1 ]]; then
        echo "WARNING: ${count} 'Developer ID Application' identities match; signing by name is ambiguous." >&2
        security find-identity -v -p codesigning | grep 'Developer ID Application' >&2
        echo "WARNING: using the last one. Override with APW_CODESIGN_IDENTITY=<sha1>." >&2
    fi
    echo "Signing identity: ${IDENTITY}"
}

# ------------------------------------------------------------------- build ---
fmt_artefact() {
    echo "${ARTEFACT_DIR}/$(apw_fmt_artefact_subdir "$1")/$(apw_fmt_bundle_name "$1")"
}

# Configure flags and build targets for the selected formats. AUv3 needs the Xcode generator
# and a Standalone container app, which this script's Makefile/Ninja build cannot produce, so
# it is only picked up when a prebuilt container is already in ${ARTEFACT_DIR}/Standalone.
format_cmake_flags() {
    local fmt
    for fmt in "${APW_FORMATS[@]}"; do
        case "${fmt}" in
            clap) echo "-DAPW_BUILD_CLAP=ON" ;;
            lv2) echo "-DAPW_BUILD_LV2=ON" ;;
            vst2) [[ -n "${APW_VST2_SDK_PATH:-}" ]] || die "vst2 needs APW_VST2_SDK_PATH"
                  echo "-DAPW_VST2_SDK_PATH=${APW_VST2_SDK_PATH}" ;;
            aax) [[ -n "${APW_AAX_SDK_PATH:-}" ]] || die "aax needs APW_AAX_SDK_PATH"
                 echo "-DAPW_AAX_SDK_PATH=${APW_AAX_SDK_PATH}" ;;
        esac
    done
}
format_build_targets() {
    local fmt
    for fmt in "${APW_FORMATS[@]}"; do
        [[ "${fmt}" == auv3 ]] && continue
        echo "AudioProvenanceCapture_$(apw_fmt_artefact_subdir "${fmt}")"
    done
}

# die inside the process substitutions below would not stop the script, so SDK paths are
# checked here in the main shell before any configure.
validate_format_inputs() {
    local fmt
    for fmt in "${APW_FORMATS[@]}"; do
        case "${fmt}" in
            vst2) [[ -d "${APW_VST2_SDK_PATH:-}" ]] || die "vst2 needs APW_VST2_SDK_PATH pointing at the legacy Steinberg SDK" ;;
            aax) [[ -d "${APW_AAX_SDK_PATH:-}" ]] || die "aax needs APW_AAX_SDK_PATH pointing at the Avid AAX SDK" ;;
        esac
    done
}

build_plugins() {
    validate_format_inputs
    local stamp="${ARTEFACT_DIR}/${SOURCE_REVISION_FILE}" cached="" fmt all_present=1
    SOURCE_REVISION="$(source_revision)"
    for fmt in "${APW_FORMATS[@]}"; do
        [[ -e "$(fmt_artefact "${fmt}")" ]] || all_present=0
    done
    [[ -f "${stamp}" ]] && cached="$(cat "${stamp}")"
    if [[ "${all_present}" == 1 && "${APW_FORCE_BUILD:-0}" != "1" && "${cached}" == "${SOURCE_REVISION}" ]]; then
        echo "Reusing the build in ${ARTEFACT_DIR} at source revision ${SOURCE_REVISION} (APW_FORCE_BUILD=1 to rebuild)."
    else
        if [[ "${all_present}" == 1 && "${APW_FORCE_BUILD:-0}" != "1" ]]; then
            echo "Cached build is at source revision ${cached:-none}, but src/ + CMakeLists.txt now hash to ${SOURCE_REVISION}."
            echo "Rebuilding rather than shipping a stale binary."
        fi
        step "Building universal ${APW_FORMATS[*]}"
        local -a flags targets
        while IFS= read -r line; do [[ -n "${line}" ]] && flags+=("${line}"); done < <(format_cmake_flags)
        while IFS= read -r line; do [[ -n "${line}" ]] && targets+=("${line}"); done < <(format_build_targets)
        cmake -S "${PROJECT_ROOT}" -B "${BUILD_DIR}" \
            -DCMAKE_BUILD_TYPE="${BUILD_TYPE}" \
            -DCMAKE_OSX_ARCHITECTURES="arm64;x86_64" \
            -DCMAKE_OSX_DEPLOYMENT_TARGET=11.0 \
            ${flags[@]+"${flags[@]}"} \
            || die "cmake configure failed"
        cmake --build "${BUILD_DIR}" --config "${BUILD_TYPE}" \
            --target ${targets[@]+"${targets[@]}"} \
            || die "cmake build failed"
    fi
    for fmt in "${APW_FORMATS[@]}"; do
        [[ -e "$(fmt_artefact "${fmt}")" ]] || die "${fmt} artefact missing: $(fmt_artefact "${fmt}")"
    done
    printf '%s\n' "${SOURCE_REVISION}" > "${stamp}"
    local exe
    for fmt in "${APW_FORMATS[@]}"; do
        apw_fmt_is_plain_dir "${fmt}" && continue
        exe="$(fmt_main_binary "$(fmt_artefact "${fmt}")")" \
            || die "${fmt}: no Mach-O executable found in $(fmt_artefact "${fmt}")"
        assert_universal "${exe}"
    done
}

# The main Mach-O of a bundle, or, for a plain directory, the first Mach-O inside it.
fmt_main_binary() {
    local bundle="$1" f
    if [[ -d "${bundle}/Contents/MacOS" ]]; then
        f="$(find "${bundle}/Contents/MacOS" -type f | head -1)"
    else
        f="$(find "${bundle}" -type f \( -name '*.so' -o -name '*.dylib' \) | head -1)"
    fi
    [[ -n "${f}" ]] || return 1
    printf '%s\n' "${f}"
}

assert_universal() {
    local f="$1" archs
    archs="$(lipo -archs "$1" 2>/dev/null)" || die "not a Mach-O file: ${f}"
    [[ "${archs}" == *arm64* ]] || die "${f} has no arm64 slice (got: ${archs})"
    [[ "${archs}" == *x86_64* ]] || die "${f} has no x86_64 slice (got: ${archs})"
    echo "universal ok: ${archs}  ${f#${PROJECT_ROOT}/}"
}

# ------------------------------------------------------------------ freeze ---
prepare_freeze_venv() {
    step "Preparing the universal2 freeze environment"
    [[ -x "${FREEZE_PYTHON}" ]] \
        || die "universal2 CPython not found at ${FREEZE_PYTHON}. Install the python.org 3.12 framework build or set APW_FREEZE_PYTHON."
    local pyarchs
    pyarchs="$(lipo -archs "${FREEZE_PYTHON}" 2>/dev/null || true)"
    [[ "${pyarchs}" == *arm64* && "${pyarchs}" == *x86_64* ]] \
        || die "${FREEZE_PYTHON} is not universal2 (archs: ${pyarchs:-unknown}). A universal daemon cannot be frozen from it."

    if [[ ! -x "${FREEZE_VENV}/bin/python" ]]; then
        "${FREEZE_PYTHON}" -m venv "${FREEZE_VENV}" || die "could not create the freeze venv"
    fi
    "${FREEZE_VENV}/bin/python" -m pip install --quiet --upgrade pip >/dev/null \
        || die "pip self-upgrade failed in the freeze venv"
    "${FREEZE_VENV}/bin/python" -m pip install --quiet "pyinstaller>=6.10" \
        || die "PyInstaller install failed"
    # pip --target refuses to overwrite an existing tree, and the default arm64 wheel
    # is what gets pulled in as a PyInstaller dependency, so clear it first.
    local site="${FREEZE_VENV}/lib/python3.12/site-packages"
    rm -rf "${site:?}/cryptography"
    find "${site}" -maxdepth 1 -name 'cryptography-*.dist-info' -exec rm -rf {} +
    "${FREEZE_VENV}/bin/python" -m pip install --quiet --no-deps \
        --only-binary=:all: --platform macosx_10_12_universal2 \
        --target "${site}" \
        "cryptography==${CRYPTOGRAPHY_PIN}" \
        || die "could not install the universal2 cryptography ${CRYPTOGRAPHY_PIN} wheel"

    "${FREEZE_VENV}/bin/python" -m pip install --quiet pycparser \
        || die "could not install pycparser (cffi's pure-Python dependency)"
    rm -rf "${BUILD_DIR:?}/fatwheel"
    install_universal_dist "cffi==${CFFI_PIN}" macosx_11_0_x86_64
    install_universal_dist "c2pa-python==${C2PA_PIN}" macosx_10_9_x86_64
    install_universal_dist "cbor2==${CBOR2_PIN}" macosx_10_9_x86_64

    assert_universal "${site}/cryptography/hazmat/bindings/_rust.abi3.so"
    assert_universal "${site}/_cffi_backend.cpython-312-darwin.so"
    assert_universal "${site}/c2pa/libs/libc2pa_c.dylib"
    assert_universal "${site}/cbor2/_cbor2.cpython-312-darwin.so"
    ( cd "${PROJECT_ROOT}" && "${FREEZE_VENV}/bin/python" -c \
        "import cryptography, cbor2, c2pa, daemon.__main__, daemon.c2pa_engine.identity, daemon.c2pa_engine.signer, daemon.c2pa_engine.verifier" ) \
        || die "the freeze venv cannot import the full daemon + C2PA engine graph"
}

cross_build_x86_wheel() {
    local spec="$1" out="$2"
    command -v rustup >/dev/null \
        || die "${spec} publishes no x86_64 macOS wheel and rustup is not installed, so the x86_64 slice cannot be built. Install rustup and 'rustup target add x86_64-apple-darwin'."
    rustup target list --installed 2>/dev/null | grep -qx 'x86_64-apple-darwin' \
        || die "${spec} needs an x86_64 build and the x86_64-apple-darwin Rust target is missing. Run: rustup target add x86_64-apple-darwin"
    echo "  no prebuilt x86_64 wheel for ${spec}; cross-building it from source"
    # IMPORTANT: rustc bakes absolute source paths into panic-location strings, so a
    # wheel cross-built here ships the developer's home directory to every recipient.
    ( CARGO_BUILD_TARGET=x86_64-apple-darwin \
      _PYTHON_HOST_PLATFORM=macosx-10.9-x86_64 \
      ARCHFLAGS="-arch x86_64" \
      RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=${HOME}=/build" \
      "${FREEZE_VENV}/bin/python" -m pip wheel --quiet --no-deps --no-binary=:all: \
          -w "${out}/wheel" "${spec}" ) \
        || die "could not cross-build an x86_64 wheel for ${spec}"
    # pip refuses to --target-install a wheel whose platform tag is not this host's,
    # and a wheel is a zip, so the cross-built tree is unpacked directly.
    local built
    built="$(find "${out}/wheel" -maxdepth 1 -name '*.whl' -print -quit)"
    [[ -n "${built}" ]] || die "the cross-build of ${spec} produced no wheel"
    unzip -q -o "${built}" -d "${out}" || die "could not unpack the cross-built ${spec} wheel"
}

# Install one distribution into the freeze venv as a universal2 tree: stage the arm64
# wheel, then lipo every Mach-O it contains against its x86_64 counterpart. Used for
# every dependency that publishes thin per-architecture macOS wheels.
install_universal_dist() {
    local spec="$1" x86_platform="$2"
    local site="${FREEZE_VENV}/lib/python3.12/site-packages"
    local slug="${spec//[^A-Za-z0-9]/_}"
    local tmp="${FATWHEEL_DIR}/${slug}"

    mkdir -p "${tmp}/arm64" "${tmp}/x86_64"
    "${FREEZE_VENV}/bin/python" -m pip install --quiet --no-deps --only-binary=:all: \
        --platform macosx_11_0_arm64 --python-version 3.12 \
        --target "${tmp}/arm64" "${spec}" \
        || die "could not fetch the arm64 wheel for ${spec}"
    if ! "${FREEZE_VENV}/bin/python" -m pip install --quiet --no-deps --only-binary=:all: \
        --platform "${x86_platform}" --python-version 3.12 \
        --target "${tmp}/x86_64" "${spec}" 2>/dev/null; then
        cross_build_x86_wheel "${spec}" "${tmp}/x86_64"
    fi

    # Rust build leftovers ship in the c2pa wheel: never loaded, thin, 28 MB each.
    find "${tmp}" -type f \( -name '*.rlib' -o -name '*.d' \) -delete

    local entry
    for entry in "${tmp}"/arm64/*; do
        find "${site}" -maxdepth 1 -name "$(basename "${entry}")" -exec rm -rf {} +
    done
    ditto "${tmp}/arm64" "${site}" || die "could not stage ${spec} into the freeze venv"

    local rel merged=0
    while IFS= read -r rel; do
        [[ -f "${tmp}/x86_64/${rel}" ]] \
            || die "${spec} ships ${rel} for arm64 but not for x86_64; the daemon would be thin on Intel"
        lipo -create "${tmp}/arm64/${rel}" "${tmp}/x86_64/${rel}" -output "${site}/${rel}" \
            || die "lipo could not merge the two ${rel} slices of ${spec}"
        merged=$((merged + 1))
    done < <(cd "${tmp}/arm64" && find . -type f \( -name '*.so' -o -name '*.dylib' \) -print \
                 | sed 's|^\./||')
    echo "universal ${spec}: ${merged} binaries merged"
}

freeze_daemon() {
    step "Freezing the provenance daemon (universal2, onedir)"
    rm -rf "${FREEZE_WORK}"
    mkdir -p "${FREEZE_WORK}"
    # IMPORTANT: PyInstaller collected a stale daemon/**/__pycache__/*.cpython-312.pyc
    # in place of its newer source, and froze a daemon that died at startup on a
    # missing method. Purge the cache and never write a new one during the freeze.
    find "${PROJECT_ROOT}/daemon" "${PROJECT_ROOT}/packaging" -type d -name __pycache__ \
        -exec rm -rf {} + 2>/dev/null || true
    ( cd "${PROJECT_ROOT}" && PYTHONDONTWRITEBYTECODE=1 "${FREEZE_VENV}/bin/python" -m PyInstaller \
        --noconfirm --clean --log-level WARN \
        --name "${DAEMON_DIR_NAME}" \
        --distpath "${FREEZE_WORK}/dist" \
        --workpath "${FREEZE_WORK}/work" \
        --specpath "${FREEZE_WORK}" \
        --paths "${PROJECT_ROOT}" \
        --collect-submodules daemon \
        --collect-all c2pa \
        --hidden-import aifc \
        --hidden-import audioop \
        --target-arch universal2 \
        --console \
        "${PROJECT_ROOT}/packaging/apw_daemon_main.py" ) \
        || die "PyInstaller failed to freeze the daemon"

    FROZEN_DIR="${FREEZE_WORK}/dist/${DAEMON_DIR_NAME}"
    [[ -x "${FROZEN_DIR}/${DAEMON_DIR_NAME}" ]] || die "frozen daemon executable missing: ${FROZEN_DIR}/${DAEMON_DIR_NAME}"

    local thin=0 f archs
    while IFS= read -r f; do
        archs="$(lipo -archs "${f}" 2>/dev/null || true)"
        if [[ "${archs}" != *arm64* || "${archs}" != *x86_64* ]]; then
            echo "WARNING: thin binary in the frozen daemon: ${archs:-unknown}  ${f#${FROZEN_DIR}/}" >&2
            thin=$((thin + 1))
        fi
    done < <(macho_files "${FROZEN_DIR}")
    if [[ "${thin}" -gt 0 ]]; then
        die "${thin} binaries in the frozen daemon are not universal; it would fail on Intel. Refusing to ship it silently."
    fi
    echo "frozen daemon is universal: ${FROZEN_DIR}"

    local frozen_src
    frozen_src="$("${FROZEN_DIR}/${DAEMON_DIR_NAME}" --module-fingerprint 2>/dev/null || true)"
    local repo_src
    repo_src="$( cd "${PROJECT_ROOT}" && PYTHONPATH="${PROJECT_ROOT}" PYTHONDONTWRITEBYTECODE=1 \
        "${FREEZE_VENV}/bin/python" packaging/apw_daemon_main.py --module-fingerprint 2>/dev/null || true)"
    [[ -n "${frozen_src}" && "${frozen_src}" == "${repo_src}" ]] \
        || die "the frozen daemon module graph does not match the repository source (frozen=${frozen_src:-none}, repo=${repo_src:-none}). It was built from stale bytecode."
    echo "frozen module graph matches the repository source: ${frozen_src}"
}

# --------------------------------------------------------------- identity ---
# REQUIRED: the daemon signs C2PA claims with material issued by whatever provenance
# store it is pointed at. A trust anchor that is not that store's own root makes every
# signature the recipient produces read as untrusted, so the anchor is derived from the
# store that ships beside it and the two are checked for equality on every build.
prepare_demo_identity() {
    step "Preparing the shipped demo signing identity"
    local store="${ASSET_DIR}/${DEMO_STORE_NAME}" anchor="${ASSET_DIR}/demo-root-ca.pem"
    mkdir -p "${ASSET_DIR}"
    ( cd "${PROJECT_ROOT}" && "${FREEZE_VENV}/bin/python" -c '
import sys
from pathlib import Path
from daemon.provenance import LocalReferenceProvider
LocalReferenceProvider(Path(sys.argv[1]), common_name="APW Demo Signer (not a verified identity)")
' "${store}" ) || die "could not create the demo provenance store at ${store}"

    [[ -f "${store}/ca/root_cert.pem" ]] || die "${store} has no ca/root_cert.pem"
    cat "${store}/ca/root_cert.pem" > "${anchor}" || die "could not write ${anchor}"

    local store_fp anchor_fp
    store_fp="$(openssl x509 -in "${store}/ca/root_cert.pem" -noout -fingerprint -sha256)"
    anchor_fp="$(openssl x509 -in "${anchor}" -noout -fingerprint -sha256)"
    [[ "${store_fp}" == "${anchor_fp}" ]] \
        || die "the shipped trust anchor is not the demo store's root (${anchor_fp} != ${store_fp})."
    echo "Demo trust anchor matches the shipped store root: ${store_fp}"

    sign_demo_clip "${store}" "${anchor}"
}

# The canned clip in the demo project has to verify against the same anchor, or the
# one asset a recipient can check without recording anything contradicts the guide.
sign_demo_clip() {
    local store="$1" anchor="$2"
    local samples="${ASSET_DIR}/demo-project/apw-demo Project/Samples/Imported"
    [[ -f "${samples}/apw-demo-unsigned.wav" ]] \
        || die "${samples}/apw-demo-unsigned.wav is missing; the signed demo clip cannot be produced."
    ( cd "${PROJECT_ROOT}" && "${FREEZE_VENV}/bin/python" -c '
import sys
from pathlib import Path
from daemon.c2pa_engine.manifest import ManifestSpec, build_manifest
from daemon.c2pa_engine.signer import build_signer, detect_format, sign_asset
from daemon.c2pa_engine.verifier import verify_asset
from daemon.provenance import LocalReferenceProvider, VerificationState

store, source, dest, anchor = (Path(a) for a in sys.argv[1:5])
material = LocalReferenceProvider(store, common_name="APW Demo Signer (not a verified identity)").issue_signing_material()
signer = build_signer(material.certificate_chain_pem, material.private_key_handle, material.algorithm)
asset = detect_format(source)
result = sign_asset(source, dest, build_manifest(ManifestSpec(title=dest.name, mime=asset.mime)), signer)
check = verify_asset(result.asset_path, result.mime, trust_anchors_pem=anchor.read_bytes())
if check.state != VerificationState.VERIFIED.value or check.failure_codes:
    raise SystemExit(f"signed demo clip does not verify against the shipped anchor: {check.state} {check.failure_codes}")
print(f"demo clip signed and verified against the shipped anchor: {dest.name}")
' "${store}" "${samples}/apw-demo-unsigned.wav" "${samples}/apw-demo-signed.wav" "${anchor}" ) \
        || die "could not sign and verify the demo clip against the shipped anchor"
}

# ------------------------------------------------------------------- stage ---
macho_files() {
    # Deepest path first so nested code is signed before its container. `file` is not
    # usable here: it pads its output for multiple arguments and prints an extra line
    # per slice of a fat binary, so paths containing spaces cannot be recovered.
    local f slashes
    find "$1" -type f -print0 | while IFS= read -r -d '' f; do
        lipo -archs "${f}" >/dev/null 2>&1 || continue
        slashes="${f//[^\/]/}"
        printf '%s\t%s\n' "${#slashes}" "${f}"
    done | sort -rn -k1,1 | cut -f2-
}

stage_bundles() {
    step "Staging the DMG payload"
    rm -rf "${STAGE_DIR}"
    mkdir -p "${DMG_STAGE}"

    local fmt staged bundle
    for fmt in "${APW_FORMATS[@]}"; do
        staged="$(apw_fmt_staged_path "${fmt}")"
        mkdir -p "$(dirname "${staged}")"
        ditto "$(fmt_artefact "${fmt}")" "${staged}" || die "could not stage the ${fmt} format"
        # Bundles carry the frozen daemon so a plug-in loaded from any format can find it.
        # Plain directories (LV2) have no Contents/ and rely on the daemon pkg component.
        if [[ -d "${staged}/Contents" ]]; then
            rm -rf "${staged}/Contents/Resources/${DAEMON_DIR_NAME}"
            mkdir -p "${staged}/Contents/Resources"
            ditto "${FROZEN_DIR}" "${staged}/Contents/Resources/${DAEMON_DIR_NAME}" \
                || die "could not inject the frozen daemon into ${staged}"
            printf '%s\n' "${SOURCE_REVISION}" > "${staged}/Contents/Resources/apw-source-revision.txt"
        fi
    done
    # The DMG needs both of its formats. A pkg-only selection leaves the DMG unbuilt.
    if [[ ! -d "${DMG_STAGE}/${VST3_NAME}" || ! -d "${DMG_STAGE}/${AU_NAME}" ]]; then
        DMG_ENABLED=0
    fi
    printf '%s\n' "${APW_FORMATS[@]}" > "${STAGE_DIR}/formats.txt"

    # Drag-drop targets. These system directories need admin rights, so Finder authenticates on
    # drop; "Install Plug-Ins.command" stays as the non-interactive path for anyone it refuses.
    if [[ "${DMG_ENABLED}" == 1 ]]; then
        ln -sfn "/Library/Audio/Plug-Ins/VST3" "${DMG_STAGE}/VST3" \
            || die "could not create the VST3 drop symlink"
        ln -sfn "/Library/Audio/Plug-Ins/Components" "${DMG_STAGE}/Components" \
            || die "could not create the Components drop symlink"
    fi

    # REQUIRED: stock macOS ships no /Library/Audio/Plug-Ins/VST3, and a disk image
    # cannot create one, so a drag-and-drop alias to it dangles on every recipient's
    # machine. The installer script creates both destinations before it copies.
    local script
    for script in "${DMG_SCRIPTS[@]}"; do
        [[ -f "${DMG_SCRIPT_DIR}/${script}" ]] || die "missing DMG payload script: ${DMG_SCRIPT_DIR}/${script}"
        ditto "${DMG_SCRIPT_DIR}/${script}" "${DMG_STAGE}/${script}" \
            || die "could not stage ${script}"
        chmod 755 "${DMG_STAGE}/${script}"
    done

    [[ -d "${ASSET_DIR}" ]] \
        || die "${ASSET_DIR} is missing. The DMG must not ship without the QuickStart, the trust anchor and the demo project."
    local asset
    for asset in "${REQUIRED_ASSETS[@]}"; do
        [[ -e "${ASSET_DIR}/${asset}" ]] \
            || die "required asset missing: ${ASSET_DIR}/${asset}. Refusing to build a DMG that silently lacks it."
    done
    ditto "${ASSET_DIR}" "${DMG_STAGE}/Documentation and Demo Project" \
        || die "could not stage packaging/assets"
    echo "Staged assets from ${ASSET_DIR}:"
    ls -1 "${ASSET_DIR}"

    assert_no_broken_links "${DMG_STAGE}"
}

# IMPORTANT: the two plug-in drop targets are absolute links resolved on the RECIPIENT's Mac, so
# build-host resolution says nothing about them. Every other link must resolve here, since a
# dangling one means a staging bug.
assert_no_broken_links() {
    local link broken=0 relative
    while IFS= read -r link; do
        [[ -e "${link}" ]] && continue
        relative="${link#${DMG_STAGE}/}"
        if [[ "${relative}" == "VST3" || "${relative}" == "Components" ]]; then
            echo "NOTE: ${relative} points at $(readlink "${link}"), absent on this build host. It \
resolves on any Mac that already has one; where it does not, \"Install Plug-Ins.command\" installs \
to the user scope instead and needs no admin rights." >&2
            continue
        fi
        echo "ERROR: dangling link in the DMG payload: ${link#${STAGE_DIR}/}" >&2
        broken=$((broken + 1))
    done < <(find "$1" -type l)
    [[ "${broken}" -eq 0 ]] || die "${broken} links in the DMG payload resolve to nothing on this machine."
}

# -------------------------------------------------------------------- sign ---
sign_one() {
    local target="$1" ents="${2:-}"
    if [[ -n "${ents}" ]]; then
        codesign --force --options runtime --timestamp --entitlements "${ents}" \
            --sign "${IDENTITY}" "${target}"
    else
        codesign --force --options runtime --timestamp --sign "${IDENTITY}" "${target}"
    fi || die "codesign failed on ${target}. A --timestamp failure is usually Apple's timestamp server being unreachable; re-run."
}

sign_bundle_inside_out() {
    local bundle="$1" f count=0
    echo "Signing nested binaries in ${bundle##*/}"
    while IFS= read -r f; do
        if [[ "${f}" == */"${DAEMON_DIR_NAME}"/"${DAEMON_DIR_NAME}" ]]; then
            sign_one "${f}" "${DAEMON_ENTITLEMENTS}"
        else
            sign_one "${f}"
        fi
        count=$((count + 1))
    done < <(macho_files "${bundle}")
    echo "  signed ${count} nested Mach-O files"
    sign_one "${bundle}"
    codesign --verify --deep --strict --verbose=2 "${bundle}" \
        || die "codesign verification failed for ${bundle}"
}

# REQUIRED: hardened runtime without allow-unsigned-executable-memory denies libffi the
# W+X mapping ctypes needs for a closure. libffi's x86_64 fallback then retries mkostemp
# in open_temp_exec_file_dir forever, so the frozen daemon livelocks at 100% CPU the
# first time it builds a C2PA signer. arm64 libffi uses static trampolines and never
# hits it, so this is invisible until an Intel host loads the plug-in.
sign_stage() {
    step "Signing inside-out with ${IDENTITY}"
    [[ -f "${DAEMON_ENTITLEMENTS}" ]] \
        || die "missing ${DAEMON_ENTITLEMENTS}; the frozen daemon would be signed without allow-unsigned-executable-memory and livelock on x86_64."
    local fmt staged
    for fmt in "${APW_FORMATS[@]}"; do
        staged="$(apw_fmt_staged_path "${fmt}")"
        if apw_fmt_is_plain_dir "${fmt}"; then
            sign_plain_dir "${staged}"
        else
            sign_bundle_inside_out "${staged}"
        fi
    done
}

# A plain directory (LV2) cannot carry a bundle signature, so each Mach-O in it is signed
# and verified on its own. It cannot be stapled either; notarization covers it via the pkg.
sign_plain_dir() {
    local dir="$1" f count=0
    echo "Signing Mach-O files in ${dir##*/}"
    while IFS= read -r f; do
        sign_one "${f}"
        codesign --verify --strict "${f}" || die "codesign verification failed for ${f}"
        count=$((count + 1))
    done < <(macho_files "${dir}")
    [[ "${count}" -gt 0 ]] || die "no Mach-O files found in ${dir}; nothing to sign"
    echo "  signed ${count} Mach-O files"
}

# --------------------------------------------------------------- run check ---
check_slices_run() {
    step "Executing both slices of the frozen daemon"
    local exe="" fmt cand
    for fmt in "${APW_FORMATS[@]}"; do
        cand="$(apw_fmt_staged_path "${fmt}")/Contents/Resources/${DAEMON_DIR_NAME}/${DAEMON_DIR_NAME}"
        [[ -x "${cand}" ]] && { exe="${cand}"; break; }
    done
    [[ -n "${exe}" ]] || die "no selected format carries the frozen daemon; select a bundle format (vst3, au, clap, ...)"
    exercise_slice arm64 "${exe}" || die "the arm64 slice of the frozen daemon failed its gate"

    # IMPORTANT: an absent Rosetta 2 and a broken x86_64 slice are different facts.
    # Conflating them is how a daemon that cannot sign on Intel ships unnoticed.
    if ! arch -x86_64 /usr/bin/true >/dev/null 2>&1; then
        echo "WARNING: Rosetta 2 is absent, so the x86_64 slice was NOT executed." >&2
        echo "WARNING: the slice is present, universal and signed, but UNVERIFIED on Intel." >&2
        echo "WARNING: install Rosetta 2 (softwareupdate --install-rosetta) to close this gap." >&2
        return
    fi
    exercise_slice x86_64 "${exe}" \
        || die "Rosetta 2 is present and the x86_64 slice failed its gate; it would fail the same way on an Intel Mac."
}

# --self-test covers native crypto, --help forces the full daemon import graph the
# self-test short-circuits past, and --c2pa-self-test signs and verifies a real WAV.
# The last one is the only gate that reaches the lazily imported C2PA engine, which is
# where a frozen daemon silently loses the headline feature.
exercise_slice() {
    local slice="$1" exe="$2"
    arch "-${slice}" "${exe}" --self-test || return 1
    arch "-${slice}" "${exe}" --help >/dev/null || return 1
    arch "-${slice}" "${exe}" --c2pa-self-test || return 1
    echo "${slice}: crypto, import graph and C2PA sign+verify all OK"
}

# --------------------------------------------------------------------- dmg ---
build_dmg() {
    step "Building the DMG"
    mkdir -p "${DIST_DIR}"
    rm -f "${DMG_PATH}"
    command -v create-dmg >/dev/null || die "create-dmg not found on PATH"
    if ! create-dmg \
        --volname "Audio Provenance Capture" \
        --window-size 720 460 \
        --icon-size 96 \
        --no-internet-enable \
        --hdiutil-quiet \
        "${DMG_PATH}" "${DMG_STAGE}"; then
        echo "WARNING: create-dmg failed (its Finder AppleScript step needs a UI session); falling back to hdiutil." >&2
        rm -f "${DMG_PATH}"
        hdiutil create -volname "Audio Provenance Capture" -srcfolder "${DMG_STAGE}" \
            -ov -format UDZO "${DMG_PATH}" >/dev/null \
            || die "hdiutil could not create the DMG either"
    fi
    [[ -f "${DMG_PATH}" ]] || die "the DMG was not produced at ${DMG_PATH}"

    codesign --force --timestamp --sign "${IDENTITY}" "${DMG_PATH}" \
        || die "could not sign the DMG"
    codesign --verify --strict --verbose=2 "${DMG_PATH}" \
        || die "DMG signature verification failed"
    echo "DMG: ${DMG_PATH}"
}

main() {
    # notarize.sh staples the staged bundles, then rebuilds the image around them: a
    # ticket stapled to the DMG never reaches a plug-in dragged out of it.
    if [[ "${1:-}" == "--dmg-only" ]]; then
        [[ -d "${DMG_STAGE}/${VST3_NAME}" && -d "${DMG_STAGE}/${AU_NAME}" ]] \
            || die "--dmg-only needs an existing staged payload in ${DMG_STAGE}. Run without it first."
        resolve_identity
        assert_no_broken_links "${DMG_STAGE}"
        build_dmg
        step "Result"
        echo "DMG rebuilt from the staged (stapled) bundles: ${DMG_PATH}"
        return
    fi
    resolve_identity
    resolve_pins
    build_plugins
    prepare_freeze_venv
    prepare_demo_identity
    freeze_daemon
    stage_bundles
    sign_stage
    check_slices_run
    step "Result"
    echo "Formats        : ${APW_FORMATS[*]}"
    if [[ "${DMG_ENABLED}" == 1 ]]; then
        build_dmg
        echo "DMG            : ${DMG_PATH}"
        echo "Size           : $(du -h "${DMG_PATH}" | cut -f1)"
    else
        echo "DMG            : skipped (needs both vst3 and au); build the pkg with scripts/package_pkg.sh"
    fi
    echo "Signed with    : ${IDENTITY}"
    echo "Source revision: ${SOURCE_REVISION}"
    echo "Engine pins    : c2pa-python==${C2PA_PIN}, cbor2==${CBOR2_PIN}"
    echo "Next           : ${SCRIPT_DIR}/notarize.sh \"${DMG_PATH}\""
}

main "$@"
