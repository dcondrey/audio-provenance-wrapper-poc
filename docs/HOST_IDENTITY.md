# Host identity for hosts the wrapper does not recognise

## Mechanism

The plug-in reports `host_recognised`, `host_name` (from `juce::PluginHostType`), `host_executable_name` and `wrapper_format` in the `host_environment` event. JUCE does not recognise every host (for example Audacity, LMMS, Zrythm, VCV Rack, Pure Data, MilkyTracker). For those, `data/host_executables.json` maps a host executable file name to a host.

Rules, enforced by `daemon/host_identity/__init__.py`:

- A wrapper-recognised host is never overridden. `identify_host(..., jucehost_recognised=True, jucehost_name=...)` returns it with identification `juce_plugin_host_type` and proof level `directly_observed`.
- Otherwise the executable name is compared for exact, case-insensitive equality with a table entry for the given platform. One trailing `.exe` on the observed name is ignored. `.app` folders, paths, prefixes and substrings never match. On macOS the name is the `CFBundleExecutable` file, not the bundle folder.
- A hit returns `host_name` = the display name, identification `inferred_from_executable_name`, proof level `inferred`, plus `host_id`, `display_name` and `source_url`. It is never `directly_observed`: any program can be named `lmms`.
- No hit, an empty or non-string name, an unknown platform, or a name that maps to different hosts on different platforms when no platform is given: `unrecognised`, proof level `unknown_unobserved`.

The loader (`load_table` / `parse_table`) rejects, with `HostTableError`: files over 256 KiB, non-UTF-8 or non-JSON, wrong `schema_version` (currently 1), wrong types, non-https `source_url`, unknown platforms (`windows`, `macos`, `linux`), names with a path separator, `.exe` or `.app`, duplicate `host_id`, and the same (platform, name) claimed twice. Bounds: 512 hosts, 32 matches per host, 512 characters per string.

## Table provenance

Each mapped host cites the primary file that fixes its executable name (`source_url` per host; its `note` fields give the detail). All are the projects' own build or packaging files.

| Host | Names | Source |
|---|---|---|
| Audacity | 3.x `Audacity` (Windows, macOS), `audacity` (Linux); 4.x `Audacity4` (Windows, from `MUSE_APP_NAME` + major version), `audacity` target elsewhere | audacity `CMakeLists.txt` (Audacity-3.7.9), `src/app/CMakeLists.txt` and `version.cmake` (master) |
| LMMS | `lmms` | `src/CMakeLists.txt` `ADD_EXECUTABLE(lmms`; macOS plist uses the CPack project name `lmms` |
| VCV Rack | `Rack` / `Rack.exe` | `Makefile` `STANDALONE_TARGET` |
| MilkyTracker | `MilkyTracker` (Windows, macOS), `milkytracker` (Linux) | `src/tracker/CMakeLists.txt` `OUTPUT_NAME` |
| Pure Data | `pd` (core process; `pd.exe` on Windows) | `src/Makefile.am` `bin_PROGRAMS`, `msw/pd.nsi` |
| Zrythm | `zrythm` | `src/gui/CMakeLists.txt` `qt_add_executable(zrythm` |

Unmapped, not guessed: Finale, ACID Pro, Vegas Pro, Dorico, Sibelius, Max, Reaktor, AudioMulch (closed source, no primary source confirming the process name), Sonic Pi (not a plug-in host; its engine is scsynth). They are listed under `unmapped` in the JSON with reasons. Add a host only with a primary-source URL.

Residual risk: the names are build-time names. Packagers can rename binaries (for example a distribution suffix), which then simply fails to match. A different program with an identical name would be mislabelled, which is why the result is `inferred`.

## Integration points (not wired here)

Python: `daemon/manifest_builder/generator.py`, `derive_host_environment`. In the `host_unrecognised` branch (and only there), call

```python
from daemon.host_identity import identify_host
ident = identify_host(observed.get("host_executable_name"), recognised,
                      observed.get("host_name"), platform=<windows|macos|linux>)
```

If `ident.identification == "inferred_from_executable_name"`, emit `host_name = ident.host_name`, `host_recognised` stays as observed (False) or a new `host_identified` flag is added, `status` a distinct value such as `host_inferred`, `apw:proof_level = ident.proof_level` (`inferred`), and add `identification`, `host_id`, `source_url` and a basis stating the name was matched against the table. Keep the recognised branch as is. Derive `platform` from `sys.platform` (`win32` -> windows, `darwin` -> macos, `linux*` -> linux) at the daemon, or from the event if the plug-in adds it. `schema.py` `_require_proof` accepts `inferred` already for other blocks; confirm it for `host_environment`.

Rust: the session code that records `host_environment` and builds the equivalent grade should read the same `data/host_executables.json` (embed with `include_str!` or load at the same path), apply the identical rules (casefold, strip one trailing `.exe`, exact match per platform, platform from `cfg!(target_os)`, JUCE precedence, ambiguity gives unmatched), enforce the same size cap and validation, and produce the same field values as the Python function. A shared conformance test should run both against the table's names and the near-miss cases in `tests/test_host_identity.py`.
