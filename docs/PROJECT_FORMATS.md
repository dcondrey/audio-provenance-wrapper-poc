# Project formats

Registry: `daemon/project_formats/`. A format is supported only when it is open
or documented, its extraction is grounded in a primary spec or the app's own
source, and its test input has stated provenance. Each supported entry carries a
`validation` field: `real_files` or `constructed_fixtures_only`. Do not read
more confidence into a snapshot than that field states.

Watcher behaviour is unchanged: the parsers return the same `ProjectSnapshot`
the REAPER parser returns, `project_format` is added to the diff event for
non-`.als` projects, and the Ableton event schema is untouched.

## Supported

| format_id | extensions | validation | grounding |
|---|---|---|---|
| `dawproject` | `.dawproject` | constructed_fixtures_only (real `project.xml`, built zip) | `Project.xsd` and README of github.com/bitwig/dawproject @ `ee4dcdd` (MIT) |
| `ardour` | `.ardour` | constructed_fixtures_only | Ardour source @ `4e6e9fd` (GPL-2+), `libs/ardour`, `libs/temporal` |
| `lmms` | `.mmp`, `.mmpz` | constructed_fixtures_only | LMMS source @ `a2f57e7` (GPL-2+), `src/core`, `src/tracks`, `include/` |
| `pure_data` | `.pd` | real_files | Pd source @ `43a5b89` (BSD), `src/g_readwrite.c` |
| `max_patcher` | `.maxpat` | constructed_fixtures_only | no spec; keys observed in one Max 7 file (see below) |
| `milkytracker` | `.xm`, `.mod` | real_files | FastTracker 2 XM layout and MilkyTracker `LoaderXM.cpp`/`LoaderMOD.cpp` @ `e3ceb05` (GPL-3+, cited only) |
| `vcv_rack` | `.vcv` | constructed_fixtures_only | VCV Rack v2.6.6 `src/patch.cpp`, `src/system.cpp`, `src/engine/{Module,Cable}.cpp` (GPL-3+, cited only) |
| `reaper_rpp` | `.rpp` | constructed_fixtures_only | pre-existing, unchanged |
| `ableton_als` | `.als` | constructed_fixtures_only | pre-existing; tests use synthetic gzip XML |

### DAWproject

Zip with `project.xml` per `Project.xsd`. Extracted: Transport tempo (only when
`unit="bpm"`) and time signature; nested `Structure/Track` (group tracks become
`FolderTrack`, `Channel@role` maps master/effect/submix/vca); devices as
`<element tag>: <deviceName>` from `Channel/Devices`; arrangement clips (the
outermost `Clip` under each track's `Lanes`, so a Bitwig audio clip counts once,
not once per audio event); `Audio/File@path` at any depth; `Note` count;
`Point` elements under a track's lanes; `Marker` count. `timeUnit` is inherited
down the timeline tree and defaults to beats (Reference.html); seconds are
converted with the project tempo. Plug-in `State@path` members are hashed into
the device fingerprint when present; no member is ever extracted to disk. Not
extracted: tempo/time-signature automation, scenes/clip launcher, mixer values.

Fixture `dawproject/basic.dawproject`: its `project.xml` is the example in the
bitwig/dawproject README, labelled there as a file saved by Bitwig Studio 5.0.
The zip container was built here (`build_fixtures.py`, fixed timestamps) and
holds only `project.xml`; the referenced audio and CLAP preset members are
absent. License text: `tests/fixtures/projects/dawproject/LICENSE.third-party`.
No real `.dawproject` file exists in that repository (`test-data/` has only a WAV).

### Ardour

Session XML: `Session@name/sample-rate`, `Sources/Source`, `Routes/Route`
(a route with `audio-playlist`/`midi-playlist` is a track; `PresentationInfo@flags`
is the track type), `Processor` elements with `unique-id` (only
`PluginInsert::state` writes it) as plug-ins, `Playlists/Playlist/Region`,
`Locations/Location`, `TempoMap` (first tempo and meter; later tempo changes are
ignored). Region `length` is a `timecnt_t` string `<a|b><dist>@<a|b><pos>`, so it
carries the position; `a` is superclocks (rate from `TempoMap@superclocks-per-second`;
without it superclock positions become 0.0 rather than a guess), `b` is ticks at
1920 per beat, and a bare integer is a legacy sample count. Loop is reported as
on when an auto-loop location exists. Not extracted: automation, MIDI notes
(MIDI data is in external files), plug-in state, gzip-compressed sessions
(refused).

Fixture `ardour/basic.ardour`: **constructed from**
https://github.com/Ardour/ardour/tree/4e6e9fd887392d751742424568dc50c1cdb135cb/libs/ardour
and `.../libs/temporal` (`session_state.cc`, `route.cc`, `track.cc`, `playlist.cc`,
`region.cc`, `source.cc`, `location.cc`, `tempo.cc`, `timeline.cc`). Ardour is GPL, so no Ardour-produced file is
vendored. It has never been compared with a file written by a real Ardour.

### LMMS

`.mmp` is XML with a bodyless `<!DOCTYPE lmms-project>` (allowed; any other
DOCTYPE or entity declaration is refused). `.mmpz` is `qCompress` output: a
4-byte big-endian length then a zlib stream. Like `DataFile::loadData`, plain
XML is tried first whatever the extension. Extracted: `head@bpm` (or the child
element form when automated), time signature, every `track` in document order
(pattern tracks nest their own `trackcontainer`), instrument and FX chain
devices, `midiclip`/`sampleclip`/`automationclip` and legacy `pattern`/
`sampletco`/`automationpattern`/`bbtco`, note and automation point counts,
`timeline@lp0pos/lp1pos/lpstate`, and the sampler file
(`instrument/audiofileprocessor@src`, per `AudioFileProcessor::saveSettings`) as a
track sample reference; other instruments' internal sample references (for
example drum-machine or sample-bank plug-ins) are not extracted. 48 ticks per
quarter note. Track ids are
positional (`pos-N`) because LMMS tracks carry no id. Only song projects are read.

Fixtures `lmms/basic.mmp` and `basic.mmpz`: **constructed from**
https://github.com/LMMS/lmms/tree/a2f57e70ce9c3468b4b6d21955bbe65a0989048a
(`src/core/DataFile.cpp`, `Song.cpp`, `Timeline.cpp`, `Track.cpp`, `MidiClip.cpp`,
`SampleClip.cpp`, `AutomationClip.cpp`, `EffectChain.cpp`, `src/tracks/*`,
`plugins/AudioFileProcessor/AudioFileProcessor.cpp`); the `.mmpz` is the same XML
through `qCompress` framing. GPL, so nothing from LMMS is vendored.
One-off local check, not committed: the real demo
`data/projects/demos/DnB.mmpz` at that revision (a real LMMS 1.3.0-alpha.1 file,
format version 22, legacy element names, nested pattern tracks, a CDATA notes
block containing an HTML DOCTYPE) parsed successfully with 174 bpm and its
nested tracks. That run found the need to walk nested track containers and to
refuse DTDs structurally rather than by substring.

### Pure Data

Plain text `;`-terminated messages with backslash escapes. Pd has no tempo,
clips or plug-in chain, so the mapping is: one track per canvas (root `main`,
subpatches by name, graphs), each `#X obj` a device (`class args`), atoms ending
in an audio extension (in `obj`/`msg`) as sample references. Box coordinates are
excluded from the device-chain hash (moving a box is not an edit); connections
are included.

Fixture `puredata/A01.sinewave.pd`: real, unmodified, from
github.com/pure-data/pure-data `43a5b89`, `doc/3.audio.examples/`. Pd's license
(Standard Improved BSD) is committed beside it as `LICENSE.third-party`.
Subpatch nesting, escapes and the caps are tested with in-test text.

### Max

JSON `{"patcher": {"boxes": [{"box": {...}}], "lines": [...]}}`, subpatchers
nested as a box's own `patcher`. Cycling '74 publishes no spec, so only keys seen
in a real Max 7 file are used (`help/dummy.maxhelp` in Cycling74/max-sdk
`15b6fe1`) and unknown keys are ignored. Same mapping as Pd (one track per
patcher, box text as devices, comments excluded, geometry keys excluded from the
hash).

Fixture `maxpat/basic.maxpat`: **constructed from**
https://github.com/Cycling74/max-sdk/blob/15b6fe17eedc7c8a8b4ee249706d3aaf21e192fa/help/dummy.maxhelp
(structure only; nothing copied). The max-sdk help file itself was not vendored: its MIT license states it
covers the SDK's headers and source examples, and whether that includes help
patchers is not clear. Format status: no formal spec, treat as best-effort.

### MilkyTracker modules (.xm, .mod)

Grounding: https://github.com/milkytracker/MilkyTracker @ `e3ceb05105e8c5bdc907d6451245bf5a1926b1f9`,
`src/milkyplay/LoaderXM.cpp`, `LoaderMOD.cpp`, `XModule.h` (limits), and the
FastTracker 2 XM layout they implement. Nothing is copied or vendored.
Content, not extension, picks the reader (`Extended Module:` means XM).

XM: version 0x0104 only (0x0102/0x0103 store instruments before patterns and
are refused). Header size is honoured and a short header is zero-padded as
`LoaderXM.cpp` does (the OpenMPT fixture has a 22-byte header size). Extracted:
module name, tracker name, channels (1..32), song length (clamped to 256),
restart, patterns (max 256; rows 1..256; packed data hashed, never unpacked),
instruments (max 255) with name and up to 96 samples each (name, length in
bytes), frequency table flag, ticks per row, BPM (`transport_bpm`). Sample data is
hashed, not decoded. A file that ends at an instrument boundary, or inside
sample data, is tolerated as MilkyTracker tolerates it and reported as a
`truncated` device.

MOD: 31-sample layout with a format tag at offset 1080 recognised as
`getPTnumchannels` does (`M.K.`, `M!K!`, `FLT4`, `FLT8`, `OKTA`, `OCTA`, `FA08`,
`CD81`, `nCHN`, `nnCH`, `nnCN`). Title, tag, channels, song length, restart,
pattern count (max order entry + 1), sample slots with used names/lengths.
Refused: 15-sample Soundtracker files (MilkyTracker detects them heuristically,
which is not reproduced), files without a tag, and files shorter than 1084
bytes. ModPlug ADPCM-packed samples are not recognised; sample-data hashes past
such a sample are positional. No tempo is stored, so `transport_bpm` is 0.0.

Snapshot mapping: a `module` track (name = title, devices = header facts), a
`patterns` track (order table plus pattern hashes), an `Instrument` track per
XM instrument or used MOD slot (devices = `name (N bytes)`).

Fixtures `milkytracker/test.xm`, `test.mod`: real, unmodified, the OpenMPT
project's own test modules, github.com/OpenMPT/openmpt @
`f83cedb0cd5446e4dfaa83ac97e3087107e26767`, `test/`. That repository's BSD-3-Clause
`LICENSE` (no separate notice covers `test/`) is committed as
`milkytracker/LICENSE.third-party`. MilkyTracker's own demo songs
(`resources/music`) were not used: no licence is stated for them.

### VCV Rack (.vcv)

Grounding: https://github.com/VCVRack/Rack tag v2.6.6 (object
`061ccf63c1758599396ac1bb10d47345d9d34076`). `Manager::load`/`isPatchLegacyV1`:
a file starting with the Zstandard magic `28 b5 2f fd` is a tar (libarchive
`pax_restricted`, entries named `./...`) containing `patch.json`; anything else
is a legacy Rack 1 JSON patch. Root keys `version`, `modules`, `cables` (legacy
`wires`); module `id`, `plugin`, `model`, `version`, `params`, `bypass` (legacy
`disabled`), `data`; cable `outputModuleId`, `outputId`, `inputModuleId`,
`inputId`. `version` must be a string or the file is refused.

Snapshot mapping: a `patch` track (container, Rack version, module and cable
counts), a `Module` track per module (`plugin/model`; devices: version, param
count, `data: present`, `bypassed`; hash over plugin, model, version, params,
data, bypass but not id, `pos` or expander links), and a `Cables` track (a device
per cable). Module `data` is untrusted and only hashed. Sample paths in `data`,
module asset folders and patch metadata such as `zoom` are not extracted.
No tempo (0.0).

Container rules (`_tarzst.py`, `tarzst.rs`, identical): exactly one Zstandard
frame that consumes the whole file (frame header and block headers are walked
first: no dictionary, reserved bits, windows over 128 MiB, block sizes, trailing
or missing bytes are refused), decoded under `MAX_TAR_BYTES` (256 MiB); tar is
ustar with pax `path`/`size`, checksum verified, regular files and directories
only (links, devices, FIFOs, GNU long names and pax globals are refused), names
refused when absolute, drive-lettered, NUL-bearing or containing `..` (`.`
segments dropped), regular-file names unique, at most `MAX_TAR_MEMBERS` (10 000)
entries; nothing is extracted to disk. Non-finite JSON numbers and integers over
4300 digits are refused in both languages.

Fixture `vcv/basic.vcv`: **constructed** (`tests/fixtures/projects/module_builders.py`)
from the Rack source above, written like Rack 2 writes (`./` names, directory
entries, an asset member, unsized Zstandard frame). Its bytes depend on the
`zstandard` build that made them, so its `file_hash` changes if it is rebuilt with
another libzstd. Not vendored: Rack's own `template.vcv` (GPL-3+); it was parsed
locally once (Rack 2.6.6, unsized frame, no checksum, 512 KiB window, five tar
entries) and the parser handled it, but it is not committed, so validation stays
`constructed_fixtures_only`.

### Renoise (.xrns) stays unsupported

Renoise publishes no schema or format specification: github.com/renoise/xrnx @
`1f35b9c` has no XSD and covers tool scripting only, so `Song.xml` is not guessed.

## Tempo-less formats

Pd, Max, VCV Rack and MOD have no tempo, and a DAWproject without `Transport/Tempo` in bpm has
none either. Their snapshots carry `transport_bpm = 0.0` (REAPER's parser falls
back to 120.0 instead). Anything that reports `transport_bpm` as a fact about the
session must treat 0.0 as "no tempo in this format". At the time of writing
`daemon/manifest_builder/generator.py` (`session_facts`) copies the value
unchanged and describes every snapshot as an Ableton `.als` parse; that needs a
follow-up outside this change.

## Untrusted-input handling (all parsers above)

- File size: `registry.MAX_PROJECT_FILE_BYTES` (64 MiB), checked before and while
  reading. Tests cover limit-1, limit, limit+1 for every fixture.
- Zip (`_safe.open_zip`): member count `MAX_ZIP_MEMBERS` (10 000), summed declared
  size `MAX_ZIP_TOTAL_BYTES` (256 MiB), duplicate names refused, names refused when
  absolute, drive-lettered, containing NUL or a `..` segment (backslashes are
  treated as separators). Members are read through a capped stream and never
  written to disk. Only `project.xml` and named `State@path` members are read.
- zlib (LMMS): bounded `decompressobj`, declared length capped and required to
  match.
- XML (`_safe.parse_xml`): expat handlers refuse DOCTYPE and entity/notation/
  external declarations structurally (so `<!DOCTYPE` inside CDATA is fine and
  encodings cannot hide a DTD); the one exception is the bodyless
  `<!DOCTYPE lmms-project>`. Depth `MAX_XML_DEPTH` (128), elements
  `MAX_XML_ELEMENTS` (2 M), size `MAX_XML_BYTES`. No `eval`, no XSD processing.
- JSON: depth `MAX_JSON_DEPTH`, node count `MAX_JSON_NODES`, `RecursionError`
  becomes `ValueError`.
- Pd: statements, canvases and canvas nesting are capped.
- XM/MOD: every count is bounded by the format maxima (tested at limit-1, limit,
  limit+1); every offset is bounds-checked.
- VCV: see the container rules above. Dependencies: Python `zstandard==0.25.0`
  (libzstd 1.5.7, BSD-3-Clause; output bounded by `max_output_size` and a
  pre-check of the declared content size); Rust `zstd` 0.14 (libzstd 1.5.7 through `zstd-sys`,
  BSD-3-Clause, no default features, built with `cc` as `ring` already is) under a
  `take(cap + 1)` reader. Both languages decode with libzstd, so the same corrupt
  stream is accepted or refused by both: the corpus flips every byte of the fixture
  and compares the two verdicts exactly.

## Golden outputs

`tests/fixtures/projects/<format>/<fixture>.golden.json` is the expected
snapshot for each fixture: sorted keys, sorted sets, no timestamps or paths,
`file_hash` of the committed bytes. The Rust port should parse the same fixture
and compare. Regenerate with
`./.venv/bin/python tests/fixtures/projects/build_fixtures.py`; that script also
rebuilds `basic.dawproject` and `basic.mmpz` reproducibly.

Digests inside snapshots (clip and device-chain fingerprints) are
`sha256` of a canonical JSON of the parsed structure (see `_snapshot.digest_of`),
so a port must reproduce that canonicalisation to match them.

## Registered as unsupported (explicit `project_format_unsupported` event)

| format_id | extensions | reason |
|---|---|---|
| `logic_pro` | `.logicx`, `.logic` | proprietary package |
| `cubase` | `.cpr`, `.npr` | proprietary; exports `.dawproject`, which is parsed |
| `fl_studio` | `.flp` | proprietary |
| `pro_tools` | `.ptx`, `.ptf` | proprietary |
| `bitwig` | `.bwproject` | layout unverified; exports `.dawproject` |
| `studio_one` | `.song` | layout unverified; exports `.dawproject` |
| `cakewalk` | `.cwp` | proprietary |
| `garageband` | `.band` | proprietary package |
| `reason` | `.reason` | proprietary |
| `sibelius` | `.sib` | proprietary |
| `dorico` | `.dorico` | zip, internal XML unspecified |
| `finale` | `.musx`, `.mus` | proprietary; MusicXML is the route, not parsed |
| `vegas` | `.veg` | proprietary |
| `acid` | `.acd` | proprietary |
| `premiere` | `.prproj` | compressed XML, unspecified |
| `audition` | `.sesx` | XML, no published schema |
| `resolve` | `.drp` | proprietary |
| `audiomulch` | `.amh` | XML the developer calls undocumented |
| `reaktor` | `.ens`, `.rkplr` | proprietary |
| `renoise` | `.xrns` | zip of `Song.xml`; Renoise publishes no schema (xrnx repository has no XSD), so nothing grounds extraction |
| `audacity` | `.aup3` | SQLite, schema unpublished |
| `tracktion_waveform` | `.tracktionedit` | believed JUCE ValueTree XML, layout not verified at primary level |

`.mod` is also used by unrelated tools; the entry only matters where a project
path is explicitly watched.

## Rust port

`rust/apw-daemon/src/project/` reads the same formats with the same limits (`safe::Limits` holds every cap so tests can lower one). `apw daemon --project <file>` starts the saved-project watcher: it writes `project_diff` events, marks the `project_differ` layer active, and puts `session_facts` and the project sample references into the manifest, exactly as the Python daemon does.

Verification, all against the Python oracle:

- `rust/apw-daemon/tests/project_parity.rs` compares every `tests/fixtures/projects/**/*.golden.json` byte for byte on the canonical JSON, and also the registry (including every unsupported reason), the `project_format_unsupported` events, the differ, `session_facts`, and 88 XML documents (accepted or refused, canonical form, and the `ET.tostring` serialisation the `.als` hashes depend on).
- `rust/apw-daemon/tests/project_hardening.rs` covers limit-1, limit and limit+1 for every cap, traversal names, duplicate zip members, decompression bombs, and DTD/entity refusal. These inputs are constructed.
- `rust/apw-daemon/tests/project_modules_parity.rs` runs `tests/fixtures/parity/project_modules_corpus.json`
  (about 6 700 inputs: fixtures, byte pokes, truncation at every offset, tar and Zstandard
  oddities, limit-1/limit/limit+1 under lowered caps) and requires the Python verdict for each
  (snapshot digest or exact error text; `malformed JSON:` diagnostics compare by class).
  Regenerate with `tests/fixtures/parity/generate_project_fixtures.py`.
- The manifest key-path parity test runs a session with `--project tests/fixtures/projects/lmms/basic.mmp`.

The `reaper` and `als` golden fixtures were added for this port: `reaper/*.rpp` are copies of `tests/fixtures/reaper/`, and `als/*.als` are constructed Live sets (`tests/fixtures/parity/generate_project_fixtures.py`), not files saved by Live.

Known differences from the Python readers, none reachable from the committed fixtures:

- XML namespaces and declared encodings other than UTF-8 are unsupported in the `.als` reader (Python's ElementTree honours both); such a file is refused. The `.als` reader also caps depth at 1024, where Python has no cap.
- Number parsing understands ASCII digits only; Python's `float()` and `int()` also accept other Unicode decimal digits.
- A ZIP whose central directory cannot be walked (ZIP64) is validated by the zip reader alone, so a repeated member name there is not detected.
