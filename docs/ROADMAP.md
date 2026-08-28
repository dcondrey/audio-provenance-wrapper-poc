# Roadmap

## Project Goal

Build a macOS + Ableton Live proof of concept for opt-in audio provenance capture.

The first successful demo is not universal DAW provenance. It is a one-stem workflow where a producer routes audio through a capture plugin, exports a WAV or AIFF, and receives a JSON manifest that honestly reports what was observed, hashed, declared, verified, bypassed, or missed.

## Core Principle

Never claim full Ableton provenance.

The system must distinguish between:

- directly observed data
- inferred data
- user-declared data
- externally verified data
- unknown or unobserved data

## Current Status: v0.9 Demo Candidate

The one-stem automated path now builds a complete JSON manifest and HTML fight
card from simulated UDP evidence and a detected export. New and overwritten
exports are covered by integration tests, and the verifier checks the export,
evidence-prefix bindings, chain commitment, and local integrity seal.

Remaining v1.0 gate: build/install the current VST3 and complete the manual
Ableton load, pass-through, playback/reload, and real export validation in
`docs/VALIDATION.md`.

## Agile Epics

### Epic 1: Capture Plugin

Build the JUCE-based VST3 capture plugin.

Goal:
- Load inside Ableton Live.
- Pass audio through unchanged.
- Observe audio buffers and MIDI where available.
- Timestamp events.
- Compute hashes or fingerprints.

### Epic 2: Plugin-to-Daemon Communication

Stream structured events from the plugin to a local macOS daemon.

Goal:
- Use local UDP for v0.
- Send non-blocking event messages.
- Persist received events.

### Epic 3: Export Detection and Hashing

Detect final exported audio files.

Goal:
- Watch configured export folders.
- Detect WAV or AIFF exports.
- Hash final exported files.
- Associate export files with session data.

### Epic 4: Manifest Generation

Generate the JSON fight-card manifest.

Goal:
- Include observed stem hashes.
- Include final export hash.
- Include source category.
- Include timestamps.
- Include proof levels for every claim.

### Epic 5: Workflow Validation and Failure Testing

Validate the system honestly.

Goal:
- Test one-stem workflow.
- Test bypassed audio.
- Test unknown source cases.
- Test manual imports.
- Expand to five stems only after one-stem demo works.

## Sprint Plan

### Sprint 0: Foundation

Deliverables:
- `AGENTS.md`
- `requirements/`
- `docs/ROADMAP.md`
- `docs/ARCHITECTURE.md`
- `docs/MANIFEST_SCHEMA.md`
- Cursor and Codex prompt docs

Exit criteria:
- Project scope is clear.
- Non-goals are documented.
- Manifest trust model is defined.
- Future AI agents know how to contribute safely.

### Sprint 1: Pass-Through Plugin

Deliverables:
- JUCE project scaffold.
- VST3 plugin loads in Ableton.
- Audio passes through unchanged.

Exit criteria:
- Plugin can be inserted on an Ableton track.
- Playback works without audible changes.
- No crashes during basic playback.

### Sprint 2: Local Event Logging

Deliverables:
- Session ID generation.
- Stem ID generation.
- Audio buffer hash events.
- MIDI event logging where available.
- Local debug logging.

Exit criteria:
- Plugin produces timestamped observed events.
- Events include proof-level metadata.
- Events can be inspected locally.

### Sprint 3: UDP Event Streaming

Deliverables:
- UDP sender inside plugin.
- UDP receiver daemon.
- Local event persistence.

Exit criteria:
- Daemon receives plugin events.
- Events are stored locally.
- Plugin remains real-time safe.

### Sprint 4: Export Detection and Hashing

Deliverables:
- Export folder watcher.
- WAV and AIFF detection.
- Final file hash computation.

Exit criteria:
- Daemon detects new exported audio files.
- Daemon computes final file hash.
- Export is linked to active session where possible.

### Sprint 5: JSON Manifest

Deliverables:
- Manifest builder.
- Initial JSON schema.
- Sample manifest output.

Exit criteria:
- Exporting a file produces a JSON manifest.
- Manifest includes observed stem evidence and final export hash.
- Unknown and unobserved fields are represented honestly.

### Sprint 6: Multi-Stem and Bypass Tests

Deliverables:
- Five-stem test session.
- Bypass test cases.
- Failure mode documentation.

Exit criteria:
- Five routed stems appear in the manifest.
- Bypassed or unobserved sources are not overclaimed.
- Known limitations are documented.

### Sprint 7: C2PA Alignment

Deliverables:
- Map JSON fight-card fields to C2PA concepts.
- Explore sidecar or embedded manifest options.
- Document C2PA integration path.

Exit criteria:
- Clear plan for moving from JSON manifest to C2PA-compatible output.
- No change to honesty model.

## Version Milestones

### v0.1
Pass-through plugin loads and plays audio unchanged.

### v0.2
Plugin logs local observed events.

### v0.3
Plugin streams events to daemon over UDP.

### v0.4
Daemon detects exported WAV or AIFF and hashes it.

### v0.5
Daemon produces first JSON manifest.

### v0.9
One-stem demo candidate: automated pipeline, fight card, verifier, and build/run
scripts are ready; manual Ableton validation remains.

### v1.0
One-stem end-to-end Ableton demo works reliably.

### v1.1
Five-stem workflow works with bypass/failure tests.

### v2.0
C2PA sidecar or signing alignment begins.

## Definition of Done for v1.0

A producer can:

1. Open Ableton Live.
2. Insert the capture plugin on one track.
3. Route a mic recording, synth output, imported sample, generated audio, or resampled audio through the plugin.
4. Export a WAV or AIFF.
5. Find a generated JSON manifest.
6. Verify that the manifest contains:
   - observed stem hash
   - final export hash
   - timestamps
   - source category
   - proof levels
   - unknown or unobserved fields where appropriate
