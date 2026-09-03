# Founder Demo Talk Track

Companion to `docs/DEMO_RUNBOOK.md`. The runbook holds the commands and the
recovery. This holds the words, and the boundary each sentence must not cross.

---

## The honest position, in four sentences

Say this before anything runs, because everything after it is calibrated to it.

> One plug-in observes one routed stem. It directly hashes what passed through
> it, and the daemon directly hashes the exported file. Linking the two is an
> inference from signal features, and I label it inferred rather than dressing
> it as proof. Everything the plug-in could not see is written down as
> unobserved, in the manifest, signed.

What that buys: a claim that survives a hostile reading. What it costs: nothing
we actually had.

---

## Directly observed, inferred, not claimed

Keep these three columns straight. Every field in the manifest carries one of
`directly_observed`, `inferred`, `user_declared`, `externally_verified`, or
`unknown_unobserved`, and the fight card prints the label next to the claim.

**Directly observed.** The audio buffers routed through the plug-in, hashed in
a chain. The bytes of the exported file, hashed by the daemon. The delivery
record: which datagrams this daemon accepted, persisted and acknowledged, with
gaps, rejections and chain breaks left visible. The C2PA hard binding over the
exported WAV.

**Inferred.** The link between the routed stem and the export. It compares
bounded relative RMS, zero-crossing rate, crest factor and a coarse energy
envelope, with gain normalisation and an offset search. A strong match is still
an inference. It is not a watermark and it is not a hash match.

**Not claimed, and stated as such.** Full DAW provenance. Sample rights or
licence. Preset or device identity. Hidden plug-in state. Anything routed around
the plug-in. That bypass was impossible. And, most important, the identity of
the signer.

---

## Two signature layers, and they are not the same thing

This is the sentence most likely to be got wrong under pressure. There are two
independent layers and conflating them will be noticed by anyone who knows C2PA.

**Layer 1, the local evidence manifest.** JSON, sealed with an HMAC for
same-machine integrity and an Ed25519 signature that anyone can check against
the pinned public key. `./scripts/verify_demo.sh` checks the export hash, the
evidence prefix hashes, re-derives the coverage counters from the bound
evidence, and reports `verified`. That word means local integrity. It is not a
registry result and not an identity result.

**Layer 2, C2PA.** A real manifest embedded in the exported WAV as an appended
`C2PA` chunk, with a `c2pa.hash.data` exclusion covering exactly that appended
region. Signed ES256 over P-256 with an X.509 chain, leaf then root. The
assertions are `c2pa.actions.v2` and a project assertion listing what was not
observed. It is read by `c2patool`, which we did not write, and it reaches
`"validation_state": "Trusted"` with zero failures against the shipped root.

Say it as:

> The Ed25519 signature covers our own evidence manifest. The C2PA layer is
> separate: ES256 over an X.509 chain, embedded in the WAV, and here is a tool I
> did not write reading it back as Trusted.

Then run the c2patool command from Runbook Step 1 or Step 6. Twenty seconds,
and it is the most externally checkable thing in the demo.

The chain is self-issued. A trusted result proves the bytes match a signature
made by that key. It attests to no person and no organisation. Say that in the
same breath as the word Trusted, every time.

---

## The four verification states

The vocabulary is exactly four, and three of them are not failures.

| State | What it means | What it does not mean |
| --- | --- | --- |
| **verified** | A mark was found, its signer chains to an anchor this verifier holds, and the bytes still match the binding. | That the signer is a verified identity. |
| **registered but changed** | The signer is trusted and the file was altered after signing. | That the alteration was malicious, or that the original claim was bad. |
| **mark found, claim not trusted** | A mark is present and intact, but the signer does not chain to any anchor this verifier holds. | That the mark is forged. It usually means the verifier lacks the anchor. |
| **nothing found** | This file carries no mark. | **Nothing at all about how it was made.** |

Grading rule worth stating if pressed: an untrusted signer outranks a hash
mismatch. Anyone can self-sign a file and change a byte. Reporting that as a
registration the system recognises would be strictly stronger than the truth.

**The line to land, slowly:**

> A missing mark is never evidence of synthetic origin. It is an absence of
> evidence. Most audio in the world will have no mark for years. A system that
> treats unmarked as suspicious will be wrong about almost everything and will
> be abandoned by the people it accuses.

---

## Five-minute run

**0:00 Set the boundary.** The four sentences above. Then start the session:
`./scripts/demo.sh ./demo-output imported_sample "/path/to/YourSet.als"`.

**0:40 Observation and delivery.** Insert the plug-in, play. Point at plug-in
instance ID, submitted buffers and samples, hashed windows, FIFO drops, UDP
attempts, locally emitted datagrams, and the acknowledged sequence.

> A successful UDP write is not receipt. That separate line means this daemon
> accepted the event, persisted it, and returned a scoped acknowledgement.
> Stale, rejected, missing, mismatched and gap states all stay visible.

**1:40 Export and align.** Deactivate the device, export 16-bit PCM WAV into the
watched folder. The fight card opens. Show the alignment confidence, matched
coverage, offset, the green and amber window bars, and the explicit unknowns.
Repeat that the association is inferred.

**2:45 Verify, both layers.** `verify_demo.sh` for the local manifest, then
`c2patool ... trust --trust_anchors ...` for the C2PA layer. Read the warnings
out loud rather than letting someone find them: local integrity is not hardware
attestation, and signature validity is not identity.

**3:25 Break it.** Copy the signed clip, flip one byte inside the PCM data,
re-read. `assertion.dataHash.mismatch`, `"validation_state": "Invalid"`, graded
`registered but changed`. The original is untouched.

**4:10 The honest null.** Read the unsigned clip. `Error: No claim found`, graded
`nothing found`. Same eight seconds of audio, byte-identical PCM. Then the
missing-mark line above.

**4:40 Close on the seam.** Below.

---

## Why the seam is the offer, not an integration

Be direct about this. It is a strength, not an apology.

> I did not integrate with your stack, and I want to be precise about why. There
> is no published SDK, no API, no package and no spec I could build against. So
> claiming an integration would have been the first false claim in a project
> whose entire discipline is not making false claims.

> What I built instead is the seam. The provider interface is neutral and
> vendor-free, all the way into the signed manifest fields; there is no vendor
> name anywhere in the code or in anything signed. Today a local reference
> provider implements it: it holds the key custody, issues the leaf, signs, and
> carries a soft-binding mark key. Swapping in a real provider is an adapter
> behind the same interface, not a rewrite.

Then name what only they can supply, in their own functional terms:

- **Identity.** We produce key possession. Binding a key to a real person or
  organisation is theirs.
- **Key management with revocation that does not erase history.** Ours is a
  local store. Revocation that invalidates future signing without destroying the
  provenance of past work is theirs.
- **Inaudible soft binding.** Our association is a feature-alignment inference.
  A perceptual mark that survives re-encoding is a different threat model and a
  different mechanism, and it is theirs.
- **Registry.** We emit a neutral registration handoff with the trust
  requirements it cannot satisfy listed explicitly. Where that handoff lands is
  theirs.

Close:

> The wedge is not more metadata. It is disciplined creation-stage evidence,
> labelled honestly, handed to a downstream system that can establish the things
> a plug-in never could.

---

## Fallback

If Ableton is unavailable, run `./scripts/presenter_fallback.sh` and say plainly:

> This is a synthetic operational fallback. It exercises acknowledgement receipt,
> a fixed gain and offset transformation, inferred alignment, sealing, public-key
> verification and bundle integrity. It is not manual Ableton proof, and I am not
> going to present it as one.

The Ableton gate itself was closed live on 2026-08-29 and is recorded per step in
`docs/VALIDATION.md`. Offer that record rather than re-running it in the room.

---

## Recovery, in the room

- **UDP port busy:** stop the prior process, rerun `./scripts/preflight.sh`.
- **No plug-in events:** confirm the VST3 is inserted, play non-silent audio,
  compare the plug-in and dashboard instance IDs.
- **Emitted but not acknowledged:** compare capture-session IDs, restart the
  launcher, and never describe send success as receipt.
- **No export detected:** export directly into the timestamped `exports/` folder
  and wait for the file to settle.
- **Association `unavailable`:** the render was 32-bit float. Re-export at
  16-bit.
- **C2PA reads untrusted:** wrong anchor. Shipped clips take
  `packaging/assets/demo-root-ca.pem`; session exports take
  `~/.apw/provenance/ca/root_cert.pem`.
- **Fight card did not open:** use the dashboard artifact links.
- **Start clean:** rerun the same three-argument `demo.sh`. It never appends into
  a previous session.

---

## Sentences to never say

- "This proves the audio was made in Ableton."
- "This is a verified identity."
- "No mark means it is AI."
- "The routed audio matches the export" without the word inferred.
- "Bypass was impossible."
- "We integrate with their SDK."
