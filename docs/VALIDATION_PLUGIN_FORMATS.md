# Plug-in format validation

All runs on 2026-09-29, macOS 27.0 (Darwin 27.0.0), Xcode 27.0 (27A266a), Apple Silicon,
arm64-only builds, source tree at branch `feat/verify-time-anchor-parity-platforms`.
Validators and logs live under `/Volumes/C/apw-val-tools`; builds under `/Volumes/C/apw-val-plug`
(VST3, AU, CLAP; Makefiles, Release, `-DCMAKE_OSX_ARCHITECTURES=arm64`) and
`/Volumes/C/apw-val-auv3` (Xcode generator).

| Format | Validator | Version | Result |
| --- | --- | --- | --- |
| VST3 | pluginval | 1.0.4 | Pass at strictness 5 and at 10 (the maximum) |
| AU (`aumf ApCa ApPr`) | pluginval | 1.0.4 | Pass at strictness 5 and 10 |
| AU | `auval` | 1.10.0 | `AU VALIDATION SUCCEEDED` |
| AUv3 (out-of-process) | `auval` | 1.10.0 | `AU VALIDATION SUCCEEDED`, log reports "Loaded AudioUnit out-of-process: true" |
| CLAP | clap-validator | 0.4.1 | 44 run: 31 passed, 1 failed, 1 warning, 11 skipped (details below) |

## Commands

pluginval: release v1.0.4 asset `pluginval_macOS.zip`, sha256
`3c4c533bda0c5059eea3ddaea752d757ee2025041f0f47e6bcb0e87f6082b29f`. The release page publishes
no checksum; the app is Developer ID signed and notarized by Tracktion Software Corp
(`spctl -a -vv`: accepted, Notarized Developer ID, team YLB5W3GE5T).

```
pluginval.app/Contents/MacOS/pluginval --strictness-level {5,10} --validate-in-process \
  "<build>/AudioProvenanceCapture_artefacts/Release/VST3/Audio Provenance Capture.vst3"   # exit 0
pluginval.app/Contents/MacOS/pluginval --strictness-level {5,10} --validate-in-process \
  "~/Library/Audio/Plug-Ins/Components/Audio Provenance Capture.component"                 # exit 0
```

The AU must be installed in a Components directory: pointed at the build-tree path, pluginval
reports "No types found" (exit 1) because macOS does not know that AU. For these runs it was
copied to the user-scope `~/Library/Audio/Plug-Ins/Components`, validated, and removed again.

auval (AU, freshly rebuilt from the current tree, and AUv3 registered with
`pluginkit -a` on the appex inside the container app, then `pluginkit -r`):

```
auval -v aumf ApCa ApPr    # exit 0, both times
```

The AU and AUv3 share one type/subtype/manufacturer, so they were validated one at a time, never
installed together.

clap-validator: source tag `0.4.1` (commit `152b9823e992d782c5c1fd33bca0295478b919aa`), built with
`cargo build --release --locked` (exit 0).

```
clap-validator validate "<build>/AudioProvenanceCapture_artefacts/Release/CLAP/Audio Provenance Capture.clap"   # exit 1
```

### clap-validator detail

- **Failed, `param-conversions`: "attempt to divide by zero. This is a bug in the validator"**
  (`src/tests/plugin_instance/params.rs:82`, `4000usize.div_ceil(param_info.len())`). The plug-in
  exposes no parameters (`src/` has no `AudioParameter`/`addParameter`), and clap-validator 0.4.1
  divides by the parameter count. This is a validator crash on a zero-parameter plug-in, not a
  plug-in conformance failure, but it makes the run exit 1. Not verified against another
  validator version.
- **Warning, `state-invalid-random`**: `state::load()` accepts 3 x 1 MB of random bytes and
  returns success without crashing. `setStateInformation` (`src/PluginProcessor.cpp:406`) is
  `void` and resets to safe defaults on rejected input by design, so the JUCE CLAP wrapper has no
  failure to report; the warning is expected and not a defect in the safety behavior.
- **Skipped (11)**: preset-discovery (3), 64-bit audio (2), note-wildcard, audio-ports-activation,
  audio-ports-config, configurable-audio-ports, and `param-set-events` / `param-set-no-cookies`
  (no automatable parameters). All are extensions the plug-in does not implement.
- The wrapper's "CLAP asked for plugin_id '...capturex1'" line is printed by
  clap-juce-extensions when the validator probes a nonexistent id; the associated tests passed.

## Build fixes found while validating

- Xcode generator (AUv3): the default deployment target 11.0 is rejected by Xcode 27's build
  system (supported range 12.0 to 27.0), which fails the compiler-identification project.
  That surfaced as "No CMAKE_C_COMPILER could be found" in JUCE's `juceaide` sub-configure. Cause
  read from `<build>/JUCE/tools/CMakeFiles/CMakeConfigureLog.yaml`. `CMakeLists.txt` now defaults
  to 12.0 under the Xcode generator; other generators keep 11.0.
- AUv3 then failed to link (`AUViewController` undefined, CoreAudioKit) and JUCE's Standalone
  container needs `juce_audio_utils` and `juce_audio_devices`. Under `APW_BUILD_AUV3=ON` the
  build now adds the Standalone format (the container app) and links those two modules.

## `pkgbuild` "write: Permission denied"

`pkgbuild` on this OS prints four `write: Permission denied` lines per invocation and still exits
0 and writes the package. It reproduces with no scripts, no component plist, and a one-file payload
(`pkgbuild --root <dir with one file> --identifier x.t out.pkg`), with the sandbox disabled, with
stdin closed, and under a pty. No child process is spawned (polled during a 1 GB payload) and a
`write` shim on `PATH` was never called, so the message comes from inside Apple's `/usr/bin/pkgbuild`
(a `perror("write")`-style EACCES; debugger attach is refused for the platform binary, so the
exact call was not traced). The resulting component's payload list matches the source root exactly
(`pkgutil --payload-files`). It is not caused by `scripts/package_pkg.sh`, which was left unchanged
so real pkgbuild errors are not filtered out.

## Not validated

- AU and AUv3 were built arm64 only; no x86_64 or universal slice was validated, and neither has
  been loaded in a real host (Logic, GarageBand, Ableton Live, etc.).
- VST3: only pluginval. Steinberg's own `validator` (VST3 SDK) was not run. Not loaded in a DAW.
- CLAP: not loaded in a CLAP host; the parameter-conversion, and 64-bit and preset-discovery
  paths are unexercised (see above).
- Signed and notarized builds: all runs used ad-hoc signed development builds; the Developer ID
  signed artefacts from `scripts/package_installer.sh` were not validated.
- LV2, LADSPA, DSSI (no lv2lint / lv2-validate), VST2 and AAX (SDKs unavailable): no validator run.
- Windows and Linux: nothing here ran there.
- pluginval ran in-process (`--validate-in-process`); the out-of-process default was not run.
