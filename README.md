<!-- repo-header:start -->
<img src="https://github.com/dcondrey.png?size=160" alt="Audio Provenance Capture logo" width="120" align="left">

<h1>Audio Provenance Capture</h1>

<p><strong>Proof of concept for audio provenance capture in Ableton Live using a wrapper/capture plugin, local daemon, audio hashing, and JSON manifests for stem-to-export traceability.</strong></p>

<br clear="left">

[![Best Practices Evidence](https://img.shields.io/badge/best%20practices-evidence%20reviewed-6a4c93?style=flat-square&labelColor=20232a)](.bestpractices.json)
<!-- repo-header:end -->

## Current State: v0.9 Evidence-Adapter Demo Candidate

The automated one-stem path is implemented and tested. A manual Ableton Live
pass on the demonstration machine (2026-08-28, session
`capture-20260828T221446Z-9959`) completed the one-stem path end to end with
`complete_observed_path` coverage and `inferred_match` association. Final v1.0
status still requires the transparency/null test and project save/close/reload
recorded as open in `docs/VALIDATION.md`.

### VST3 capture plugin

- Mono/stereo pass-through audio
- Complete rolling SHA-256 observation chain (4096-sample windows)
- RMS, zero-crossing, spectral-centroid, and three-band measurements
- Silence/audio transitions and spectral-profile changes
- Host transport and routed MIDI observations
- Non-blocking FIFO from the audio callback to a background observer
- Local UDP event emission to `127.0.0.1:9876`
- Local UDP daemon acknowledgements received on a dedicated non-audio thread
- UI counters that show routed audio, hash windows, and emitted events
- Explicit plug-in instance/session IDs, event sequences, FIFO-loss counters,
  and UDP attempt/failure counters
- Compact UI that distinguishes prepared events, UDP attempts, locally emitted
  datagrams, fresh/stale/rejected daemon receipt, sequence gaps, and unknown state

### Local daemon and output

- Validates and persists plugin events as JSONL
- Assigns a scoped local capture-session ID and one-stem ID
- Watches optional sample and Ableton `.als` files
- Detects new **or overwritten** WAV/AIFF exports
- Hashes stable export files and extracts basic audio metadata
- Generates a proof-labelled JSON manifest
- Generates a polished, dependency-free HTML fight card
- Uses daemon monotonic time for bounded, deduplicated cross-layer correlation
- Rotates JSONL evidence at 64 MiB with three retained backups
- Binds the manifest to streaming-hashed immutable evidence prefixes
- Compares routed/export relative RMS, zero-crossing, crest-factor, and coarse
  energy-envelope sequences with bounded time-offset search; the result always
  remains `inferred` or unknown
- Derives `complete_observed_path`, `partial_observed_path`, or
  `unknown_coverage` conservatively from counters
- Applies a local software HMAC integrity seal
- Applies a portable Ed25519 signature with signer identity kept separate
- Produces a JSON-Schema-governed downstream registration handoff
- Serves a live local dashboard and four plain local verifier outcomes
- Produces a deterministic ZIP evidence bundle plus a separately signed canonical
  index covering every other archive entry

## Five-minute demonstration

Build and install the plugin. If JUCE is not already installed, CMake fetches
the pinned JUCE `8.0.15` release automatically.

```sh
python3 -m pip install -r requirements.txt
./scripts/build_plugin.sh --install
```

Start a clean timestamped session, preflight the machine, and open the live
dashboard plus watched export folder:

```sh
./scripts/demo.sh
```

If Ableton is unavailable, run the deterministic presenter fallback. It creates
a fresh synthetic routed session, exercises daemon ACK receipt, gain/offset
alignment, sealing, public-key verification, and bundle verification, then opens
the final dashboard and fight card:

```sh
./scripts/presenter_fallback.sh
```

Optional arguments are the demo root, producer-declared source category, and
saved Ableton project path:

```sh
./scripts/demo.sh ./demo-output imported_sample /path/to/Demo.als
```

Then:

1. Open Ableton Live and rescan VST3 plug-ins.
2. Insert **Audio Provenance Capture** on one audio track.
3. Play the track until the plugin shows `Capture status: ACTIVE` and increasing hash windows.
4. Export a WAV or AIFF into the timestamped folder opened by the launcher.
5. The generated fight card opens automatically; the dashboard also links it.
6. Verify the adjacent JSON manifest:

```sh
./scripts/verify_demo.sh "$(cat ./demo-output/latest-session.txt)/manifests/your_export_manifest.json"
```

Run the full safe adversarial sequence without changing the original:

```sh
./scripts/demo_adversarial.sh /path/to/export_manifest.json
```

Assemble a self-contained, sendable evidence package from the session:

```sh
python3 scripts/package_demo.py "$(cat ./demo-output/latest-session.txt)"
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
- The association between routed observations and an export is `inferred` from
  comparable features emitted by accepted routed plug-in windows and extracted
  from the export; session co-occurrence alone does not establish it.
- A daemon ACK proves only that this local daemon accepted and persisted an event
  before dispatching a receipt. It is not DAW trust, identity proof, remote
  attestation, or registry confirmation.
- A producer-selected source category is `user_declared`.
- Hidden plug-in state, bypassed routing, upstream rights, and unobserved audio
  remain `unknown_unobserved`.
- The HMAC seal is local integrity protection. It is not Secure Enclave
  attestation, third-party identity, or a production C2PA signature.
- The Ed25519 signature is independently checkable with the public key. A
  self-generated key proves possession and integrity, not identity or external trust.
- Audio association uses simple feature sequences and remains `inferred`; a
  failed or unavailable result does not prove routed audio was absent.
- The included C2PA structures are an alignment/mapping prototype, not an
  embedded, conforming C2PA manifest.
- The downstream record is a neutral provenance registration handoff, not a
  Genotone API payload or compatibility claim.

## Tests

```sh
python3 -m unittest discover -s tests -v
```

The suite includes UDP acknowledgement state, routed-feature alignment fixtures,
deterministic bundle integrity, UDP-to-export integration,
overwritten-export detection, report rendering, manifest verification, and
focused bounded-growth, sequence-gap, and proof/coverage invariant regressions.

## Documentation

- `docs/PROJECT_BRIEF.md` — product and demonstration scope
- `docs/DEMO_RUNBOOK.md` — presenter checklist and talk track
- `docs/ARCHITECTURE.md` — components and trust boundary
- `docs/MANIFEST_SCHEMA.md` — evidence and proof-level model
- `docs/ROADMAP.md` — milestones and remaining v1.0 validation
- `docs/VALIDATION.md` — build and Ableton validation record
- `docs/GENOTONE_ALIGNMENT.md` — complementary integration boundary
- `docs/FOUNDER_DEMO_TALK_TRACK.md` — five-minute private demo and recovery
- `docs/EXECUTIVE_PRODUCT_BRIEF.md` — wedge, pilot, risks, and 30/60/90 path
- `docs/manifest.schema.json` — machine-readable JSON Schema
- `docs/MULTI_LAYER_OBSERVATION.md` — longer-term research architecture
