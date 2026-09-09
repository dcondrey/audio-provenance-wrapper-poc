# Validation

This page documents manual validation for the JUCE VST3 proof-of-concept milestones.

## Core hardening gates (2026-09-01)

The automated plug-in gate builds the JUCE lifecycle/pass-through test host and VST3, runs a
callback-thread allocation probe, checks bit-identical mono/stereo float and double pass-through
across block sizes 1 through 8192 (including bypass), exercises FIFO saturation, sample-rate and
block-size changes, multiple-instance isolation, malformed/oversized state, ACK restart/scope
logic, daemon startup ordering/restart, state-chunk save/delete/reload, rapid offline callbacks,
plug-in deletion, exact FIFO conservation and bounded shutdown, then runs a static forbidden-call
reachability audit. A scheduled self-hosted CI job runs this host for eight hours and replaces the
manual Ableton soak. Ableton-only compatibility checks are listed in `PLUGIN_REALTIME_AUDIT.md`.

The watermark fast gate covers the local-region/interior-gap predicate, signed duration and block
distribution, discontinuity detection, actual-audio interior substitution, crop recovery, and the
permanent adversarial-plan schema. The parallel full-corpus runner and its measured classical
Watermark baseline live under `sdk/qualification/reports/classical-apw-watermark/`. The 2026-09-01
baseline is complete but does not meet every declared quality target; the report preserves those
failures rather than promoting the implementation.

The development adapter gate starts a temporary capture session, accepts UDP observations, detects
an export, builds and signs the handoff/bundle, invokes the Rust SDK through the Python
orchestrator, attaches a development record, verifies it with the public SDK, and asserts unchanged
proof levels and evidence hashes. It intentionally reports identity `not_established` and writes
the post-sign result to an atomic receipt rather than mutating the signed capture manifest.

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
  still holds the prior binary inode after the new install. At that point no
  current-build rescan, UI observation, null test, reload, or real export had
  been observed. The live pass recorded below later closed all of these except
  the null test and reload.

Live Ableton validation on 2026-08-28:

- A manual pass in Ableton Live on the demonstration machine completed the
  one-stem path end to end. The evidence is session
  `capture-20260828T221446Z-9959`: a real WAV export, manifest, fight card,
  signed bundle and index, and a stored local verifier outcome of `verified`.
- Coverage graded `complete_observed_path`; routed/export association graded
  `inferred_match`.
- The transparency/null test and project save/close/reload were not performed
  during this pass and are not marked complete.

Manual v1.0 gate on the demonstration machine (evidence for completed steps:
session `capture-20260828T221446Z-9959`, 2026-08-28):

1. [x] Quit Ableton completely.
2. [x] Reopen it and rescan the installed VST3.
3. [x] Insert **Audio Provenance Capture** on one routed stem.
4. [x] Confirm current plug-in instance and capture-session identifiers in the dashboard.
5. [x] Confirm locally emitted and daemon-acknowledged counts advance during playback.
6. [x] Complete the documented polarity/null transparency test. (2026-08-29)
7. [x] Save, close, and reload the Ableton project. (2026-08-29, after the export sealed)
8. [x] Export a real WAV/AIFF into the watched folder.
9. [x] Confirm inferred alignment, coverage, sealing, bundle creation, and verification.
10. [x] Save the final real-session manifest and fight card.

The gate is closed. Exact per-step recovery instructions are in
`docs/DEMO_RUNBOOK.md`.

No automated or synthetic result is recorded as manual Ableton validation.

Live Ableton validation on 2026-08-29 (gate steps 6-7 and runtime verification
of the 2026-08-29 fix batches; rebuilt plug-in installed, binary
`f7e32e45a0f7…`):

- Transparency/null test (step 6): the routed stem was duplicated, the plug-in
  left on only the original, and the duplicate polarity-inverted with the
  Utility "Phase Invert" preset (Ø L + Ø R). During playback both source
  meters ran hot while the Main meter stayed dark. A 24-second 32-bit-float
  offline render of the Main bus contained 2,116,800 samples, every one
  exactly 0.0 (peak -inf dBFS): the plug-in path is bit-identical on the
  routed path.
- Save/close/reload (step 7): the set was saved as
  `demo-output/apw-live-set/apw-live-verification Project/apw-live-verification.als`,
  Live was quit completely and relaunched, and the set reopened. The reloaded
  plug-in instance (`plugin-092972dcd532`) streamed genesis-clean into the
  same daemon session (contiguous acknowledgements, zero gaps, zero chain
  breaks, zero alerts). Performed after the graded export sealed; a
  mid-session reload adds a second instance, and later status honestly grades
  `partial_observed_path` for the two-instance remainder, as the runbook
  documents.
- Graded pass (session `capture-20260830T021208Z-11629`): plug-in loaded fresh
  after daemon start, 16-bit export `session-b-take2.wav` graded coverage
  `complete_observed_path`, association `inferred_match` (confidence 0.8538,
  211/258 windows, routed coverage 0.332), verifier `verified` (12 checks, 2
  expected warnings), session facts populated from the saved `.als` (6 tracks,
  BPM 120), `hardware_binding` present and the hardware cosignature nested
  inside `manifest_signature`. A first take rendered as 32-bit float WAV
  (format 3) was recorded honestly as association `unavailable` with reason
  `unsupported WAV format`; the association reader accepts PCM WAV/AIFF only.
- H-011 (unsupported-MIDI counter), session `capture-20260830T015947Z-85675`:
  17 pitch-bend plus 5 aftertouch messages injected over a virtual CoreMIDI
  port produced `midi_unsupported_dropped: 22` in telemetry (exact count),
  degraded live coverage to `partial_observed_path`, and rendered the
  dashboard alert for unsupported MIDI types.
- H-018 (missing/pending panel): with the plug-in running before any daemon,
  the panel showed `Daemon receipt: UNKNOWN … missing/pending` equal to the
  full prepared backlog beside `send failed 0`; after a daemon started
  mid-stream it showed `ACKNOWLEDGED / CHAIN BREAK · accepted through 3700 ·
  contiguous through 0 · missing/pending 0` with ACK gaps 1770, and the daemon
  logged the non-genesis chain start. In the clean session the same panel
  reached `contiguous through 783 · missing/pending 0`. The
  failed-send arm of the old defect is not reachable live (unconnected UDP
  send to a dead localhost port reports success), so that arm remains
  code-review-verified only.
- M-022 (per-channel CC keying): a 3-second CC71 ramp aggregated into one
  `parameter_change` record (channel, CC, start/end value, change count all
  correct). Cross-channel merging is not reachable through Ableton track
  routing: notes sent on channels 5, 2, and 3 all arrived at the plug-in on
  channel 1 with the MPE input flag both off and on, and MPE-mode CC74 never
  reached processBlock as a controller. Live flattens routed MIDI to channel
  1 for this plug-in, so the (channel, CC) keying's cross-channel arm stays
  covered by code review and the C++ inspection only.
- M-023 (teardown flush): with a CC71 knob gesture mid-flight (94 of 240
  planned changes), the plug-in device was deleted; the final event on the
  wire for that stream was the flushed `parameter_change` (start 0, end 93,
  change_count 94) after the final window drains. Shutdown loss is accounted
  rather than silent.
- Bonus alert paths observed live: the FIFO-overflow alert fired with exact
  recovery text after an offline render with the device active
  (253,953 samples / 497 windows dropped), and the readiness panel showed
  READY TO EXPORT with a 14-second minimum against 55.4 routed seconds in the
  clean session.

## External C2PA validation (2026-08-31)

Validated with `c2patool 0.26.68`, the reference C2PA command-line tool, run
outside our pipeline. It shares the c2pa-rs core with the `c2pa-python` binding
that wrote these manifests, so this is a real tool reading our output, not two
independent implementations agreeing. Round-tripping inside our own writer
proves less than this; a second implementation would prove more.

- Pipeline output
  `demo-output/founder-package/evidence-package-20260831T091551Z/artifacts/presenter_export_c2pa.wav`,
  carrying `c2pa.actions.v2`, a `c2pa.ingredient.v3` with `apw:` metadata keys,
  and the non-registered `apw.unobserved` assertion. With no trust anchor:
  `validation_state: Valid`, every `assertion.hashedURI.match` (including the
  one over `apw.unobserved`) and `assertion.dataHash.match` succeeding, and
  `signingCredential.untrusted` the sole failure. Re-run as
  `c2patool <asset> trust --trust_anchors ~/.apw/provenance/ca/root_cert.pem`:
  `validation_state: Trusted`, zero failures, one informational
  `ingredient.unknownProvenance` for the routed stem, which carries no manifest
  of its own.
- Shipped demo asset
  `packaging/assets/demo-project/apw-demo Project/Samples/Imported/apw-demo-signed.wav`
  against the bundled `packaging/assets/demo-root-ca.pem`:
  `validation_state: Trusted`, zero failures.

Scope: the embedded WAV path only. The AIFF `.c2pa` sidecar has no c2pa-rs
handler and was not externally validated. `signingCredential.untrusted` without
an anchor is the expected result for a self-issued chain, not a defect.

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
