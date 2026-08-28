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

Omit the `.als` path if project parsing is not part of the story. The launcher
creates a fresh timestamped session and opens its dashboard and export folder.

## Ableton sequence

1. Insert **Audio Provenance Capture** on the demonstration track.
2. Open its UI and point out the scope statement.
3. Press play.
4. Wait for `Capture status: ACTIVE`, an increasing window count, and the daemon
   message `Capture plugin evidence stream detected`.
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
- the observation coverage counters and gap state;
- the explicit list of unobserved or unverified facts.
- the neutral downstream registration handoff and missing trust requirements.

Suggested language:

> The plugin directly observed this routed audio, and the daemon directly
> hashed this export. Their association is intentionally labelled inferred
> because this proof of concept does not claim visibility into every Ableton
> path. The fight card makes that boundary visible instead of hiding it.

## Verify live

```sh
./scripts/verify_demo.sh ./demo-output/manifests/<export>_manifest.json
```

Expected local POC outcome: `verified`, with warnings that signer identity is
unverified, the HMAC is local-only compatibility, and optional `.als` session
facts are unavailable.

Then run the safe disposable tamper and export-only sequence:

```sh
./scripts/demo_adversarial.sh /path/to/<export>_manifest.json
```

## Recovery

- No plugin stream: confirm the plugin window counter is moving and UDP port
  `9876` is free, then restart the daemon before restarting Ableton.
- No manifest: confirm the export is WAV/AIFF, non-empty, and inside the exact
  watched export directory.
- No stem in the fight card: the export arrived before any plugin hash event;
  play audio through the plugin and export again.
- Ableton cannot find the plugin: rerun the build script with `--install`, then
  perform a full VST3 rescan.

## Claims to avoid

Do not say the demo proves full Ableton provenance, sample rights, preset
identity, device identity, hidden plug-in state, or that bypass was impossible.
