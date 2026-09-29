# LADSPA and DSSI formats

`audio_provenance_capture_ladspa.so` and `audio_provenance_capture_dssi.so` are
shared modules that host the same `AudioProvenanceCaptureAudioProcessor` as the
VST3: the same `AudioObserver` hash chain, the same UDP events to the daemon on
`127.0.0.1:9876`, and the same audio pass-through. They are opt-in
(`-DAPW_BUILD_LADSPA=ON`, `-DAPW_BUILD_DSSI=ON`, both default OFF) and are
defined in `cmake/ladspa_dssi.cmake`.

There is no "LADSPA v2". LADSPA 1.1 is the last version of that API; LV2 is its
successor and is built separately.

## Behaviour

| Aspect | Behaviour |
|---|---|
| Ports | 4 audio ports: `Input L`, `Input R`, `Output L`, `Output R`. No control ports. |
| `instantiate` | Creates the processor and calls `prepareToPlay(rate, 8192)`. LADSPA makes `activate` optional, so an instance is runnable without it. |
| `activate` | Resets the observation era (`reset()`), or re-runs `prepareToPlay` after a `deactivate`. |
| `run` | `processBlock` on the output buffers after the input has been copied across. Splits runs longer than 8192 frames. Zero-length runs return immediately. |
| `deactivate`, `cleanup` | `releaseResources()` marks a lifecycle discontinuity in the hash chain. |
| Aliasing | `in == out` (in place) is a no-op copy. Partly overlapping or swapped buffers are staged through a preallocated scratch so no input is overwritten before it is read. |
| Unconnected ports | Connected channels are still copied (or zeroed) but nothing is observed. |
| MIDI (DSSI) | `run_synth` and `run_multiple_synths` map NOTE_ON/OFF, KEYPRESS, CONTROLLER, CHANPRESS and PITCHBEND to MIDI and pass them to `processBlock`, so routed MIDI observations work. Event times are frame offsets from the start of the run, re-based per chunk; times past the run are pinned to its last frame. At most 1024 events per processed block are forwarded. |
| `host_environment` | `wrapper_format` is `"LADSPA"` or `"DSSI"`. JUCE has no `WrapperType` for these, so the processor constructor takes an optional `wrapperFormatOverride` (default null, existing formats unchanged). |

`run` allocates nothing and takes no lock in the shim. `scripts/audit_plugin_realtime.py` does not cover the shim (`CaptureInstance::run` and `processChunk`); the evidence for it is the `operator new` counter in the `*_realtime_static` tests, which does not see `malloc` or locks. The processor's own
audio-thread functions are covered by `scripts/audit_plugin_realtime.py`. The
descriptors do not claim `LADSPA_PROPERTY_HARD_RT_CAPABLE`: `CaptureInstance`
uses only preallocated buffers, but the claim would rest on the processor and
the platform's UDP stack staying that way on every host.

### Session identity

LADSPA and DSSI have no GUI and no state save or restore. The instance and
capture-session ids are generated in the processor constructor, so every
`instantiate` is a new session; a host project reload starts new sessions
rather than resuming the old one. There is no consent, signing-arm or
verification UI: the daemon receives events, and anything that needs the editor
is unavailable in these formats.

### Limits

- Nothing pumps JUCE's message loop in a LADSPA/DSSI host. The processor, observer and emitter use no Timer, AsyncUpdater or `callAsync` (checked in `src/`, editor excluded), so capture is unaffected.

- Stereo only; a mono track needs the host to provide both channels.
- Unique IDs are `0x415043` (LADSPA) and `0x415044` (DSSI), in the range the
  spec reserves for development. No ID is registered.
- DSSI programs, `configure`, MIDI controller mapping and the `_adding`
  variants are not provided (all NULL). DSSI bank and program change events are
  not delivered by DSSI hosts through `run_synth` anyway.
- The modules link JUCE statically (hidden symbols, only the descriptor
  functions are exported). `dlclose` of a JUCE module with live worker threads
  is host-dependent; a host should not unload while instances exist.
- The daemon port is fixed at 9876, as in the VST3.

## Building

    cmake -S . -B build-ladspa -DAPW_JUCE_DIR=<juce> -DBUILD_TESTING=ON \
          -DAPW_BUILD_LADSPA=ON -DAPW_BUILD_DSSI=ON
    cmake --build build-ladspa
    ctest --test-dir build-ladspa -R 'ladspa|dssi'

Install by copying the `.so` files to a `LADSPA_PATH` / `DSSI_PATH` directory
(for example `~/.ladspa`, `~/.dssi`). `snd_seq_event_t` comes from
`<alsa/seq_event.h>` when ALSA development headers exist; otherwise the
layout-compatible `src/dssi/compat/alsa/seq_event.h` is used (macOS, CI).

## Tests

- `ladspa_module_host`, `dssi_module_host`: a console host that `dlopen`s the
  built module, walks descriptors, instantiates two instances, feeds a
  deterministic signal (negative zero, denormals, NaN, infinities included) in
  blocks of 64, 1, 0, 333, 512, 8192, 8193, 17, 4096, 20000 and 129 frames, and
  requires bit-exact output out of place, in place and with swapped buffers. It
  binds UDP 127.0.0.1:9876 and requires `host_environment` with the right
  `wrapper_format`, `buffer_hash` events, distinct session ids per instance
  and, for DSSI, `midi_event`s.
- `ladspa_realtime_static`, `dssi_realtime_static`: the same checks with the
  entry points linked in-process and a global `operator new` counter, requiring
  zero allocations on the run thread.
- The tests need port 9876 free, so stop a running daemon first.

Not covered by these tests: real Linux hosts (Ardour, Carla, jack-dssi-host,
Qtractor), ALSA's own `seq_event.h`, and concurrent instantiation across host
threads.

## Third-party headers and licensing

| File | Source | Version |
|---|---|---|
| `src/ladspa/ladspa.h` | https://www.ladspa.org/download/ladspa_sdk_1.17.tgz, `ladspa_sdk_1.17/src/ladspa.h` | LADSPA 1.1, SDK 1.17. SHA-256 `c72ceb7383f159a944bfe80b1b155795857026aea1155dbe4ecf1664354320ad` |
| `src/dssi/dssi.h` | https://downloads.sourceforge.net/project/dssi/dssi/1.1.1/dssi-1.1.1.tar.gz (via sourceforge.net/projects/dssi/files/dssi/1.1.1/), `dssi/dssi.h` | DSSI 1.0 API, package 1.1.1. SHA-256 `ee3ddcce00a9c5d242d342d6f73e83041c71a755c2521f5fb67e08bb30522265` |
| `src/ladspa/LICENSE.LGPL-2.1` | `COPYING` from the same DSSI 1.1.1 tarball (SHA-256 `d3c9f4554edb732f13ac5fc26331933130153876ad8af6e648ad001197cdbac6`) | GNU LGPL 2.1 |

Both headers are copied unmodified. The canonical LADSPA site is ladspa.org;
DSSI's upstream is dssi.sourceforge.net. No git revision exists for either (the
upstream projects publish tarballs; GitHub mirrors were not used).

Each header carries this notice, verbatim: "This library is free software; you
can redistribute it and/or modify it under the terms of the GNU Lesser General
Public License as published by the Free Software Foundation; either version 2.1
of the License, or (at your option) any later version." Copyright is
2000-2002 Richard W.E. Furse, Paul Barton-Davis, Stefan Westerfeld (ladspa.h)
and 2004, 2009 Chris Cannam, Steve Harris, Sean Bolton (dssi.h). The full
licence text is `src/ladspa/LICENSE.LGPL-2.1`, the DSSI tarball's `COPYING`; the
LADSPA SDK tarball ships no separate licence file, and both headers name the
same LGPL 2.1-or-later grant, so that one text covers both.

Compatibility with `LICENSING.md`: the two headers are third-party files under
the LGPL 2.1 and stay under it; they are not covered by that file's "all
rights reserved" rows for `src/`. The shim only includes them, and they hold
type declarations and macros with no functions, which the LGPL treats as a work
that uses the library without restrictions on the including code (LGPL 2.1,
section 5). `LICENSING.md` has no row for them; adding one is a decision for
its owners and was not done here. The existing JUCE AGPL-or-commercial
constraint on distributing a binary that links JUCE applies to these modules
exactly as it does to the VST3, and is unchanged.
