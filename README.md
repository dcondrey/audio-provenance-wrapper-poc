<!-- repo-header:start -->
<h1>Audio Provenance Capture</h1>

<p><strong>Proof of concept for audio provenance capture in Ableton Live using a wrapper/capture plugin, local daemon, audio hashing, and JSON manifests for stem-to-export traceability.</strong></p>

[![Best Practices Evidence](https://img.shields.io/badge/best%20practices-evidence%20reviewed-6a4c93?style=flat-square&labelColor=20232a)](.bestpractices.json)
<!-- repo-header:end -->

One monorepo for creation-stage capture, signed provenance records, resilient audio recovery, and
verification.

## Repository layout

| Path | Role |
| --- | --- |
| `src/`, `daemon/`, `scripts/`, `packaging/` | Ableton VST3/AU capture workflow, Python daemon, demo, and macOS distribution |
| `rust/` | Rust implementation of the capture daemon, C2PA, association, provider, CLI, and FFI surfaces |
| `sdk/` | Rust/TypeScript verification SDK, CLI, registry, trust, watermark, benches, training, and capture rig |
| `docs/` | Product scope, architecture, schema, validation, provider contract, and integration boundary |

The capture side records what happened during creation. The SDK side signs, locates, registers,
and verifies the resulting audio and records. They share the same proof-level discipline and the
`audio-provenance-manifest-v0` interoperability contract; the implemented development adapter and
its deliberately deferred production infrastructure are documented in
[SDK integration](docs/SDK_INTEGRATION.md).

## Validation entry points

```sh
# Capture workflow
./.venv/bin/pytest -q
cargo test --manifest-path rust/Cargo.toml --workspace

# Plug-in real-time and pass-through gate
cmake -S . -B build-rt-audit -DBUILD_TESTING=ON
cmake --build build-rt-audit --target AudioProvenanceCaptureTests AudioProvenanceCapture_VST3
ctest --test-dir build-rt-audit --output-on-failure

# SDK and verification engine
cargo test --manifest-path sdk/Cargo.toml --workspace
pnpm --dir sdk --filter @writerslogic/audio-provenance-sdk test
sdk/scripts/watermark-adversarial-fast.sh

# One-command development capture-to-SDK round trip
./.venv/bin/python scripts/synthetic_rehearsal.py --sdk-adapter
```

## Capture application

## Current State: v0.9 Demo Candidate, v1.0 Gate Closed

The automated one-stem path is implemented and tested. The manual Ableton Live
gate on the demonstration machine closed on 2026-08-29. Its graded pass
(session `capture-20260830T021208Z-11629`) ran the one-stem path end to end
with `complete_observed_path` coverage, `inferred_match` association, and a
`verified` local verifier outcome; the transparency/null test and the project
save/close/reload were performed live in the same sitting. Per-step evidence is
in `docs/VALIDATION.md` and `docs/ROADMAP.md` records the closed gate. No
automated or synthetic result was used to close it.

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
4. Export **16-bit PCM WAV** into the timestamped folder opened by the launcher.
   A 32-bit float render cannot be signed and grades the association
   `unavailable`.
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
- Each supported export is signed into a real C2PA claim: embedded for 16-bit
  PCM WAV, a `.c2pa` sidecar for AIFF. The signed copy lives under
  `manifests/artifacts/`; the export itself is never rewritten.
- That claim is signed under a certificate chain this machine issued to itself.
  A `verified` validation state means it chains to our own root, not to any
  external trust list, registry, or verified creator identity.
- `c2pa_mapping` remains in the manifest as a descriptive projection of internal
  evidence, not as the signed claim.
- The downstream record is a neutral provenance registration handoff. The explicit adapter in
  `docs/SDK_INTEGRATION.md` admits it, preserves proof levels, signs/attaches a development record,
  and verifies it without promoting identity beyond `not_established`.

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
- `docs/SDK_INTEGRATION.md` — complementary integration boundary
- `docs/FOUNDER_DEMO_TALK_TRACK.md` — five-minute private demo and recovery
- `docs/EXECUTIVE_PRODUCT_BRIEF.md` — wedge, pilot, risks, and 30/60/90 path
- `docs/manifest.schema.json` — machine-readable JSON Schema
- `docs/MULTI_LAYER_OBSERVATION.md` — longer-term research architecture
