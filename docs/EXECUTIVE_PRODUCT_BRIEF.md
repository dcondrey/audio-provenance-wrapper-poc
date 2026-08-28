# Executive Product Brief

## Product wedge

Creation-stage evidence is usually discarded before a master reaches a trust
or registration system. A small opt-in adapter can preserve narrowly scoped,
machine-checkable facts from the tools creators already use and deliver them
with the exported master.

The wedge is a truthful evidence handoff, not universal DAW surveillance. One
well-instrumented routed path is more credible than broad claims about hidden
session state.

## Stakeholder value

| Stakeholder | Practical value |
|---|---|
| Artist / producer | Opt-in evidence without leaving the DAW; visible gaps and unknowns |
| Mastering engineer | A structured pre-master context bundle and exact export hash |
| Studio | Repeatable session hygiene and a reviewable handoff policy |
| Label / rights team | Better evidence for intake questions and later disputes, without treating declarations as verified rights |
| Platform | More structured provenance at ingest, subject to its own trust policy |
| Genotone | Potential creation-stage context that can enrich master-stage registration while leaving identity, soft binding, recovery, signing policy, and registry authority downstream |

## Pilot opportunity

Run a one-studio, one-mastering-partner pilot on 20–50 tracks. Capture only one
declared stem or pre-master path. At registration, compare the bundle with the
delivered audio and record which evidence was accepted, challenged, or ignored.

Measure:

- percentage of sessions producing an intact, gap-free observed path;
- association availability and matched coverage after normal mastering;
- time saved resolving registration questions;
- creator comfort and opt-in rate;
- frequency and cause of unknown or partial outcomes.

Do not measure “AI detected” or “human proved”; neither is an output of this
adapter.

## Build, integrate, defer

### Build now

- reliable VST3 observation and bounded event transport;
- deterministic session/export versioning and coverage accounting;
- portable bundle validation and adversarial demonstrations;
- policy-friendly neutral handoff records.

### Integrate through a defined seam

- mastering workstation handoff;
- downstream identity and credential selection;
- accepted claim vocabulary and C2PA mapping;
- registry submission and public verification result.

### Defer

- universal DAW introspection;
- claims about hidden plug-in state or universal bypass detection;
- in-house identity verification, watermarking, recovery, or registry services;
- production C2PA certificates and conformance until a downstream policy owner
  is selected;
- sample-rights or consent claims without external verification.

## Major risks

- UDP loss or overload can make coverage partial; the product must preserve that
  result rather than smooth it over.
- Mastering can weaken simple feature alignment; a failed comparison is
  inconclusive.
- Creator declarations can be mistaken for verified claims unless proof labels
  survive every UI and interchange.
- A self-generated key can create false identity confidence unless signature
  validity and signer identity remain separate.
- Workflow friction will kill adoption faster than cryptographic weakness; the
  integration must remain opt-in, visible, and recoverable.
- Premature brand/API language can imply a partnership or compatibility that
  does not exist.

## Pragmatic 30 / 60 / 90 days

### 30 days — Validate the seam

- complete manual Ableton pass-through/export validation on the pilot machine;
- rehearse failure recovery and collect 10 representative exports;
- define a downstream field-acceptance policy with a mastering partner;
- measure alignment across gain, EQ, limiting, resampling, and leading silence.

### 60 days — Pilot workflow

- package signed/notarized installer and background service;
- replace local UDP ambiguity with authenticated local acknowledgement or a
  bounded IPC transport;
- add multi-instance/stem policy without claiming project completeness;
- generate a versioned handoff accepted by the downstream pilot system.

### 90 days — Production decision

- review pilot evidence quality, creator adoption, and operational burden;
- choose production credential custody and certificate-chain ownership;
- perform C2PA conformance and threat-model review with the downstream provider;
- decide whether to expand DAW coverage, remain a focused adapter, or integrate
  capture directly into mastering tools.

Creation-stage evidence increases the usefulness of master-stage registration
by adding inspectable context and earlier commitments. It does not replace the
identity, watermarking, recovery, credential, consent, rights, or registry
layers that turn integrity evidence into durable external trust.
