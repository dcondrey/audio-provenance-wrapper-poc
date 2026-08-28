# Audio Provenance Capture

A macOS + Ableton Live proof of concept for opt-in, routed-audio provenance.
The system observes audio that passes through a JUCE VST3, streams evidence to
a local daemon, hashes a detected WAV/AIFF export, and produces both a JSON
manifest and a human-readable HTML fight card.

## Current State: v0.9 Demo Candidate

The automated one-stem path is implemented and tested. Final v1.0 status still
requires a clean manual validation pass inside Ableton Live on the demonstration
machine.

### VST3 capture plugin

- Mono/stereo pass-through audio
- Complete rolling SHA-256 observation chain (4096-sample windows)
- RMS, zero-crossing, spectral-centroid, and three-band measurements
- Silence/audio transitions and spectral-profile changes
- Host transport and routed MIDI observations
- Non-blocking FIFO from the audio callback to a background observer
- Local UDP event emission to `127.0.0.1:9876`
- UI counters that show routed audio, hash windows, and emitted events

### Local daemon and output

- Validates and persists plugin events as JSONL
- Assigns a scoped local capture-session ID and one-stem ID
- Watches optional sample and Ableton `.als` files
- Detects new **or overwritten** WAV/AIFF exports
- Hashes stable export files and extracts basic audio metadata
- Generates a proof-labelled JSON manifest
- Generates a polished, dependency-free HTML fight card
- Binds the manifest to immutable prefixes of its evidence files
- Applies a local software HMAC integrity seal
- Verifies export hashes, evidence bindings, chain commitments, and manifest integrity

## Five-minute demonstration

Build and install the plugin. If JUCE is not already installed, CMake fetches
the pinned JUCE `8.0.15` release automatically.

```sh
./scripts/build_plugin.sh --install
```

Start a clean demo workspace. The second argument is the producer-declared
source category; use `unknown` when it is not known.

```sh
./scripts/run_demo.sh ./demo-output imported_sample
```

Then:

1. Open Ableton Live and rescan VST3 plug-ins.
2. Insert **Audio Provenance Capture** on one audio track.
3. Play the track until the plugin shows `Capture status: ACTIVE` and increasing hash windows.
4. Export a WAV or AIFF into `demo-output/exports/`.
5. Open the generated `*_provenance.html` fight card in `demo-output/manifests/`.
6. Verify the adjacent JSON manifest:

```sh
./scripts/verify_demo.sh ./demo-output/manifests/your_export_manifest.json
```

For a richer demonstration, pass the saved Ableton set as the third argument:

```sh
./scripts/run_demo.sh ./demo-output imported_sample /path/to/Demo.als
```

The `.als` parser is an experimental, unsupported interpretation of saved
project structure. Its resulting session facts are labelled `inferred`.

## Manual daemon command

```sh
python3 -m daemon \
  --port 9876 \
  --sample-dir ~/Music/ProvenanceSamples \
  --export-dir ~/Music/Exports \
  --project ~/Music/MyProject/MyProject.als \
  --manifest-dir manifests \
  --source-category imported_sample
```

Supported source declarations are:

- `unknown`
- `audio_interface_recording`
- `midi_vst_synth`
- `imported_sample`
- `generator`
- `resampling`
- `manual_import`

Non-unknown source categories are recorded as `user_declared`, not verified.

## Trust boundary

This project never claims full Ableton provenance.

- Routed buffers and export-file hashes are `directly_observed`.
- The association between routed observations and an export is `inferred`
  because both appeared during the same local capture session.
- A producer-selected source category is `user_declared`.
- Hidden plug-in state, bypassed routing, upstream rights, and unobserved audio
  remain `unknown_unobserved`.
- The HMAC seal is local integrity protection. It is not Secure Enclave
  attestation, third-party identity, or a production C2PA signature.
- The included C2PA structures are an alignment/mapping prototype, not an
  embedded, conforming C2PA manifest.

## Tests

```sh
python3 -m unittest discover -s tests -v
```

The current suite contains 89 tests, including UDP-to-export integration,
overwritten-export detection, report rendering, manifest verification, and
proof-level behavior.

## Documentation

- `docs/PROJECT_BRIEF.md` — product and demonstration scope
- `docs/DEMO_RUNBOOK.md` — presenter checklist and talk track
- `docs/ARCHITECTURE.md` — components and trust boundary
- `docs/MANIFEST_SCHEMA.md` — evidence and proof-level model
- `docs/ROADMAP.md` — milestones and remaining v1.0 validation
- `docs/VALIDATION.md` — build and Ableton validation record
- `docs/MULTI_LAYER_OBSERVATION.md` — longer-term research architecture
