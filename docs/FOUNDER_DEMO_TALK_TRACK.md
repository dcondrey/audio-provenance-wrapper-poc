# Founder Demo Talk Track

## Five-minute script

### 0:00–0:40 — Set the boundary

Say: “This is a creation-stage evidence adapter, not a full Ableton provenance
claim. I route one stem through an opt-in pass-through plug-in. The downstream
opportunity is to carry truthful creation evidence into master-stage
registration, where identity, soft binding, signing policy, recovery, and the
registry still begin.”

Run:

```sh
./scripts/demo.sh
```

The command preflights the machine, creates a fresh timestamped session, and
opens the live dashboard and clean export folder.

### 0:40–1:40 — Show observation, delivery, and coverage

1. Insert **Audio Provenance Capture** on one Ableton audio track.
2. Play the track.
3. Point to the plug-in instance ID, submitted buffers/samples, hashed windows,
   FIFO drops, prepared events, UDP attempts, locally emitted datagrams, and
   daemon-acknowledged sequence.
4. Say: “A successful UDP write is still not receipt. This separate line means
   this local daemon accepted and persisted the event, then returned a scoped
   ACK. Stale, rejected, missing, mismatched, and gap states remain visible.”
5. On the dashboard, show the matching plug-in instance and capture-session IDs,
   highest accepted/contiguous sequence, received events, ACKs issued, gaps,
   chain breaks, and proof-level legend.

### 1:40–2:45 — Export, align, and hand off

Export a WAV or AIFF into the folder opened by the launcher. The fight card
opens automatically.

Say: “The hard hash is directly observed. The routed-to-export link is still
inferred, even with a strong alignment. The chart compares bounded relative RMS,
zero-crossing, crest, and coarse energy-envelope features emitted from routed
windows against the export with gain normalization and offset search. It is not
a watermark.”

Show:

- session timeline;
- coverage status and counters;
- alignment confidence, matched coverage, and offset;
- matched/comparable counts and the green/amber window visualization;
- explicit unknowns;
- downstream registration handoff and missing requirements.
- downloadable deterministic bundle and signed canonical index.

### 2:45–3:25 — Positive verification

Copy the manifest path from the dashboard and run:

```sh
./scripts/verify_demo.sh /path/to/export_manifest.json
```

Say: “Verified is a local POC integrity outcome. The Ed25519 signature is
independently checkable with its public key, but the self-generated signer has
no externally verified identity.”

### 3:25–4:25 — Tamper failure

Run:

```sh
./scripts/demo_adversarial.sh /path/to/export_manifest.json
```

The command leaves the valid original untouched. It verifies the original,
alters a disposable export copy and reports `changed`, alters a disposable
manifest copy and reports `changed`, then creates an export-only session.

### 4:25–5:00 — Honest no-evidence result

Point to the export-only rehearsal result:

- export integrity: `verified` by the local POC verifier;
- observation coverage: `unknown_coverage`;
- routed/export association: unavailable;
- no claim that routed audio was absent or synthetic.

Close with: “The wedge is not more metadata. It is a disciplined handoff from
creation evidence into a downstream trust system that can establish the things
this plug-in cannot.”

If Ableton is unavailable, run:

```sh
./scripts/presenter_fallback.sh
```

Say explicitly that this is a synthetic operational fallback. It exercises ACK
receipt, a fixed gain plus offset transformation, inferred alignment, sealing,
public-key verification, and bundle integrity; it is not manual Ableton proof.

## Recovery

- UDP port busy: stop the prior demo process, then rerun `./scripts/preflight.sh`.
- No plug-in events: confirm VST3 insertion, play non-silent audio, and compare
  the plug-in and dashboard instance IDs.
- Locally emitted but not acknowledged: compare the plug-in capture-session ID,
  restart the launcher, and do not describe UDP send success as receipt.
- No export detected: export directly into the timestamped `exports/` folder
  and wait for the file to become stable.
- Fight card did not open: use the dashboard Artifact links or open the session
  `manifests/` folder.
- Start clean: stop the command and rerun `./scripts/demo.sh`; it never appends
  into the previous session.
