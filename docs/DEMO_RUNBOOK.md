# Demo Runbook

## Goal

Show one truthful, legible chain from routed audio to an exported-file hash in
under five minutes.

## Before the meeting

1. Run `./scripts/build_plugin.sh --install`.
2. Open Ableton Live, enable VST3 system folders, and rescan plug-ins.
3. Confirm **Audio Provenance Capture** appears in the browser.
4. Prepare a short, recognizable audio clip in a simple Ableton set.
5. Run `./scripts/preflight.sh`; it checks the port, runtime, disk, signing
   material, directories, and VST3 bundle.

## Start the capture

From the repository root:

```sh
./scripts/demo.sh ./demo-output imported_sample /path/to/Demo.als
```

Save the Ableton set first and pass its `.als` path (or export
`APW_PROJECT_PATH`); without it the manifest carries the `no_session_facts`
warning, the only remaining warning in a clean live pass that is fixable.
Export `APW_TIME_ANCHOR=1` (or a TSA URL) to anchor each export hash at an
RFC 3161 timestamp authority; this needs network access at sealing time and
degrades to an explicit `unavailable` record without it. The launcher creates
a fresh timestamped session and opens its dashboard and export folder.

If Ableton cannot be used, the presenter fallback completes the same local
receipt/alignment/sealing/verification story with deterministic synthetic audio:

```sh
./scripts/presenter_fallback.sh
```

It creates a new ignored session, receives every daemon ACK on the sender socket,
adds a three-window offset and fixed gain change, verifies the signed bundle, and
opens the completed dashboard and fight card. Interruption stops the daemon and
releases its UDP port.

## Ableton sequence

1. Insert **Audio Provenance Capture** on the demonstration track.
2. Open its UI and point out the scope statement.
3. Press play.
4. Wait for `Capture status: ACTIVE`, an increasing window count, and plug-in UI
   lines showing prepared, UDP attempted, locally emitted, and `ACKNOWLEDGED BY
   THIS DAEMON`. The dashboard must show the same plug-in instance and plug-in
   capture-session identifiers.
5. Export a WAV or AIFF into the folder opened by the launcher. Reusing a
   filename is okay; repeated artifacts are versioned `_v002`, `_v003`, and on.
6. Wait for both `Manifest written` and `Fight-card report written`.

## Present the fight card

Use the dashboard Fight Card link and show:

- the exported filename and SHA-256;
- the final routed-audio chain commitment;
- the number of observed hash windows;
- the proof label on each claim;
- the inferred feature-sequence alignment, confidence, coverage, and offset;
- the comparable/matched window counts and green/amber alignment bars;
- the daemon ACK protocol, accepted/contiguous sequence, gap, rejection, and
  chain-break state;
- the observation coverage counters and gap state;
- the explicit list of unobserved or unverified facts.
- the neutral downstream registration handoff and missing trust requirements.
- the directly downloadable ZIP and its canonical signed bundle index.

Suggested language:

> The plugin directly observed this routed audio, and the daemon directly
> hashed this export. Their association is intentionally labelled inferred
> because this proof of concept does not claim visibility into every Ableton
> path. The fight card makes that boundary visible instead of hiding it.

## Verify live

```sh
./scripts/verify_demo.sh "$(cat ./demo-output/latest-session.txt)/manifests/<export>_manifest.json"
```

Expected local POC outcome: `verified`, with warnings that signer identity is
unverified, the HMAC is local-only compatibility, and optional `.als` session
facts are unavailable.

Then run the safe disposable tamper and export-only sequence:

```sh
./scripts/demo_adversarial.sh /path/to/<export>_manifest.json
```

## Package to send

After a session has a sealed manifest (and optionally an adversarial run),
assemble a self-contained package for handing to a downstream reviewer:

```sh
python3 scripts/package_demo.py "$(cat ./demo-output/latest-session.txt)"
```

It emits a folder and deterministic ZIP under `demo-output/founder-package/`
containing the manifest, fight card, evidence bundle, verifier transcripts for
the original and tampered copies, the honest-null session, and a generated
README stating what is proven, inferred, and not established.

## Recovery

- No plugin stream: confirm the plugin window counter is moving and UDP port
  `9876` is free, then restart the daemon before restarting Ableton.
- Local emission advances but ACK stays unknown: confirm the daemon is running
  the same port and session, then restart the launcher. A stale, rejected,
  mismatched, or gap state must not be described as confirmed receipt.
- No manifest: confirm the export is WAV/AIFF, non-empty, and inside the exact
  watched export directory.
- No stem in the fight card: the export arrived before any plugin hash event;
  play audio through the plugin and export again.
- Ableton cannot find the plugin: rerun the build script with `--install`, then
  perform a full VST3 rescan.
- Bundle missing: open the stored verifier JSON. If the fight card is present but
  packaging failed, rerun the fallback and retain the prior manifest as an
  incomplete artifact set; do not hand-assemble or overclaim it.

## Remaining minimal Ableton gate

Automated and synthetic validation do not close this gate:

1. Quit Ableton completely.
2. Reopen it and rescan the installed VST3.
3. Insert **Audio Provenance Capture** on one routed stem.
4. Confirm the current plug-in instance and plug-in capture-session identifiers
   appear in the dashboard.
5. Play audio and confirm locally emitted and daemon-acknowledged counts advance.
6. Perform the documented transparency/null test.
7. Save, close, and reload the Ableton project.
8. Export a real WAV/AIFF into the watched folder.
9. Confirm inferred alignment, observation coverage, sealing, bundle creation,
   and local POC verification.
10. Save the final real-session manifest and fight card.

Recovery by step:

- 1–2: if Live or its scanner remains resident, quit it from Activity Monitor,
  reinstall with `./scripts/build_plugin.sh --install`, then reopen and rescan.
- 3: if insertion fails, confirm mono/stereo routing and inspect Ableton's plug-in
  scan log; do not substitute an older bundle.
- 4–5: if identifiers or ACKs do not agree, stop the launcher, confirm UDP 9876
  is free with `./scripts/preflight.sh`, then start one fresh session.
- 6: if nulling fails, remove all differing gain, pan, warp, routing, and Utility
  settings, then repeat with two otherwise identical paths.
- 7: if reload fails, retain the crash/scan log, reopen a copy of the set, and do
  not mark reload stability complete.
- 8: if no export appears, use uncompressed WAV/AIFF and the exact timestamped
  `exports/` folder shown by the launcher.
- 9: if alignment is unavailable/not established, inspect format, duration,
  matched count, and offset; treat the result as inconclusive and re-export after
  enough routed playback. If verification is `changed` or `untrusted`, preserve
  the artifacts and start a fresh session rather than editing them.
- 10: retain the manifest, fight card, signed bundle index, ZIP, and real export
  together; never replace this gate with a synthetic record.

## Timing rules for a clean live pass

The dashboard now enforces these in realtime: its **Export readiness** panel
shows READY TO EXPORT with a minimum render length, and flags a stale plug-in
instance or FIFO overflow with the exact recovery action. The rules below
explain what it is checking.

Learned on the 2026-08-28 live pass (`capture-20260828T221446Z-9959`, coverage
`complete_observed_path`, association `inferred_match`). Steps 8–9 fail without
them:

- Load the plug-in fresh **after** the daemon session starts. Coverage requires
  the plug-in's cumulative `windows_hashed` to equal the daemon's received chain
  length, so the counters must start at zero within the session. Delete and
  re-add the device; restarting the daemon mid-plug-in-life leaves the counters
  ahead and grades every later session `partial_observed_path`.
- Keep the observed session tight. Association requires the export to overlap at
  least 25% of all routed windows, and silence through a loaded plug-in counts
  as routed audio. Play at least as long as the export, and pick a render length
  of at least a quarter of the total time the device has been active.
- Deactivate the plug-in device before **File → Export**. Offline render outruns
  the realtime hasher, overflows the plug-in FIFO (`fifo_samples_dropped` grades
  coverage partial), and re-enters the render itself into the routed stream,
  diluting the overlap ratio. Deactivation freezes the observed stream at the
  played audio; reload the device fresh for the next take.

## Claims to avoid

Do not say the demo proves full Ableton provenance, sample rights, preset
identity, device identity, hidden plug-in state, or that bypass was impossible.
