# Genotone Alignment

## Executive position

This POC is an upstream evidence adapter for an existing producer workflow. It
observes one audio path deliberately routed through an Ableton VST3, binds those
observations to a local export with a conservative routed-feature comparison,
packages a signed local evidence index/bundle, and prepares a neutral downstream
registration handoff.

It does not reproduce or claim compatibility with Genotone technology. It gives
a downstream trust provider more creation-stage context to evaluate when the
master reaches registration.

```text
Ableton creation session
  → routed-audio observations and coverage counters
  → exported master + evidence bundle
  → neutral downstream provenance registration handoff
  → downstream identity, credential policy, soft binding, signing,
    resilient recovery, registry publication, and open verification
```

## Why the boundary fits

Genotone’s official public material describes registration at the master stage:
an inaudible mark, an audio fingerprint, and a signed record bound to a verified
name, followed by checking against a registry. Its Proof of Human initiative
also frames the problem as infrastructure rather than AI detection, with
creator-controlled identity and verification at ecosystem entry points.

The adapter operates earlier. It can supply evidence that a specific plug-in
instance observed a sequence of routed buffers, whether the local daemon returned
scoped operational receipts and whether that sequence had known gaps, the hard
hash of the resulting export, and how strongly compact audio features align.
This can make a master-stage registration record more
useful without substituting creation telemetry for identity or authorship.

Official context reviewed 28 August 2026:

- <https://genotone.com/>
- <https://andymelchior.com/proof-of-human>

## Integration architecture

| Stage | This adapter establishes | Downstream trust layer establishes |
|---|---|---|
| Creation | Routed buffers observed; local emitted/received/ACK-issued states; chain and sequence continuity; declared source category | Nothing implied about identity, authorship, ownership, consent, or rights |
| Export | Local hard hash and audio metadata directly observed | Canonical master policy and registration acceptance |
| Association | Relative RMS/ZCR/crest/envelope offset match, always inferred | Production-grade fingerprint and/or audio-native soft binding |
| Integrity | Ed25519 integrity check under a self-generated demo key | Author-controlled credential policy, verified identity, certificate chain |
| Portability | Deterministic ZIP, self-generated-key signed index, JSON manifest, and tentative C2PA assertion mapping | Production C2PA claim generation, resilient recovery, registry publication |
| Verification | Local outcomes: verified, changed, untrusted, not_found | Open downstream/registry result under its published trust policy |

## What must not be conflated

- A valid self-generated signature is key-possession and integrity evidence, not
  externally verified identity.
- Feature alignment is an inferred local association, not a watermark,
  fingerprint-registry lookup, or proof that bypass was impossible.
- A local daemon ACK is operational receipt evidence, not Genotone verification,
  registry confirmation, identity, or remote attestation.
- A producer declaration is not authorship, ownership, consent, or rights
  verification.
- A missing observation or failed match is not evidence that audio is synthetic
  or absent.
- The JSON C2PA mapping is tentative; it is not a conforming production claim.
- The handoff is not a Genotone API payload and does not invent GenoMark,
  GenoTrace, registry, identity, certificate, signing, or SDK compatibility.

## Credible pilot seam

A narrow pilot could place the adapter in one studio workflow and give a
mastering engineer a bundle alongside the delivered pre-master. The downstream
registration team could decide which fields to accept, ignore, or challenge and
record that policy explicitly. Success would be measured by fewer provenance
questions at registration and better dispute evidence, not by claiming complete
DAW capture.
