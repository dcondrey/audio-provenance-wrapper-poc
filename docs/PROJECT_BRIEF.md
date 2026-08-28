# Project Brief

## Objective

Demonstrate opt-in audio provenance capture for one stem in Ableton Live.
A producer inserts a pass-through VST3 on a track, routes audio through it,
exports a WAV or AIFF, and receives a proof-labelled manifest that binds local
routed-audio observations to the exported file hash.

## Demonstration promise

The demonstration proves that:

- audio buffers passed through the capture plugin;
- the plugin produced a rolling observation-hash chain;
- the local daemon received and persisted those events;
- the daemon returned scoped local operational receipts for accepted events;
- a stable WAV/AIFF export appeared in the watched folder;
- the daemon computed the export SHA-256;
- a manifest and derived fight card were generated for the same local capture session.

The stem-to-export association is `inferred`. The system does not prove that
all Ableton routing passed through the plugin or that the export contains only
the observed stem.

A daemon receipt is not identity proof, remote attestation, DAW trust, or a
registry result. The downloadable bundle is governed by a self-generated demo
key whose valid signature proves integrity and key possession, not identity.

The hardened demo positions the bundle as creation-stage input to a downstream
registration flow. Identity, author-controlled credentials, production
certificates, audio-native soft binding, resilient recovery, registry
publication, production C2PA generation, consent, and rights verification all
remain downstream requirements.

## Audience

- music-technology founders evaluating provenance workflows;
- producers who want opt-in evidence without changing creative tools;
- engineers evaluating a path toward stronger signing and C2PA integration.

## v1.0 boundary

v1.0 is a reliable one-stem macOS demonstration, not a universal DAW capture
product. It requires:

1. a clean VST3 build and install;
2. successful Ableton discovery and insertion;
3. transparent audio pass-through;
4. visible routed-audio observations;
5. automatic export detection;
6. JSON manifest and HTML fight-card generation;
7. successful local verification;
8. explicit unknown/unobserved limitations.

## Deferred work

- production C2PA manifests and certificate-based signing;
- Secure Enclave or other hardware attestation;
- independently verifiable identity;
- multiple stems and bypass coverage;
- supported Ableton semantic APIs;
- sample licensing or ownership verification;
- universal DAW support.
