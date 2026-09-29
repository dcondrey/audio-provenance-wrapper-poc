#!/usr/bin/env bash
# Print an `installer -applyChoiceChangesXML` plist that selects exactly the listed choices and
# deselects every other choice the pkg may offer.
#
#   packaging/macos/make_choices.sh clap,lv2,daemon [AudioProvenanceCapture-0.9.0.pkg] > choices.xml
#   sudo installer -pkg AudioProvenanceCapture-0.9.0.pkg -applyChoiceChangesXML choices.xml -target /
#
# Choice ids: vst3 au clap lv2 vst2 auv3 aax daemon docs loginagent
# loginagent needs daemon; selecting loginagent without daemon is refused here.
# With the optional pkg argument, only choices that pkg actually offers are emitted and
# selecting one it lacks is an error. Without it, all ids are emitted.
set -euo pipefail

ALL=(vst3 au clap lv2 vst2 auv3 aax daemon docs loginagent)
want=",${1:-}"; want="${want// /}"
[[ -n "${1:-}" ]] || { echo "usage: $0 <comma separated choice ids>" >&2; exit 2; }

IFS=',' read -r -a picked <<< "${1// /}"
for id in "${picked[@]}"; do
    ok=0; for a in "${ALL[@]}"; do [[ "${a}" == "${id}" ]] && ok=1; done
    [[ "${ok}" == 1 ]] || { echo "unknown choice '${id}'. Known: ${ALL[*]}" >&2; exit 2; }
done
if [[ "${want}" == *",loginagent"* && "${want}" != *",daemon"* ]]; then
    echo "loginagent requires daemon" >&2; exit 2
fi

OFFERED=("${ALL[@]}")
if [[ -n "${2:-}" ]]; then
    [[ -f "${2}" ]] || { echo "no such pkg: ${2}" >&2; exit 2; }
    shown="$(installer -pkg "${2}" -showChoicesXML -target / 2>/dev/null)" \
        || { echo "installer could not read ${2}" >&2; exit 2; }
    OFFERED=()
    for a in "${ALL[@]}"; do
        grep -q "<string>${a}</string>" <<< "${shown}" && OFFERED+=("${a}")
    done
    for id in "${picked[@]}"; do
        ok=0; for a in "${OFFERED[@]}"; do [[ "${a}" == "${id}" ]] && ok=1; done
        [[ "${ok}" == 1 ]] || { echo "choice '${id}' is not offered by ${2}. Offered: ${OFFERED[*]}" >&2; exit 2; }
    done
fi

echo '<?xml version="1.0" encoding="UTF-8"?>'
echo '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
echo '<plist version="1.0"><array>'
for id in "${OFFERED[@]}"; do
    val=0; [[ "${want}" == *",${id}"* && ",${want#,}," == *",${id},"* ]] && val=1
    printf '<dict><key>choiceIdentifier</key><string>%s</string><key>choiceAttribute</key><string>selected</string><key>attributeSetting</key><integer>%d</integer></dict>\n' "${id}" "${val}"
done
echo '</array></plist>'
