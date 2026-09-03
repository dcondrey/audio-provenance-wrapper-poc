# Product Requirements

## Project
Audio Provenance

## Objective
Build a macOS + Ableton Live proof of concept for opt-in audio provenance capture.

A producer routes at least one stem through a capture plugin. The system observes what passes through the plugin, hashes the audio, timestamps observable events, and produces a JSON provenance manifest linked to the final exported WAV or AIFF.

## Core Product Principle
Never claim full Ableton provenance.

The system must report exactly what was:
- routed
- observed
- hashed
- declared
- verified
- bypassed
- missed

## Initial Demo Workflow
1. Open Ableton Live.
2. Add the capture plugin to one track or stem.
3. Route audio, MIDI, or a plugin through the capture plugin.
4. Export a WAV or AIFF.
5. Automatically generate a JSON manifest.
6. Inspect the manifest and see observed hashes and proof levels.

## Supported Source Categories
- audio interface recording
- MIDI driving a VST synth
- imported samples or stems
- generators
- resampling
- manual imports

## Initial Scope
The first milestone supports one observed stem only.

## Out of Scope
- full Ableton session provenance
- cloud synchronization
- plugin reverse engineering
- automatic sample license verification
- hidden plugin state extraction
- authentication systems
- blockchain integrations

## Success Criteria
The user can successfully export audio and receive a valid JSON manifest containing:
- stem identifier
- timestamps
- observed hashes
- source category
- proof levels
- final export hash
- unknown or unobserved fields where appropriate
