`audio-provenance` — ESM-only bin, node:util parseArgs, zero argument-parsing dependencies. The CLI holds NO provenance logic: every verdict comes from the SDK; the CLI chooses a renderer and maps status to an exit code.

GLOBAL FLAGS
  --json  --config=<path>  --registry=<name>  --trust-store=<dir>  --offline
  --quiet  --no-color  --version  --help

EXIT CODES (stable; the contract for CI use)
  0  verified
  1  changed
  2  untrusted
  3  not_found
  4  incomplete (a rung could not run: registry unreachable, decode timeout)
  64 usage error       65 unreadable/oversize input      70 internal error
`--offline` never yields 4 by itself: skipped network rungs are `skipped`, not `unavailable`.

A lossily transcoded copy of a marked, registered work exits 0 when the verifier
is given BOTH `--null-test` and a `--trust-store` that anchors the signer, and
exits 2 (`untrusted / soft_binding_false_positive_rate_unknown`) without the
first. It does NOT exit 1: the soft binding affirmed the recording, so reporting
`changed` would be a false statement about a file nobody altered. Exit 1 stays
what it always was, audio that moved under a valid signature with nothing
corroborating it as the same work.

--- audio-provenance verify <file> [--registry=<name>] [--sidecar=<path>|--no-sidecar]
                          [--threshold=<0..1>] [--accept-inferred] [--offline] [--json] [-v]

THE BRIEF'S INVOCATION, RENDERED EXACTLY:

  $ audio-provenance verify filename.wav --registry=public
  verified   filename.wav
    identity   Signal Room Studios        (externally_verified via studio-ca)
    signed     2026-03-13
    match      1.00                       (hard binding, exact)
    registry   public (filesystem)        recovered via embedded manifest

The identity line prints the authority in parentheses BECAUSE identity without
an authority is the exact claim the prior art forbids. When no anchor resolved:

  $ audio-provenance verify demo.wav --registry=public
  untrusted  demo.wav
    identity   -                          (unknown_unobserved: self-generated key,
                                           proves key possession, not identity)
    signed     2026-03-13
    match      1.00                       (hard binding, exact)

A transcode-recovered result shows the soft basis without hiding it in a footnote:

  $ audio-provenance verify ripped.mp3 --registry=public
  verified   ripped.mp3
    identity   Signal Room Studios        (externally_verified via studio-ca)
    signed     2026-03-13
    match      0.94                       (GenoMark, 17/18 blocks, inferred)
    note       hard binding unavailable (audio re-encoded); soft binding above
               threshold 0.72. False-positive rate at this match: 1.4e-07.

-v adds the full ladder, including the rungs that missed, because
`not_found` must never be readable as "no manifest exists":

  trace  embedded_manifest       miss  0.4ms   no provenance box in RIFF
         sidecar_manifest        miss  0.2ms   no sidecar at ripped.mp3.audio-provenance.json
         content_hash_lookup     miss  8.1ms   sha256 e3b0c442... not in registry
         decoded_audio_hash      miss  7.9ms   decoded audio digest not in registry
         genomark_recovery       hit   4.8s    strong, 17/18 blocks, rho 1.0000
         fingerprint_search      skip  -       requires --accept-inferred

--json emits the VerifyResult verbatim, one object, no wrapper. This is the
contract for scripting; the human renderer may change, the JSON may not.

--- audio-provenance sign <file> --key=<id|path> [--embed] [--sidecar=<path>|--no-sidecar]
                        [--mark] [--out=<path>] [--registry=<name>]
Order is enforced and reported: allocate the locator, mark, hash, sign, publish.
`--mark` on an already-signed file exits 64 with `mark_after_sign`, because
marking changes the audio the hard binding covers. With `--registry=<name>` the
backend is opened BEFORE the audio is rewritten and the freshly allocated
locator is checked against it; a registry that cannot answer exits 4 with
`locator_check_unavailable` rather than marking a master blind.

--- audio-provenance embed <file> --out=<path> [--namespace=<n>] [--delta=<nepers>]
Writes a marked copy and prints the payload plus the honest capability block:

  $ audio-provenance embed master.wav --out=master.marked.wav
  embedded   master.marked.wav
    algorithm  genomark-lepqim-v1         payload 56 bits
    locator    a41f9c2e5b07               namespace 0 (public)
    resolves   nothing: a standalone embed publishes no record. `audio-provenance sign --mark
               --registry=<name>` allocates a locator and registers the record it names.
    block      9.66 s                     guaranteed decode 19.32 s
    survives   lossy compression (unmeasured), resample, gain, speed +-6%
    FAILS      acoustic re-recording (unsupported by design)
               lowpass below 5 kHz, time-stretch, pitch shift
    transparency  not measured

The FAILS block is not a --verbose extra. It prints on every embed, because a
user who does not see it will assume the mark survives a microphone.

--- audio-provenance inspect <file> [--json]
Reports what GenoTrace found with NO verdict and NO trust evaluation. Exit 0
whenever the file was readable, regardless of provenance.

--- audio-provenance registry <init|add|list|get> ...
Manages the local filesystem registry and named backends in config. `add`
refuses to store a manifest whose canonical bytes do not round-trip
(`noncanonical_manifest`) — the corpus stays canonical by construction.

--- audio-provenance bench <run|report|ratchet> [--corpus=<dir>] [--rows=<glob>]
Ships in the CLI, not in a scripts/ directory, so it is a named, invocable
surface rather than an orphan script. `ratchet` compares the emitted JSON
matrix against the committed baseline and exits non-zero on any pass-rate
regression on a row that previously met threshold.
