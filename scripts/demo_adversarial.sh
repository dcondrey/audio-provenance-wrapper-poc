#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "Usage: $0 /path/to/export_manifest.json" >&2
    exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
MANIFEST="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
EXPORT="$(cd "${PROJECT_ROOT}" && env python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["export"]["file_path"])' "${MANIFEST}")"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
COPY_DIR="$(dirname "$(dirname "${MANIFEST}")")/adversarial-copies/${STAMP}"
mkdir -p "${COPY_DIR}"
cd "${PROJECT_ROOT}"

echo "1/4 Original bundle"
"${SCRIPT_DIR}/verify_demo.sh" "${MANIFEST}"

echo "2/4 Disposable altered export"
cp "${EXPORT}" "${COPY_DIR}/altered-export$(basename "${EXPORT}" | sed 's/^[^.]*//')"
ALTERED_EXPORT="${COPY_DIR}/altered-export$(basename "${EXPORT}" | sed 's/^[^.]*//')"
printf 'tamper' >> "${ALTERED_EXPORT}"
if env python3 -m daemon.verify "${MANIFEST}" --public-only --export "${ALTERED_EXPORT}"; then
    echo "Expected altered export verification to fail" >&2
    exit 1
fi

echo "3/4 Disposable altered manifest"
cp "${MANIFEST}" "${COPY_DIR}/altered-manifest.json"
perl -0pi -e 's/Never claim full DAW provenance\./Altered demo claim./' "${COPY_DIR}/altered-manifest.json"
if env python3 -m daemon.verify "${COPY_DIR}/altered-manifest.json" --public-only; then
    echo "Expected altered manifest verification to fail" >&2
    exit 1
fi

echo "4/4 Fresh export-only session (honest unobserved result)"
env python3 "${SCRIPT_DIR}/synthetic_rehearsal.py" --export-only --output "${COPY_DIR}/export-only"

echo "Disposable copies: ${COPY_DIR}"
