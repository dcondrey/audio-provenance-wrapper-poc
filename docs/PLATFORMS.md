# Platform and host support

Status vocabulary: **verified here** means run on the maintainer's macOS
machine (Darwin 27, arm64 host) with the commands shown. **CI only** means a
workflow exists (`.github/workflows/platform-matrix.yml`) but no maintainer has
seen it pass. **Unsupported** means no implementation exists. Nothing in this
document claims Windows or Linux correctness from a macOS build.

## Support matrix

| Component | macOS | Linux | Windows |
| --- | --- | --- | --- |
| Plug-in, VST3 | verified here | CI only (experimental leg) | CI only (experimental leg) |
| Plug-in, AU | verified here (builds; Apple-only by design) | not applicable | not applicable |
| Plug-in, LV2 | builds with `-DAPW_BUILD_LV2=ON` (verified here; never loaded in a host) | enabled by default, CI only | enabled by default, CI only; JUCE refuses it on an Arm64 host |
| Plug-in, AAX | unsupported | unsupported | unsupported |
| Plug-in, Standalone | not built | not built | not built |
| Plug-in real-time and lifecycle tests (`ctest`) | verified here | CI only (under `xvfb-run`) | realtime test and source audit only; lifecycle harness not built |
| Python daemon and pytest | verified here | CI only | CI only |
| Sample watcher, WAV/AIFF metadata | verified here (portable readers) | portable readers, CI only | portable readers, CI only |
| Sample watcher, MP3/M4A metadata | via `afinfo` | reported as unavailable (nulls plus a note) | reported as unavailable (nulls plus a note) |
| Hardware attestation | `SoftwareProvider`, not attested | `SoftwareProvider`, not attested | `SoftwareProvider`, not attested |
| Input capture | unsupported | unsupported | unsupported |
| Screen observer | unsupported | unsupported | unsupported |
| Project diff | `.als`, `.rpp` | `.als`, `.rpp` (path handling only) | `.als`, `.rpp` (path handling only) |
| `scripts/build_plugin.sh` | verified here | syntax-checked on macOS, unrun | not applicable (bash script) |

"Portable readers" are pure-Python RIFF/WAVE and AIFF/AIFC header parsers.
They are tested against synthetic files with `platform.system` monkeypatched to
each OS; they have not run on a real Windows or Linux interpreter.

## Plug-in formats

Formats are chosen in `CMakeLists.txt` (`APW_PLUGIN_FORMATS`). JUCE 8.0.15's
`FORMATS` accepts `Standalone Unity VST3 AU AUv3 AAX VST LV2`; CLAP comes from
free-audio/clap-juce-extensions; LADSPA and DSSI are separate shims
(`docs/LADSPA_DSSI.md`).

| Format | Status | How | Verified here |
| --- | --- | --- | --- |
| VST3 | built by default, all OS | | macOS build and ctest |
| AU (`aumf`) | built by default, Apple only | JUCE emits `aumf` (MusicEffect) because the plug-in takes MIDI input | macOS build; type read from JUCE's `JUCEUtils.cmake` |
| CLAP | opt-in `-DAPW_BUILD_CLAP=ON` | clap-juce-extensions, pinned commit | macOS build produces `Audio Provenance Capture.clap`; never loaded in a CLAP host |
| LV2 | default off Apple, opt-in on Apple | JUCE | macOS build only |
| AUv3 | opt-in `-DAPW_BUILD_AUV3=ON`, Apple, Xcode generator | JUCE | not built: `juceaide` failed to configure under Xcode 27 with the Xcode generator on this machine |
| VST2 | opt-in `-DAPW_VST2_SDK_PATH=` | needs the legacy Steinberg SDK, which Steinberg stopped licensing in 2018 | only the missing-path guard |
| AAX | opt-in `-DAPW_AAX_SDK_PATH=` | needs Avid's SDK; a loadable Pro Tools build also needs PACE/iLok signing | only the missing-path guard |
| LADSPA, DSSI | opt-in shims | see `docs/LADSPA_DSSI.md` | see that document |

Not buildable, and no stub is provided:

- **RTAS, AudioSuite, TDM**: Avid retired them with Pro Tools 10 (32-bit) and
  no SDK is available; TDM also targets DSP hardware. AAX is the successor.
- **MAS (MOTU Audio System), MOTU FreeForm, Sound Designer II**: MAS and
  FreeForm are discontinued Mac OS 9-era formats with no available SDK; SDII
  is an audio file format, not a plug-in interface.
- **MASH/ProPlugin**: no public SDK exists that this repository can build
  against.
- **DirectX / DXi**: legacy 32-bit DirectShow-era interfaces; current Windows
  hosts load VST3 or CLAP.
- **LADSPA v2**: there is no such standard. LV2 is LADSPA's successor and is
  built.
- **Rack Extension (Reason)**: needs Reason Studios' SDK and runs sandboxed
  without network or file access, so it cannot emit events to the daemon.
- **Max for Live devices (`.amxd`)**: a Max patcher, not a native plug-in;
  Max loads the VST3/AU directly, and an `.amxd` would need Max to author and
  verify. The `ableton_bridge/` Live API probe is a separate observation path.
- **JSFX**: the runtime has no sockets, so a JSFX cannot send events to the
  daemon. REAPER, its only host, loads VST3 and CLAP.

Installers let users pick which formats to install: see `docs/INSTALL.md`.

### Building on Linux

JUCE's documented packages, without webkit and curl (disabled by
`JUCE_WEB_BROWSER=0` and `JUCE_USE_CURL=0`):

```sh
apt-get install libasound2-dev libjack-jackd2-dev ladspa-sdk \
  libfreetype-dev libfontconfig1-dev libx11-dev libxcomposite-dev \
  libxcursor-dev libxext-dev libxinerama-dev libxrandr-dev libxrender-dev \
  libglu1-mesa-dev mesa-common-dev xvfb
cmake -S . -B build -DBUILD_TESTING=ON -DCMAKE_BUILD_TYPE=Release
cmake --build build --target AudioProvenanceCaptureTests AudioProvenanceCapture_VST3
xvfb-run -a ctest --test-dir build -C Release --output-on-failure
```

### Building on Windows

```sh
cmake -S . -B build -DBUILD_TESTING=ON
cmake --build build --config Release --target AudioProvenanceCaptureTests AudioProvenanceCapture_VST3
ctest --test-dir build -C Release --output-on-failure
```

`AudioProvenanceHeadlessHostTests` is not defined on Windows: it replaces global
`operator new/delete` and measures blocks with `malloc_size` (macOS) or
`malloc_usable_size` (Linux) and has no Windows allocator port.

## Daemon behavior off macOS

- **Sample watcher**: WAV and AIFF headers are parsed without platform tools
  (float and extensible WAV included). `afinfo` is invoked only when
  `platform.system()` is `Darwin`. When no reader applies, `audio_metadata`
  fields are `null` and the event `notes` gain an "Audio metadata unavailable"
  line; the event is still recorded. The default watch folder
  `~/Music/ProvenanceSamples` is created on demand and is the same string on
  every OS; pass `--watch-dir` to change it.
- **Hardware attestation**: `detect_provider()` returns `SoftwareProvider` on
  every OS. `attestation_status()` reports `hardware_attested: false` and
  `proof_level: unknown_unobserved`, and names any Secure Enclave or TPM
  candidate as "not integrated". No Secure Enclave, TPM, or Windows platform
  crypto provider is used. On Windows the signing-seed file's permissions are
  not managed (POSIX mode bits do not express ACLs) and a warning says so.
- **Input capture and screen observer**: no backend exists on any OS. Their
  entry points print a JSON `unsupported_platform` status and exit with code 2.

## Project formats (`daemon/project_formats/`)

The registry maps a project path to a parser or an explicit refusal, by
extension. `--project` on the daemon and the standalone project differ both use
it. `.als` output is unchanged; other supported formats add a `project_format`
field to `project_diff` events.

| Host | Extension | Status |
| --- | --- | --- |
| Ableton Live | `.als` | supported (gzip XML; existing parser) |
| REAPER | `.rpp` | supported: tracks, items, sources, FX chain names, envelope points, MIDI note-ons, markers, tempo, time signature, sample rate. The RPP layout is unofficial and the parser is validated only against hand-written fixtures in `tests/fixtures/reaper/`, not files written by REAPER. Item positions are converted to beats with the base tempo, so a tempo envelope makes them approximate. |
| Logic Pro | `.logicx`, `.logic` | unsupported |
| Cubase, Nuendo | `.cpr`, `.npr` | unsupported |
| FL Studio | `.flp` | unsupported |
| Pro Tools | `.ptx`, `.ptf` | unsupported |
| Bitwig Studio | `.bwproject` | unsupported (layout not verified) |
| Studio One | `.song` | unsupported (layout not verified) |
| Cakewalk | `.cwp` | unsupported |
| GarageBand | `.band` | unsupported |

A watched unsupported project yields one `project_format_unsupported` evidence
record (`proof_level: unknown_unobserved`) and no structure. Parsers treat
input as untrusted: 64 MiB file cap, nesting, block and line caps for RPP, a
256 MiB decompression cap and DOCTYPE refusal for `.als`, and no XML entity
processing for RPP.

## Host applications (plug-in side)

Host recognition uses `juce::PluginHostType`, keyed on the host executable.
Unrecognised hosts are recorded with `host_recognised: false` and an empty name,
and the wrapper format (VST3, AudioUnit, LV2, ...) is recorded separately.
JUCE 8.0.15 names, among others: Ableton Live (6 to 11 and a generic entry; Live
12 has no dedicated value), Ardour, Bitwig Studio, Cubase and Nuendo, FL Studio
("FruityLoops"), Logic, GarageBand, MainStage, Pro Tools, Reaper, Reason,
Renoise, Studio One, Tracktion Waveform, Cakewalk, Digital Performer.
Recognition by JUCE says nothing about whether this plug-in has been loaded in
that host; only the following have any evidence in this repository:

- Ableton Live: exercised via the Ableton bridge documentation
  (`docs/ABLETON_BRIDGE_VALIDATION.md`), macOS.
- The headless lifecycle harness models a generic host, not a specific DAW.

Every other host, and every host on Windows and Linux, is untested.

## Unverified on Windows and Linux

- That the plug-in configures, compiles, links or loads (VST3 or LV2).
- That `AudioProvenanceCaptureTests`, the source audit, and (Linux) the
  headless lifecycle test pass, including under `xvfb-run`.
- LV2 manifest generation and any LV2 host loading it.
- That pytest passes; no test suite run has occurred on either OS.
- The portable WAV/AIFF readers against files from real tools.
- The Windows branch of key-file handling on a real Windows filesystem.
- Path handling for project files and sample folders with Windows separators.
- `scripts/build_plugin.sh` on Linux.
- That any DAW on those platforms loads the plug-in.
