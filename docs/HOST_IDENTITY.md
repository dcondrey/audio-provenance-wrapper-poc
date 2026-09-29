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

## Integration

Wired in both daemons, reading the same table.

Python: `daemon/manifest_builder/generator.py`, `derive_host_environment(daemon, platform=None)`. In the `host_unrecognised` branch only, it calls `identify_host(executable, False, name, platform=...)`, the platform coming from `sys.platform` (`win32` windows, `darwin` macos, `linux*` linux). A table hit emits `status: host_inferred`, `host_recognised: false`, `host_name` = the display name, `identification`, `host_id`, `source_url`, a basis saying the name was matched against the table, and `apw:proof_level: inferred`. The recognised branch, the conflict branch and the unobserved branch are unchanged. An unusable table logs a warning and leaves the host unidentified.

Rust: `rust/apw-daemon/src/host_identity.rs`, called from `derive_host_environment` in `assembly.rs` with `cfg!(target_os)`. The table is embedded from `rust/apw-daemon/data/host_executables.json`; `tests/host_identity_parity.rs` asserts that copy is byte-identical to `data/host_executables.json`, so update both together. Casefolding follows Python's `str.casefold` for every character that can fold to ASCII (sharp s, long s, f-ligatures; the Kelvin sign lowercases to `k`), and `strip` follows Python's `isspace` (which includes U+001C to U+001F).

Schema: `daemon/schema.py` (`_require_proof` already accepted `inferred`, but the generic else-branch demanded `unknown_unobserved` and a null name). `host_inferred` now has its own branch: `inferred`, a non-empty `host_name`, `host_recognised: false`, `identification: inferred_from_executable_name`, and non-empty `host_id` and `source_url`. `rust/apw-core/src/schema.rs` mirrors it, with cases in `rust/apw-core/tests/fixtures/schema_cases.json`.

Conformance: `tests/fixtures/parity/host_identity.json` (table validation, every table name and near-miss on every platform, ambiguity) and `session_state.json` are generated from the Python oracle and checked by `tests/test_host_identity.py`, `tests/test_parity_fixtures.py` and the Rust tests. The manifest key-path test runs a session whose host executable is `lmms`, exercising the `host_inferred` branch end to end.
