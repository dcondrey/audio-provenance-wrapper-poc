# AGENTS.md

## Project
`audio-provenance` is the unified monorepo for opt-in audio provenance capture and verification.

- The repository root contains the macOS + Ableton Live capture application.
- `rust/` contains the Rust capture-engine implementation.
- `sdk/` contains the Rust/TypeScript signing, registry, recovery, trust, watermark, and verification SDK.

## Core Principle
Never claim full Ableton provenance. Only report what was routed, observed, hashed, declared, verified, bypassed, or missed.

## Target v0
A producer can:
1. Open Ableton Live.
2. Route one stem through a pass-through capture plugin.
3. Export a WAV or AIFF file.
4. See a JSON manifest linking observed stem data to the exported file hash.

## Tech Stack
- JUCE for plugin development.
- VST3 first; AU later.
- Local UDP for plugin-to-daemon events.
- macOS daemon/process for event collection and export watching.
- JSON fight-card manifest first.
- Real C2PA signing via c2pa-rs: embedded for 16-bit PCM WAV, `.c2pa` sidecar
  for AIFF. Signed under a self-issued chain, so it is key possession and hard
  binding, never verified identity.

## Milestones
- v0.1: pass-through plugin.
- v0.2: local event logging.
- v0.3: UDP event stream to daemon.
- v0.4: daemon stores session state.
- v0.5: daemon detects final export and hashes the file.
- v1.0 capture gate: closed by the manual Ableton validation recorded in
  `docs/VALIDATION.md`; automated and synthetic results did not close it.
- SDK alpha: implemented under `sdk/`; its robustness, null-test, trust-format,
  and security gates remain separate from the closed capture gate.

## Proof Levels
Every manifest field must include one of:
- `directly_observed`
- `inferred`
- `user_declared`
- `externally_verified`
- `unknown_unobserved`

## Constraints
- Do not build universal DAW capture for v0.
- Do not assume hidden plugin internals are observable.
- Do not claim sample license, device identity, preset identity, or upstream provenance unless externally verified.
- Prefer a working one-stem demo over a broad incomplete system.

## Instructions for AI Agents
Before editing, read:
1. `README.md`
2. `docs/PROJECT_BRIEF.md`
3. `docs/ROADMAP.md`
4. `docs/ARCHITECTURE.md`
5. `docs/MANIFEST_SCHEMA.md`
6. `docs/SDK_INTEGRATION.md`
7. For SDK work, `sdk/README.md` and `sdk/STATUS.md`

When making changes:
- Keep edits small and milestone-scoped.
- Update documentation when architecture or schema changes.
- Do not overclaim provenance guarantees.
- Prefer explicit TODOs over pretending a hard problem is solved.
