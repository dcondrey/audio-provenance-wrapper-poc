# Demo Runbook

One truthful chain from routed audio to an exported file hash, checkable by a
tool we did not write, in under five minutes.

Every command below is written to run from the repository root with the project
virtualenv first on `PATH`. Every "Expect" block is verbatim output observed on
this machine on 2026-08-31.

---

## 0. Setup, before the room

```sh
cd /Volumes/A/audio-provenance
export PATH="$PWD/.venv/bin:$PATH"
which python3 c2patool
```

Expect:

```
/Volumes/A/audio-provenance/.venv/bin/python3
/opt/homebrew/bin/c2patool
```

**If python3 resolves anywhere else:** every script here calls `env python3` and
will die on `ModuleNotFoundError: cryptography`. Re-export `PATH` in the same
shell you will run the demo from.

```sh
./scripts/preflight.sh
```

**If it fails:** it names the failing check. UDP 9876 busy is the common one;
kill the previous demo process and rerun.

Then:

1. `./scripts/build_plugin.sh --install`
2. Open Live, enable VST3 system folders, rescan.
3. Confirm **Audio Provenance Capture** appears in the browser.
4. Set **File → Export Audio/Video** to WAV **16-bit** now, not during the demo.

---

## 1. Critical path

### Step 1: the offline proof (works with no Ableton, no daemon)

This is the strongest thing in the demo and it costs 20 seconds. Two 8.000 s
16-bit PCM clips ship in `packaging/assets`. Their PCM `data` chunks are
byte-identical (SHA-256 `5f2545393ffb5603...` on both). One carries an embedded
C2PA manifest in an appended `C2PA` chunk; the other carries nothing.

```sh
c2patool "packaging/assets/demo-project/apw-demo Project/Samples/Imported/apw-demo-signed.wav" \
  trust --trust_anchors packaging/assets/demo-root-ca.pem | grep validation_state
```

Expect:

```
  "validation_state": "Trusted"
```

```sh
c2patool "packaging/assets/demo-project/apw-demo Project/Samples/Imported/apw-demo-unsigned.wav"
```

Expect:

```
Error: No claim found
```

**If the signed clip does not read Trusted:** you passed the wrong anchor. The
shipped clips are signed by the store in `packaging/assets/demo-provenance-store`,
whose root is byte-identical to `packaging/assets/demo-root-ca.pem`. Anything a
live session produces is signed by a different root (see Step 6).

### Step 2: start the session

```sh
./scripts/demo.sh ./demo-output imported_sample "/path/to/YourSet.als"
```

All three arguments matter. Without arg 2 the Source Category card reads
`unknown / no observed stem`. Without arg 3 the manifest carries the
`no_session_facts` warning. Save the Live set first so arg 3 exists.

The launcher preflights, creates a fresh timestamped session, and opens the
dashboard and the exports folder. Never reuse a session directory.

**If it exits at preflight:** fix the named check or go to Step 7 (fallback).

### Step 3: insert the plug-in, after the daemon is up

The shipped demo set is deliberately **sample-only**. It contains no saved
plug-in device. Insert **Audio Provenance Capture** by hand now, on the track
you will play.

This ordering is not cosmetic. Coverage grading requires the plug-in's
cumulative `windows_hashed` to equal the daemon's received chain length, so the
plug-in's counters must start at zero inside this session. A device that was
already live before the daemon started grades every later session
`partial_observed_path`.

**If the counters are already ahead:** delete the device and re-add it. Do not
restart the daemon instead; that leaves the plug-in ahead.

### Step 4: play, and show delivery

Press play. Wait for `Capture status: ACTIVE`, a rising window count, and the
plug-in lines reading prepared, UDP attempted, locally emitted, and
`ACKNOWLEDGED BY THIS DAEMON`. The dashboard must show the same plug-in
instance ID and capture-session ID.

Play at least as long as the render you are about to make. Association requires
the export to overlap at least 25% of all routed windows, and silence through a
loaded plug-in still counts as routed audio.

**If locally emitted rises but ACK stays unknown:** the plug-in and daemon are
on different sessions or ports. Stop the launcher, confirm 9876 is free, start
one fresh session. A stale, rejected, mismatched or gap state is not receipt and
must never be described as one.

### Step 5: deactivate, then export

**Deactivate the plug-in device before File → Export.** Offline render outruns
the realtime hasher, overflows the plug-in FIFO (`fifo_samples_dropped` grades
coverage partial), and feeds the render itself back into the routed stream,
diluting the overlap ratio.

Export **16-bit PCM WAV** into the exports folder the launcher opened. Wait for
both `Manifest written` and `Fight-card report written`.

**If the association reads `unavailable` with reason `unsupported WAV format`:**
the render was 32-bit float. Live remembers the last bit depth. Set 16-bit and
export again. Leave the bad artifact in place as an honest record; do not edit
it.

### Step 6: verify the live export

```sh
S="$(cat ./demo-output/latest-session.txt)"
./scripts/verify_demo.sh "$S/manifests/<export>_manifest.json"
```

Both artifact names are built from the export's basename, so an export named
`Demo.wav` yields `manifests/Demo_manifest.json` and
`manifests/artifacts/Demo_c2pa.wav`. A repeated filename is versioned with the
suffix *before* the tail: `Demo_v002_manifest.json`,
`Demo_v002_c2pa.wav`. Tab-complete rather than typing these.

Expect the shape below (captured from a real sealed session on 2026-08-31):

```
INFO: OK:   [export_hash_valid] Export file SHA-256 matches manifest
INFO: OK:   [evidence_hash_valid] Evidence prefix hash matches: plugin_events.jsonl
INFO: OK:   [coverage_counters_rederived] Recomputed 40 buffer_hash events from bound evidence; counters agree
INFO: OK:   [signed_content_hash_valid] Manifest content matches its signed-content hash
INFO: OK:   [local_signature_valid] Local HMAC integrity seal verified
WARNING: WARN: [local_integrity_only] Local HMAC integrity is not hardware attestation or external identity
INFO: OK:   [portable_signature_valid] Ed25519 signature verified against the pinned public key ...
WARNING: WARN: [signer_identity_unverified] Signature validity does not establish the signer's externally verified identity
INFO: OK:   [c2pa_claim_verified] The C2PA claim chains to the trust anchor held by this machine and its hard binding is intact; this is not an externally verified identity
INFO: OUTCOME: verified (local POC verifier; not a registry or identity result)
INFO: PASS: 12 checks, 3 warnings
```

Those warnings are the honest position, not defects. Say so before anyone asks.

Then hand the session's C2PA asset to the independent tool:

```sh
c2patool "$S/manifests/artifacts/<export>_c2pa.wav" \
  trust --trust_anchors ~/.apw/provenance/ca/root_cert.pem | grep validation_state
```

Expect:

```
  "validation_state": "Trusted"
```

**IMPORTANT, anchor trap.** A live session run from this repo signs with the
default store `~/.apw/provenance`, *not* the shipped demo store. Verifying a
session export against `packaging/assets/demo-root-ca.pem` returns
`signingCredential.untrusted` and `"validation_state": "Valid"`, which looks
like a failure on stage and is not one. Two anchors, two scopes:

| Asset | Anchor |
| --- | --- |
| `packaging/assets/.../apw-demo-*.wav` (shipped clips) | `packaging/assets/demo-root-ca.pem` |
| Anything a repo session produces | `~/.apw/provenance/ca/root_cert.pem` |
| Anything the installed DMG daemon produces | `~/.apw/demo-provenance-store/ca/root_cert.pem` |

### Step 7: fallback, if Ableton is unavailable

```sh
./scripts/presenter_fallback.sh
```

Runs the whole local story on deterministic synthetic audio: ACK receipt, a
fixed gain plus three-window offset, inferred alignment, sealing, verification,
bundle integrity. It prints a JSON summary and opens the dashboard and fight
card. Observed on 2026-08-31:

```
  "verifier_outcome": "verified",
  "coverage": "complete_observed_path",
  "association": "inferred_match",
  "association_offset_seconds": 0.2786,
  "bundle_integrity": "verified"
```

Say out loud that this is a synthetic operational fallback and not manual
Ableton proof.

---

## 2. The four verification states, on demand

The whole vocabulary is four states, and three of them are not failures. Each
one below is a real command with real output.

```sh
./.venv/bin/python - <<'PY'
from pathlib import Path
from daemon.c2pa_engine.verifier import verify_asset
base = Path("packaging/assets")
clips = base / "demo-project/apw-demo Project/Samples/Imported"
anchor = (base / "demo-root-ca.pem").read_bytes()
foreign = (Path.home() / ".apw/provenance/ca/root_cert.pem").read_bytes()
for label, path, pem in (
    ("verified", clips / "apw-demo-signed.wav", anchor),
    ("nothing found", clips / "apw-demo-unsigned.wav", anchor),
    ("mark found, claim not trusted", clips / "apw-demo-signed.wav", foreign),
):
    r = verify_asset(path, "audio/wav", trust_anchors_pem=pem)
    print(f"{label:32} -> {r.state} / {r.validation_state} / {list(r.failure_codes)}")
PY
```

Expect:

```
verified                         -> verified / Trusted / []
nothing found                    -> nothing_found / None / []
mark found, claim not trusted    -> mark_found_claim_not_trusted / Valid / ['signingCredential.untrusted']
```

The fourth state needs a deliberately altered copy. This never touches a
shipped asset:

```sh
mkdir -p demo-output/tamper-check
/bin/cp -f "packaging/assets/demo-project/apw-demo Project/Samples/Imported/apw-demo-signed.wav" \
  demo-output/tamper-check/tampered.wav
./.venv/bin/python -c "
p='demo-output/tamper-check/tampered.wav'
b=bytearray(open(p,'rb').read()); b[705644] ^= 1; open(p,'wb').write(b)"
c2patool demo-output/tamper-check/tampered.wav \
  trust --trust_anchors packaging/assets/demo-root-ca.pem | grep -E 'validation_state|dataHash'
```

Expect:

```
      "code": "assertion.dataHash.mismatch",
          "code": "assertion.dataHash.mismatch",
  "validation_state": "Invalid"
```

Offset 705644 is one byte in the middle of the PCM `data` chunk. Flipping a byte
in the appended `C2PA` chunk instead would change nothing, because the
`c2pa.hash.data` exclusion covers exactly that appended region.

`/bin/cp -f`, not `cp`: `cp` is aliased interactive here and a silent `n` leaves
you verifying the wrong file.

---

## 3. Traps that kill the demo

- **Collect All and Save.** Running it in Live on the demo project rewrites the
  samples and the signed clip silently loses its manifest. Never run it on this
  project.
- **32-bit float export.** Association grades `unavailable`, the alignment chart
  is empty, and the claim cannot be signed. 16-bit PCM WAV only.
- **Plug-in loaded before the daemon.** Coverage drops to
  `partial_observed_path`.
- **Plug-in left active during offline render.** FIFO overflow, diluted overlap.
- **Wrong trust anchor.** Reads as `signingCredential.untrusted`. See the table
  in Step 6.
- **Bare `cp` on any demo asset.** Interactive alias, silent no-op.
- **Reused session directory.** Always start a fresh one via `demo.sh`.

---

## 4. Presenting the fight card

Use the dashboard Fight Card link. Show, in this order:

- exported filename and SHA-256;
- final routed-audio chain commitment and observed window count;
- the proof label on each claim;
- inferred alignment: confidence, coverage, offset, matched/comparable counts,
  green and amber bars;
- daemon ACK protocol: accepted and contiguous sequence, gaps, rejections,
  chain-break state;
- observation coverage counters;
- the explicit list of unobserved and unverified facts;
- the downstream registration handoff and its missing trust requirements;
- the downloadable ZIP and its signed canonical index.

Line to use:

> The plug-in directly observed this routed audio and the daemon directly hashed
> this export. Their association is labelled inferred on purpose, because this
> proof of concept does not claim visibility into every path inside Ableton. The
> card makes that boundary visible instead of hiding it.

---

## 5. Packaging for handoff

```sh
./.venv/bin/python scripts/package_demo.py "$(cat ./demo-output/latest-session.txt)"
```

Emits a folder and deterministic ZIP under `demo-output/founder-package/` with
the manifest, fight card, evidence bundle, verifier transcripts for the original
and tampered copies, the honest-null session, and a README stating what is
proven, inferred, and not established.

---

## 6. Recovery reference

- **No plug-in stream:** confirm the window counter is moving and UDP 9876 is
  free, restart the daemon, then restart Ableton.
- **Emitted but never acknowledged:** same port, same session, restart the
  launcher. Send success is not receipt.
- **No manifest:** the export must be uncompressed WAV or AIFF, non-empty, and
  inside the exact watched exports folder.
- **No stem in the fight card:** the export arrived before any plug-in hash
  event. Play routed audio, then export again.
- **Live cannot find the plug-in:** rerun `./scripts/build_plugin.sh --install`,
  then a full VST3 rescan.
- **Bundle missing:** open the stored verifier JSON. If the fight card exists but
  packaging failed, rerun the fallback and keep the prior manifest as an
  incomplete artifact set. Never hand-assemble one.

---

## 7. Re-validation checklist (gate closed 2026-08-29)

Closed live on the demonstration machine, recorded per step in
`docs/VALIDATION.md` and `docs/ROADMAP.md`. **Do not spend meeting time
re-running it.** Keep this list for a new machine or a rebuilt plug-in.
Automated and synthetic results never close this gate.

1. Quit Ableton completely.
2. Reopen and rescan the installed VST3.
3. Insert **Audio Provenance Capture** on one routed stem.
4. Confirm the plug-in instance and capture-session identifiers appear in the
   dashboard.
5. Play, and confirm locally emitted and daemon-acknowledged counts advance.
6. Perform the transparency/null test.
7. Save, close, and reload the project.
8. Export a real 16-bit PCM WAV into the watched folder.
9. Confirm inferred alignment, observation coverage, sealing, bundle creation,
   and local verification.
10. Keep the real manifest, fight card, bundle index, ZIP, and export together.

Recovery by step:

- **1–2:** if Live or its scanner stays resident, quit from Activity Monitor,
  reinstall with `--install`, reopen, rescan.
- **3:** confirm mono/stereo routing and read Live's plug-in scan log. Never
  substitute an older bundle.
- **4–5:** stop the launcher, confirm UDP 9876 free with `./scripts/preflight.sh`,
  start one fresh session.
- **6:** remove every differing gain, pan, warp, routing and Utility setting,
  then repeat with two otherwise identical paths. The null test is measured on a
  float render on purpose; that constraint does not apply to the graded export.
- **7:** keep the crash or scan log, reopen a copy, and do not mark reload
  stability complete.
- **8:** uncompressed WAV/AIFF into the exact timestamped `exports/` folder. An
  `unavailable` association means the render was 32-bit float.
- **9:** inspect format, duration, matched count and offset; treat as
  inconclusive and re-export after more routed playback. A `changed` or
  `untrusted` result means preserve the artifacts and start a fresh session,
  never edit them.
- **10:** never replace this gate with a synthetic record.

---

## 8. Claims to avoid

Do not say this proves full Ableton provenance, sample rights, preset identity,
device identity, hidden plug-in state, or that bypass was impossible. Do not
call the signer a verified identity. Do not call a missing manifest evidence of
synthetic origin.
