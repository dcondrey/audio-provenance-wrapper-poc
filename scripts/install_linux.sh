#!/usr/bin/env bash
# Install or remove Audio Provenance Capture plug-ins on Linux, one format at a time.
#
# Payload layout (what `cmake --install . --prefix <payload>` produces, see cmake/install.cmake):
#   <payload>/lib/vst3/Audio Provenance Capture.vst3
#   <payload>/lib/clap/Audio Provenance Capture.clap
#   <payload>/lib/lv2/Audio Provenance Capture.lv2
#   <payload>/lib/ladspa/*   <payload>/lib/dssi/*      (only when the build produced them)
#   <payload>/SHA256SUMS                               (written by --write-checksums)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALL_FORMATS=(vst3 clap lv2 ladspa dssi)
PRODUCT="Audio Provenance Capture"

usage() {
    cat <<USAGE
Usage: $(basename "$0") [options]

  --formats LIST       comma separated subset of: ${ALL_FORMATS[*]}   (default: vst3)
  --user               install under \$HOME (~/.vst3 ~/.clap ~/.lv2 ~/.ladspa ~/.dssi)   [default]
  --system             install under /usr/lib/{vst3,clap,lv2,ladspa,dssi}
  --prefix DIR         install under DIR/lib/{vst3,clap,lv2,ladspa,dssi}
  --payload DIR        staged payload to install from (default: the directory above this script,
                       or ./payload next to it if that exists)
  --uninstall          remove what an earlier install put in the chosen destinations. With no
                       --formats, every format recorded in the receipt is removed
  --dry-run            print every action, change nothing
  --write-checksums    (re)write DIR/SHA256SUMS for --payload DIR and exit
  --no-verify          skip checksum verification (refused unless APW_ALLOW_UNVERIFIED=1)
  -h, --help
USAGE
}

die() { echo "ERROR: $*" >&2; exit 1; }
say() { echo "$*"; }

formats_arg=""
scope="user"
prefix=""
payload=""
uninstall=0
dry=0
write_sums=0
verify=1

while [[ $# -gt 0 ]]; do
    case "$1" in
        --formats) [[ $# -ge 2 ]] || die "--formats needs a value"; formats_arg="$2"; shift 2 ;;
        --formats=*) formats_arg="${1#*=}"; shift ;;
        --user) scope="user"; shift ;;
        --system) scope="system"; shift ;;
        --prefix) [[ $# -ge 2 ]] || die "--prefix needs a value"; scope="prefix"; prefix="$2"; shift 2 ;;
        --prefix=*) scope="prefix"; prefix="${1#*=}"; shift ;;
        --payload) [[ $# -ge 2 ]] || die "--payload needs a value"; payload="$2"; shift 2 ;;
        --payload=*) payload="${1#*=}"; shift ;;
        --uninstall) uninstall=1; shift ;;
        --dry-run) dry=1; shift ;;
        --write-checksums) write_sums=1; shift ;;
        --no-verify) verify=0; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
done

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
    else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

verb() { if [[ "${dry}" == 1 ]]; then echo "would $1"; else echo "$2"; fi; }

is_known() { local f; for f in "${ALL_FORMATS[@]}"; do [[ "$f" == "$1" ]] && return 0; done; return 1; }

parse_formats() {
    SELECTED=()
    local raw="${1//,/ }" tok
    for tok in ${raw}; do
        tok="$(printf '%s' "${tok}" | tr '[:upper:]' '[:lower:]')"
        is_known "${tok}" || die "unknown format '${tok}'. Known: ${ALL_FORMATS[*]}"
        local dup=0 s
        for s in ${SELECTED[@]+"${SELECTED[@]}"}; do [[ "$s" == "${tok}" ]] && dup=1; done
        [[ "${dup}" == 1 ]] || SELECTED+=("${tok}")
    done
}

dest_dir() {
    local fmt="$1"
    case "${scope}" in
        user) echo "${HOME:?HOME is not set}/.${fmt}" ;;
        system) echo "/usr/lib/${fmt}" ;;
        prefix) echo "${prefix}/lib/${fmt}" ;;
    esac
}

receipt_path() {
    case "${scope}" in
        user) echo "${XDG_DATA_HOME:-${HOME:?HOME is not set}/.local/share}/audio-provenance-capture/install-receipt.txt" ;;
        system) echo "/var/lib/audio-provenance-capture/install-receipt.txt" ;;
        prefix) echo "${prefix}/share/audio-provenance-capture/install-receipt.txt" ;;
    esac
}

run() {
    if [[ "${dry}" == 1 ]]; then say "[dry-run] $*"; else "$@"; fi
}

if [[ "${scope}" == prefix ]]; then
    [[ -n "${prefix}" ]] || die "--prefix needs a directory"
    case "${prefix}" in /) die "refusing --prefix /; use --system" ;; esac
fi

# ------------------------------------------------------------------ payload ---
if [[ -z "${payload}" ]]; then
    if [[ -d "${SCRIPT_DIR}/payload" ]]; then payload="${SCRIPT_DIR}/payload"
    else payload="$(cd "${SCRIPT_DIR}/.." && pwd)"; fi
fi

payload_files() { # relative paths of every regular file/symlink under lib/, sorted
    (cd "$1" && find lib \( -type f -o -type l \) -print | LC_ALL=C sort)
}

if [[ "${write_sums}" == 1 ]]; then
    [[ -d "${payload}/lib" ]] || die "${payload}/lib does not exist; nothing to checksum"
    tmp="${payload}/SHA256SUMS.tmp.$$"
    : > "${tmp}"
    while IFS= read -r rel; do
        [[ -L "${payload}/${rel}" ]] && die "symlink in payload: ${rel}; payloads must be plain files"
        printf '%s  %s\n' "$(sha256_of "${payload}/${rel}")" "${rel}" >> "${tmp}"
    done < <(payload_files "${payload}")
    mv "${tmp}" "${payload}/SHA256SUMS"
    say "Wrote ${payload}/SHA256SUMS ($(wc -l < "${payload}/SHA256SUMS" | tr -d ' ') files)"
    exit 0
fi

# Every listed file of a selected format must hash to its recorded value, and the format's
# directory must contain nothing that is not listed.
verify_format() {
    local fmt="$1" sums="${payload}/SHA256SUMS" line want rel n=0 listed
    [[ -f "${sums}" ]] || die "${sums} is missing; write it with --write-checksums, or pass --no-verify with APW_ALLOW_UNVERIFIED=1"
    while IFS= read -r line; do
        want="${line%%  *}"; rel="${line#*  }"
        [[ "${rel}" == lib/"${fmt}"/* ]] || continue
        [[ -f "${payload}/${rel}" ]] || die "checksum: ${rel} is listed but missing from the payload"
        [[ "$(sha256_of "${payload}/${rel}")" == "${want}" ]] || die "checksum mismatch: ${rel}"
        n=$((n + 1))
    done < "${sums}"
    [[ "${n}" -gt 0 ]] || die "SHA256SUMS lists no files for ${fmt}"
    while IFS= read -r rel; do
        listed="$(grep -c -F -x -- "$(sha256_of "${payload}/${rel}")  ${rel}" "${sums}" || true)"
        [[ "${listed}" -ge 1 ]] || die "checksum: ${rel} is in the payload but not in SHA256SUMS"
    done < <(cd "${payload}" && find "lib/${fmt}" \( -type f -o -type l \) -print | LC_ALL=C sort)
    say "verified ${n} file(s) for ${fmt}"
}

# -------------------------------------------------------------------- state ---
receipt="$(receipt_path)"

receipt_entries() { [[ -f "${receipt}" ]] && cat "${receipt}" || true; }

# ---------------------------------------------------------------- uninstall ---
do_uninstall() {
    local fmt path found=0
    local -a want=()
    if [[ -n "${formats_arg}" ]]; then
        parse_formats "${formats_arg}"; want=("${SELECTED[@]}")
    else
        want=("${ALL_FORMATS[@]}")
    fi
    local -a kept=() handled=()
    while IFS=$'\t' read -r fmt path; do
        [[ -n "${fmt}" ]] || continue
        local hit=0 w
        for w in "${want[@]}"; do [[ "${w}" == "${fmt}" ]] && hit=1; done
        if [[ "${hit}" == 1 ]]; then
            # The receipt is only ever written by this script, but never rm outside the
            # destination directory for the format.
            [[ "${path}" == "$(dest_dir "${fmt}")"/?* && "${path}" != *..* ]] \
                || die "receipt entry ${path} is outside $(dest_dir "${fmt}"); refusing to remove it"
            if [[ -e "${path}" || -L "${path}" ]]; then
                run rm -rf -- "${path}"; say "$(verb remove removed) ${path}"; found=1; handled+=("${fmt}")
            else
                say "already absent: ${path}"
            fi
        else
            kept+=("${fmt}"$'\t'"${path}")
        fi
    done < <(receipt_entries)
    # Known names install without a receipt (dry-run installs, older layouts).
    for fmt in "${want[@]}"; do
        case "${fmt}" in vst3|clap|lv2) ;; *) continue ;; esac
        path="$(dest_dir "${fmt}")/${PRODUCT}.${fmt}"
        local h
        for h in ${handled[@]+"${handled[@]}"}; do [[ "${h}" == "${fmt}" ]] && continue 2; done
        if [[ -e "${path}" ]]; then run rm -rf -- "${path}"; say "$(verb remove removed) ${path}"; found=1; fi
    done
    if [[ "${dry}" != 1 ]]; then
        if [[ ${#kept[@]} -gt 0 ]]; then printf '%s\n' "${kept[@]}" > "${receipt}"
        else rm -f -- "${receipt}"; fi
    fi
    [[ "${found}" == 1 ]] || say "nothing to remove for: ${want[*]}"
}

# ------------------------------------------------------------------ install ---
do_install() {
    parse_formats "${formats_arg:-vst3}"
    [[ -d "${payload}/lib" ]] || die "no payload at ${payload} (expected ${payload}/lib/<format>/). Build one with: cmake --install <build> --prefix <dir>"

    local fmt
    # Refuse anything the payload lacks before touching the filesystem.
    for fmt in "${SELECTED[@]}"; do
        [[ -d "${payload}/lib/${fmt}" ]] && [[ -n "$(ls -A "${payload}/lib/${fmt}")" ]] \
            || die "format '${fmt}' is not in the payload ${payload}. Present: $(present_formats)"
    done
    if [[ "${verify}" == 1 ]]; then
        for fmt in "${SELECTED[@]}"; do verify_format "${fmt}"; done
    else
        [[ "${APW_ALLOW_UNVERIFIED:-0}" == 1 ]] || die "--no-verify needs APW_ALLOW_UNVERIFIED=1"
        say "WARNING: payload checksums NOT verified" >&2
    fi

    local dest item base
    local -a recorded=()
    for fmt in "${SELECTED[@]}"; do
        dest="$(dest_dir "${fmt}")"
        run mkdir -p -- "${dest}"
        for item in "${payload}/lib/${fmt}"/*; do
            base="$(basename "${item}")"
            run rm -rf -- "${dest:?}/${base:?}"
            run cp -R -- "${item}" "${dest}/${base}"
            say "$(verb install installed) ${dest}/${base}"
            recorded+=("${fmt}"$'\t'"${dest}/${base}")
        done
    done
    if [[ "${dry}" != 1 ]]; then
        mkdir -p -- "$(dirname "${receipt}")"
        {
            # Drop stale entries for paths we just re-recorded, keep the rest.
            local fmt2 path2 r skip
            while IFS=$'\t' read -r fmt2 path2; do
                [[ -n "${fmt2}" ]] || continue
                skip=0
                for r in "${recorded[@]}"; do [[ "${r}" == "${fmt2}"$'\t'"${path2}" ]] && skip=1; done
                [[ "${skip}" == 1 ]] || printf '%s\t%s\n' "${fmt2}" "${path2}"
            done < <(receipt_entries)
            printf '%s\n' "${recorded[@]}"
        } > "${receipt}.new"
        mv "${receipt}.new" "${receipt}"
    else
        say "[dry-run] would record ${#recorded[@]} entries in ${receipt}"
    fi
}

present_formats() {
    local fmt out=""
    for fmt in "${ALL_FORMATS[@]}"; do
        [[ -d "${payload}/lib/${fmt}" ]] && out+="${fmt} "
    done
    echo "${out:-none}"
}

if [[ "${uninstall}" == 1 ]]; then do_uninstall; else do_install; fi
