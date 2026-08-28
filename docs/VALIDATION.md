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

Manual v1.0 gates still to record on the demonstration machine:

- [x] Current VST3 configures and builds from a clean build directory.
- [ ] Ableton Live discovers and opens the current VST3.
- [ ] A polarity/null test confirms transparent pass-through.
- [ ] Playback, stop/start, project save, and project reload remain stable.
- [ ] A real Ableton export produces a fight card with routed-audio evidence.
- [ ] `python3 -m daemon.verify` passes against that real manifest.

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
