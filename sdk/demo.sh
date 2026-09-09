#!/usr/bin/env bash
# Audio Provenance end-to-end demonstration. Every verdict below is produced live.
#
#   ./demo.sh [workdir]
#
# Shows, in order: an exact verification that resolves a real name; the same file
# after an mp3 transcode still verifying through the watermark; the soft binding
# refusing to verify without a measured false-positive rate; the same file with no
# trust store; the same file after the signer is revoked; a mark lifted onto
# unrelated audio; and an unregistered file reported neutrally.
set -euo pipefail

WORK="${1:-/tmp/audio-provenance-demo}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
G="${AUDIO_PROVENANCE_BIN:-/Volumes/C/rust-target/release/audio-provenance}"
FFMPEG="${FFMPEG:-/opt/homebrew/bin/ffmpeg}"

# The 1,000-work null test if it has been run, the pilot otherwise. Either way the
# rate behind a soft binding is measured; --null-test names which measurement.
NULL_TEST="${NULL_TEST:-}"
if [ -z "$NULL_TEST" ]; then
  for candidate in "$REPO/bench-out/null-v1/apw-watermark-lepqim-v1-null-test.json" \
                   "$REPO/bench-out/null-pilot/apw-watermark-lepqim-v1-null-test.json"; do
    [ -f "$candidate" ] && { NULL_TEST="$candidate"; break; }
  done
fi

[ -x "$G" ] || { echo "build first: cargo build --release -p audio-provenance-cli"; exit 1; }
[ -f "$NULL_TEST" ] || { echo "no null-test report; run: cargo run --release -p audio-provenance-nulltest --bin audio-provenance-null-test"; exit 1; }

rm -rf "${WORK:?}"; mkdir -p "$WORK"; cd "$WORK"
export AUDIO_PROVENANCE_REGISTRY_ROOT="$WORK/reg"

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
run() { printf '\033[2m$ %s\033[0m\n' "$*"; eval "$@"; }

say "Setup: a master, a signing key, an authority, and a registry"
# Real recorded music when the licence-verified corpus has been fetched, because a
# claim about music should be demonstrated on music. Synthesised pink noise
# otherwise, so this script runs without the 7.2 GB download.
# A dense master, deliberately. Step 8 below shows the sparse case, which behaves
# differently and is the more interesting half of the truth.
REAL="$REPO/corpus/characterisation/real_rock_loud.wav"
SPARSE="$REPO/corpus/characterisation/real_piano_sparse.wav"
if [ -f "$REAL" ]; then
  cat "$REAL" > master.wav
  echo "master: real_rock_loud.wav (FMA, licence-verified; see corpus/MANIFEST-characterisation.json)"
else
  "$FFMPEG" -v error -f lavfi -i "anoisesrc=d=45:c=pink:r=48000:a=0.35" \
    -ac 2 -c:a pcm_s16le master.wav -y
  echo "master: synthesised pink noise (corpus not fetched; see corpus/README.md)"
fi

"$G" keygen --out signer.key >/dev/null
"$G" keygen --out authority.key >/dev/null
"$G" registry init ./reg >/dev/null

# The authority is self-signed. It is trusted because a verifier is handed it,
# not because it asserts anything about itself.
"$G" trust init-authority --key authority.key --anchor-id signal-room-ca \
  --name "Signal Room Studios CA" --out anchor.json >/dev/null
"$G" trust issue --anchor anchor.json --issuer-key authority.key \
  --subject-key signer.pub --name "Signal Room Studios" \
  --record-id sr-001 --out record.json >/dev/null
"$G" trust export-anchor --anchor anchor.json --out store.json >/dev/null
"$G" trust add --store store.json --document record.json >/dev/null

say "Sign the master, embed a Watermark, publish the record"
run "\"$G\" sign master.wav --key signer.key --mark --out signed.wav --registry local --no-color | tee sign.out"
LOCATOR="$(awk '$1 == "mark" { print $2; exit }' sign.out)"

say "1. Verify the signed file  ->  expect: verified, exact, with a name"
run "\"$G\" verify signed.wav --registry local --trust-store store.json --null-test \"$NULL_TEST\" || true"

say "2. Transcode to 128k mp3, then verify  ->  expect: verified (inferred)"
# The hard binding cannot survive a lossy codec. The watermark and fingerprint
# carry the verdict instead, and the result says so rather than claiming exactness.
run "\"$FFMPEG\" -v error -i signed.wav -b:a 128k transcoded.mp3 -y"
run "\"$G\" verify transcoded.mp3 --registry local --trust-store store.json --null-test \"$NULL_TEST\" || true"

say "3. Verify without the null-test report  ->  expect: refuses to claim verified"
# A soft binding may not reach `verified` until the false-positive rate behind it
# has actually been measured. The gate is in the type system, not a review note.
run "\"$G\" verify transcoded.mp3 --registry local --trust-store store.json || true"

say "4. Verify without the trust store  ->  expect: identity not_established"
run "\"$G\" verify signed.wav --registry local --null-test \"$NULL_TEST\" || true"

say "5. Revoke the signer, then verify again  ->  expect: refused, and it says why"
# Revocation is retroactive and the refusal is printed, not buried behind -v: a
# revoked signer must not render the same as a key the store never heard of.
run "cat store.json > revoked-store.json"
run "\"$G\" trust revoke --anchor anchor.json --key authority.key \
  --subject-key signer.pub --reason key_compromise --out crl.json >/dev/null"
run "\"$G\" trust add --store revoked-store.json --document crl.json >/dev/null"
run "\"$G\" verify signed.wav --registry local --trust-store revoked-store.json --null-test \"$NULL_TEST\" || true"

say "6. Lift the mark onto unrelated audio  ->  expect: changed, not verified"
"$FFMPEG" -v error -f lavfi -i "anoisesrc=d=20:c=brown:r=48000:a=0.30" \
  -ac 2 -c:a pcm_s16le unrelated.wav -y
# The locator is the 48 bits the mark carries. Writing the SAME locator into other
# audio is exactly the watermark-copy attack the soft binding has to survive.
run "\"$G\" embed unrelated.wav --locator \"$LOCATOR\" --out lifted.wav >/dev/null"
run "\"$G\" verify lifted.wav --registry local --trust-store store.json --null-test \"$NULL_TEST\" || true"

say "7. A file that was never registered  ->  expect: not_found, stated neutrally"
run "\"$G\" verify master.wav --registry local --trust-store store.json --null-test \"$NULL_TEST\" || true"

say "8. The same transcode on a SPARSE master  ->  expect: changed, and it says why"
# This is not a bug being hidden. Watermark needs in-band energy to carry a block,
# and quiet, peaky material through a lossy codec loses enough whole blocks to fall
# under the 0.72 coverage guard. The guard is what stops a spliced insert verifying,
# so it is not being loosened to make this row pass. Measured: 7 of the 13
# licence-verified real items report `changed` here where synthetic pink noise of
# the same duration verifies. See STATUS.md.
if [ -f "$SPARSE" ]; then
  run "cat \"$SPARSE\" > sparse.wav"
  run "\"$G\" sign sparse.wav --key signer.key --mark --out sparse-signed.wav --registry local --no-color >/dev/null"
  run "\"$FFMPEG\" -v error -i sparse-signed.wav -b:a 128k sparse.mp3 -y"
  run "\"$G\" verify sparse.mp3 --registry local --trust-store store.json --null-test \"$NULL_TEST\" -v || true"
else
  echo "skipped: corpus not fetched"
fi

say "Exit codes: 0 verified, 1 changed, 2 untrusted, 3 not_found, 4 incomplete"
