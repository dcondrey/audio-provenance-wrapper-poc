# Validation

This page documents manual validation for the JUCE VST3 proof-of-concept milestones.

## v0.9 Demo Candidate

Automated validation on 2026-08-27:

- 89 Python tests pass.
- Release VST3 builds against pinned JUCE 8.0.15 in a clean build directory.
- The generated bundle is arm64, version 0.9.0, and targets macOS 12.0+.
- `codesign --verify --deep --strict` passes after final ad-hoc bundle signing.
- Built executable SHA-256:
  `40ea3e89320dbf05bb14a3ac9f62acd47e5751cef2747701f604947832b26efe`.
- UDP receipt through export detection produces a JSON manifest and HTML fight card.
- New and overwritten WAV exports are detected.
- The manifest verifier checks the export SHA-256, evidence-prefix hashes,
  routed-audio chain commitment, and signed-content hash.
- Source declarations and stem-to-export associations retain their correct
  `user_declared` and `inferred` proof levels.

Hardening validation on 2026-08-28:

- 92 Python tests pass (89 preserved tests plus three focused trust regressions).
- The authorized 30,412,921,329-byte generated
  `demo-output/evidence/composite_events.jsonl` was confirmed closed and removed.
- Mixed-clock and 5,000-event regressions keep correlation within its count
  bound, suppress repeated matches, and do not emit candidate-sized evidence.
- Evidence prefix generation and verification stream bounded chunks.
- A synthetic routed-audio rehearsal reports `complete_observed_path`, an
  `inferred_match`, and local POC verifier outcome `verified`.
- The adversarial rehearsal preserves the original, reports `changed` for a
  modified export copy, reports `changed` for a modified manifest copy, and
  produces an export-only `unknown_coverage` / unavailable association result.
- Portable Ed25519 verification succeeds without the HMAC secret; signer
  identity remains explicitly unverified.
- The current Release VST3 builds against JUCE 8.0.15 and passes strict ad-hoc
  bundle signature verification.
- The built and installed arm64 executables are byte-identical with SHA-256
  `0c01fcb673c654c0716b346998c76ea1847202eca02be88117cd8671b16f497a`.

Presenter hardening validation on 2026-08-28:

- 95 Python tests pass: the prior 92 plus three focused ACK-state,
  routed-association-fixture, and deterministic-bundle tests.
- A fresh build directory compiled release VST3 version 0.9.0 from JUCE tag
  8.0.15. The binary is arm64 with deployment target macOS 12.0.
- Built and installed executables are byte-identical with SHA-256
  `9b29fbd28eeb70875114f9d935e16867df38045802877d84dd71427178bdf909`.
- Strict deep signing verification passes for both bundles. The final install is
  Developer ID signed by team `U3PZN7P3E5`, uses hardened runtime, and carries a
  secure timestamp. CDHash is `04f8ff083be5052c16d1167b80aa15f2d3a18794`.
- `spctl` reports `Unnotarized Developer ID`. Notarization/packaging remains
  production distribution work; it is not represented as complete.
- Preflight reports READY with zero failures: installed bundle, arm64
  architecture, Developer ID signing, Python 3.14.6, signing material, disk,
  Ableton presence, and UDP port 9876 all pass.
- The exact `./scripts/presenter_fallback.sh` path completes in a fresh ignored
  session: 40/40 daemon ACKs, highest accepted/contiguous sequence 40,
  `complete_observed_path`, gain-adjusted three-window-offset
  `inferred_match` at 0.2786 seconds, local verifier `verified`, and signed bundle
  integrity `verified`.
- The stored verifier JSON records `html_report_present`; it has no false
  `html_report_missing` artifact-ordering warning.
- Original same-machine and `--public-only` verification both return the local
  POC outcome `verified`. The latter verifies Ed25519 using the public key and
  explicitly leaves signer identity unverified.
- Disposable altered-export and altered-manifest copies return `changed`.
- A fresh export-only rehearsal returns local file-integrity `verified` with
  `unknown_coverage` and `unavailable` routed/export association.
- The signed bundle index and every indexed archive payload hash verify. Dashboard,
  fight-card, handoff, bundle, index, and verifier links resolve to existing files.
- Ctrl-C shutdown of the primary launcher records `stopped`, leaves no daemon on
  its test port, and permits immediate UDP rebinding.
- Existing long-session bounds and streaming evidence-prefix tests pass. A code
  scan confirms evidence prefixes remain chunk-streamed; whole-file reads are
  limited to small manifests, indexes, and keys rather than JSONL evidence.
- Ableton Live 12 Trial is installed and running. Its log proves the prior 0.9.0
  bundle was scanned and instantiated on 2026-08-27, but the running process
  still holds the prior binary inode after the new install. No current-build
  rescan, UI observation, null test, reload, or real export was observed, so none
  is marked complete.

Manual v1.0 gate still to record on the demonstration machine:

1. [ ] Quit Ableton completely.
2. [ ] Reopen it and rescan the installed VST3.
3. [ ] Insert **Audio Provenance Capture** on one routed stem.
4. [ ] Confirm current plug-in instance and capture-session identifiers in the dashboard.
5. [ ] Confirm locally emitted and daemon-acknowledged counts advance during playback.
6. [ ] Complete the documented polarity/null transparency test.
7. [ ] Save, close, and reload the Ableton project.
8. [ ] Export a real WAV/AIFF into the watched folder.
9. [ ] Confirm inferred alignment, coverage, sealing, bundle creation, and verification.
10. [ ] Save the final real-session manifest and fight card.

Exact per-step recovery instructions are in `docs/DEMO_RUNBOOK.md`.

No automated or synthetic result is recorded as manual Ableton validation.

## Historical Milestone Records

The sections below preserve validation for the earlier scaffold and Epic 3
milestones. Statements about hashing, UDP, and daemon functionality describe
those historical builds, not the current v0.9 candidate.

## Sample Import Provenance Spike

Manual validation for the local sample-folder watcher is documented in `docs/SAMPLE_IMPORT_VALIDATION.md`.

This spike records filesystem-observed sample metadata and SHA-256 hashes in `evidence/sample_import_events.jsonl`. It does not modify the plugin, add C2PA signing, add wrapper-host behavior, or claim exact Ableton track attribution.

## Epic 3 - Audio Buffer Observation Validation

This section documents manual validation for GitHub issue `#6`, Epic 3 - Audio Buffer Observation.

The plugin observes lightweight buffer metadata and non-silent audio presence in the audio callback, stores that state in atomics, and lets the UI refresh labels on a timer. It does not hash audio, send UDP, write files, run a daemon, create C2PA data, or host wrapped plugins.

### Manual Ableton Test

1. Build the plugin using the Milestone A build steps below.
2. Copy `Audio Provenance Capture.vst3` to `~/Library/Audio/Plug-Ins/VST3/`.
3. Open Ableton Live and rescan VST3 plugins if needed.
4. Load a sample loop on an audio track.
5. Insert `Audio Provenance Capture` on the audio track.
6. Open the plugin UI.
7. Press play.
8. Confirm the UI changes from `Capture status: IDLE` to `Capture status: ACTIVE`.
9. Confirm `Audio detected: yes` while non-silent audio is playing.
10. Confirm the UI displays channel count, sample rate, buffer size, and `Last buffer seen: HH:MM:SS`.
11. Stop playback.
12. Confirm the UI eventually returns to `Capture status: IDLE` and `Audio detected: no`, or otherwise reflects no recent non-silent audio if Ableton continues delivering silent buffers.

### Expected Results

- The plugin still loads as `Audio Provenance Capture`.
- Audio remains audible and passes through unchanged.
- The UI visibly reports recent non-silent buffer activity during playback.
- `Channels`, `Sample rate`, `Buffer size`, and `Last buffer seen` update from observed host buffers.
- No hashing, UDP, daemon, C2PA, file logging, or wrapper-host behavior is introduced.

### Known Limitations

- This is a UI-visible observation milestone, not provenance capture or manifest generation.
- `Capture status: ACTIVE` means recent non-silent audio was observed through the plugin, not that full Ableton provenance is captured.
- Host-specific behavior after transport stop can vary; some hosts may continue calling the plugin with silent buffers.

## Milestone A - JUCE Project Builds Successfully

### Manual Test

From the repository root on macOS:

```sh
cmake -S . -B build -DAPW_JUCE_DIR=/Users/uzanj/Downloads/JUCE -DCMAKE_BUILD_TYPE=Debug
cmake --build build --target AudioProvenanceCapture_VST3 --config Debug
```

If JUCE is somewhere else, replace `/Users/uzanj/Downloads/JUCE` with that local checkout path.

### Expected Result

CMake configures cleanly and produces `Audio Provenance Capture.vst3` under the build artefacts directory.

### Local Result

Validated on 2026-05-19 with JUCE at `/Users/uzanj/Downloads/JUCE`. Configure and VST3 build completed successfully, and `codesign --verify --deep --strict` passed for the generated bundle.

### Likely Failure Modes

- JUCE is missing or `APW_JUCE_DIR` points at the wrong directory.
- Xcode Command Line Tools are missing or not selected.
- CMake is too old to support the project.
- macOS blocks writes to the selected build directory.

## Milestone B - Plugin Loads in Ableton Live

Codex cannot complete this milestone from the shell. It requires manual validation in Ableton Live.

### Manual Test

1. Build Milestone A.
2. Copy the built VST3 bundle into `~/Library/Audio/Plug-Ins/VST3/`.
3. Open Ableton Live.
4. Open `Live > Settings > Plug-Ins` or `Live > Preferences > Plug-Ins`, depending on Ableton version.
5. Enable VST3 system folders and rescan plug-ins.
6. Search for `Audio Provenance Capture`.
7. Insert it on an audio track.

### Expected Result

Ableton lists the plugin and opens a small editor showing the v0.1 pass-through status.

### Likely Failure Modes

- The VST3 bundle was copied to the wrong folder.
- Ableton has VST3 system folders disabled.
- Ableton needs a full rescan or restart.
- The plugin failed validation because the local build is stale or incomplete.

## Milestone C - Audio Passes Through Unchanged

Codex cannot complete this milestone from the shell. It requires manual validation in Ableton Live.

### Manual Test

1. Add a known audio clip to an Ableton audio track.
2. Play the clip without the plugin and note the audible level and meter behavior.
3. Insert `Audio Provenance Capture` on the same track.
4. Toggle the plugin on and off during playback.
5. For a stricter check, duplicate the track, put the plugin on only one copy, invert polarity on one track with Ableton Utility, and play both together.

### Expected Result

The normal listening test should sound unchanged. In the polarity-cancel test, matching audio should cancel to silence or near-silence.

### Likely Failure Modes

- Host routing differs between the two tracks.
- A gain, pan, warp, or Utility setting differs between the reference and plugin paths.
- The plugin is inserted on the wrong track.
- Mono/stereo routing does not match.

## Milestone D - Plugin Survives Playback Start/Stop and Project Reload

Codex cannot complete this milestone from the shell. It requires manual validation in Ableton Live.

### Manual Test

1. Insert the plugin on an audio track.
2. Start and stop playback repeatedly.
3. Loop a section for several minutes.
4. Save the Ableton set.
5. Close and reopen Ableton.
6. Reload the set and start playback again.

### Expected Result

Ableton reloads the set, the plugin remains inserted, the editor opens, and playback continues without crashes or audio interruption.

### Likely Failure Modes

- Ableton rescans and rejects an older copied bundle.
- The plugin bundle was moved or deleted after saving the set.
- A local debug build was replaced while Ableton was still open.
- The test set uses unsupported routing outside mono or stereo audio tracks.
