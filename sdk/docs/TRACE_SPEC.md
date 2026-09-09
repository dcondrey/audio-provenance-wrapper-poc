TRACE — RECOVERY ENGINE
Lives at packages/sdk/src/apw-trace/. Bare audio file in, admitted signed manifest out. It is the only producer of VerifyResult.trace.

CONTRACT. verify() NEVER throws for a provenance outcome. It throws only for caller errors: unreadable path, oversize input, malformed options. Every provenance condition, INCLUDING A REGISTRY OUTAGE, resolves to one of the four statuses plus `incomplete: true`. This is the rule that keeps a network fault from becoming a verdict.

--- STAGE 0: INGEST (bounded, untrusted) ---
0.1 Open by BYTES, never by extension. Sniff from magic: RIFF/WAVE, FORM/AIFF, ID3+MPEG sync, fLaC, OggS, ftyp M4A/MP4, .opus.
0.2 Bounds BEFORE work: maxBytes 512 MiB, maxDurationSeconds 3600, decodeTimeoutMs 30000. Over budget throws AudioProvenanceInputError — never a status.
0.3 fileSha256: streaming SHA-256 over raw bytes as read, 1 MiB chunks. Mirrors daemon/common.py sha256_file_with_size; KEEP THE BYTE COUNT, the length is part of the binding.
0.4 Decode to canonical analysis PCM: float32, per-channel retained for Watermark, mono-sum retained for the hash chain.
0.5 decodedAudioSha256: SHA-256 over a canonical PCM serialization (16-byte header of channel count and sample rate, then interleaved LE float32). Computed lazily at Stage 3. This is a SECOND exact binding that survives container-metadata-only edits (an added ID3 tag, a rewritten RIFF LIST chunk) while staying bit-exact over audio.

--- THE FIVE-RUNG LADDER ---
The rung determines WHETHER a manifest was recovered and WHO ASSERTED the association. It does NOT by itself determine verified-vs-changed; the hard-binding recomputation does. Method and matchBasis are independent axes.

RUNG 1 — EMBEDDED / SIDECAR MANIFEST. JUMBF scan in the container provenance slot (RIFF `C2PA` chunk, WAV `aprv` chunk, ID3 GEOB, MP4 uuid box, Ogg/FLAC metadata block); then sidecars `<file>.c2pa`, `<file>.audio-provenance.json`, `<dir>/.audio-provenance/<basename>.json`. Sidecar paths resolve against the input's REAL directory only; no path from inside a manifest is ever followed (path-traversal defense; daemon's _safe_path is the precedent).
  Association asserter: THE FILE ITSELF. So a soft-binding FAIL here is a false claim by the file, not a rejected guess by us: `untrusted`, never discard-and-continue.
  Proof level: directly_observed when the hard binding matches; user_declared when it mismatches.
  A corrupt/truncated store is NOT a candidate: record `embedded_unparseable` and continue. Forcing a downgrade gains an attacker nothing, because every lower rung's ceiling is at most what rung 1 could have given.

RUNG 2 — EXACT CONTENT HASH. registry.lookupByContentHash(fileSha256). Breaks on any transcode or container rewrite. Proof level directly_observed: the index key is the whole file.

RUNG 3 — EXACT DECODED-AUDIO HASH. registry.lookupByDecodedAudioHash(decodedAudioSha256). Tolerant of container-only change, breaks on any re-encode. A distinct rung because it yields a distinct matchBasis and a caller must be able to tell "the bytes moved but no sample did" from "nothing moved". Proof level directly_observed.

RUNG 4 — WATERMARK. Decode the 56-bit payload, then registry.lookupByWatermark(payload).
  LOCATOR RE-DERIVATION IS MANDATORY AND TERMINAL. After fetch, recompute the locator from the RECEIVED manifest bytes — SHA-256("audio-provenance-locator-v1" || 0x00 || the document's own `portable_signature.public_key_hex` || its own `locator_salt`), leading 48 bits — and compare it against the payload's locator. Read the key and the salt out of the received document, NEVER from the backend's index key (that is the claim this check exists to distrust) and never from a trust anchor (trust resolution is a later stage). A received record with no usable `locator_salt` derives nothing, and that is a MISMATCH, not a skip. Mismatch is `untrusted / locator_mismatch`, terminal, NO DESCENT. This is what stops a registry substituting a different manifest for the same payload.
  Collisions are handled, not assumed away: lookupByWatermark returns ALL matching refs (expect of order one collision at 2^24 registered works), each disambiguated by full-digest re-derivation plus soft-binding association; two survivors are `ambiguous_binding`.
  Association asserter: Trace, from a signal physically recovered out of the audio. A soft-binding FAIL here is a rejected guess: DISCARD the candidate and continue the ladder.
  Proof level: directly_observed only when the payload decoded AND the confidence class is `strong` AND the soft binding passes; `inferred` for class `single` or when the soft binding is unavailable. A `single` detection is never directly_observed.

RUNG 5 — AUDIO FINGERPRINT. Last resort, ALWAYS `inferred`. No binding is recovered from the audio at all; a manifest is GUESSED from perceptual similarity.
  DEFAULT POLICY EMITS NO CANDIDATE: findings live in recovery.diagnostics.fingerprint and the terminal status is `not_found`. Opt in with `acceptInferredAssociation: true` to let it produce `changed` or `untrusted` — never `verified` on this basis, and never `verified` at all unless the hard binding independently recomputes to MATCH.

FINGERPRINT DESIGN (rung 5). Landmark/constellation hashing. Chroma and HPCP are REJECTED: they match compositions, so a cover version or a different master would return the original's manifest — an identity error dressed as robustness.
  Preprocess: mono sum, polyphase resample to 11025 Hz (64-tap Kaiser, beta 8.6), 1-pole 20 Hz high-pass. NO gain normalization; landmark selection is amplitude-invariant by construction.
  STFT: Hann 1024 (92.9 ms), hop 128 (11.61 ms), 86.13 frames/s, 10.77 Hz/bin. Hashing band bins 3..430 (32 Hz - 4.63 kHz).
  Peaks on log10 magnitude, requiring all of: max over +-11 bins x +-5 frames; above a per-band adaptive floor (6 log-spaced bands, decay exp(-1/30)/frame); above (median frame energy + 6 dB). Target density 28 +- 6 peaks/s, bisect the global floor up to 3 times to hit it. Uniform density stops a loud section starving a quiet one.
  Pairing: target zone dt in [2,63] frames, df in [-63,+63] bins, fan-out 8. Hash u32 = f1(9) | f2(9) | dt(6) | schemeVersion(8). Posting = (hash u32, trackKey u64, t1 u32) = 16 bytes.
  Query robustness: emit each query hash 3x with f2 in {f2-1, f2, f2+1}. Time-scale sweep at {0.98,0.99,1.01,1.02} only if the 1.00 pass fails, so the common case pays nothing.
  Index: filesystem backend is one fp.idx sorted by (hash, trackKey, t1) plus a 2^24-entry u32 offset table over the hash's high 24 bits (64 MiB), mmap'd read-only. Postings per hash capped at 1000; a hash appearing in a thousand recordings carries no discriminative information and is dropped from the index.
  HTTP backend: THE CLIENT NEVER TRUSTS A SERVER-SUPPLIED SCORE. It re-derives the offset histogram locally from returned postings. A registry that can fabricate a score can fabricate a match.
  Budget: 2e6 postings scanned max; exceeding aborts the rung with `fingerprint_budget_exceeded` rather than degrading silently.
  Scoring: histogram (t1_ref - t1_query) into 1-frame bins, smooth by adding both neighbors, peak = largest smoothed bin. A hit requires ALL of: peak >= 15; peak >= 3x the largest bin outside +-5 frames of the peak (rejects a diffuse pileup); aligned anchors span >= 3.0 s of query time (rejects a single transient coincidence); query duration >= 5.0 s.
  coherenceRatio = peak / (query hashes having at least one posting on that track), clamped [0,1]. match on this basis = min(0.99, coherenceRatio).
  NON-COLLISION WITH PRIOR ART: this is a different quantity from daemon/audio_association.py's `confidence`, which is a weighted point similarity over {relative_rms 0.25, zcr 0.40, crest 0.10, energy_envelope_4 0.25} accepted at confidence >= 0.74. Never compare the two numbers or reuse the threshold.

--- STAGE 2: CANDIDATE ADMISSION (strict order; nothing bound is read before the signature check) ---
2.1 Parse manifest bytes.
2.2 CANONICALITY ASSERT. Recompute apw-json-sort-v1 over the parsed object and BYTE-COMPARE against the bytes received. Mismatch: `untrusted / noncanonical_manifest`.
    THE SIGNATURE IS VERIFIED OVER THE RECEIVED BYTES, NEVER OVER A RE-SERIALIZED OBJECT. c14n exists solely to prove the received bytes are canonical. This single decision eliminates the entire cross-language divergence class as a source of false verdicts: a JS float-formatting bug can then only cause a loud `noncanonical_manifest`, never a silent bad signature. It is strictly better than signing over a re-canonicalized object and is adopted over the competing SDK-design formulation.
2.3 Signature verification (Ed25519 default; ES256/PS256 for C2PA COSE). Absent, unsupported alg, or invalid: `untrusted`, terminal.
2.4 Schema and invariant validation, now that the bytes are trusted. Port of daemon/schema.py validate_manifest_invariants, including the proof-level lattice, the rule that an established stem_export_association stays `inferred`, and that an unavailable association stays `unknown_unobserved`.
2.5 Locator re-derivation for registry-fetched candidates (rung 4).
2.6 Trust chain resolution: anchored | unanchored | failed.

--- STAGE 3: BINDING EVALUATION (only reached with a signature-valid, schema-valid manifest) ---
3.1 HARD BINDING H: recompute the manifest's declared hard binding over this file. MATCH | MISMATCH | UNCOVERABLE.
3.2 SOFT BINDING A: does the manifest's soft descriptor (Watermark block schedule, the signed reference constellation) describe THIS audio? PASS | FAIL | UNAVAILABLE. This is the watermark-copy-attack check: a payload lifted from one track and pasted into another fails here.

3.2a THE SIGNED REFERENCE CONSTELLATION. A Audio Provenance record commits, in its signed `fingerprint` block, to the landmark constellation of the audio its hard binding covers. algorithm = `apw-trace-landmark-v1`; digest = lowercase hex of the encoded constellation. Encoding: magic "GTFR", schemeVersion u8, varint frame count, varint peak count, then peaks in strictly ascending (frame, bin) order, each as varint(frameDelta) followed by varint(bin - 3) on a new frame or varint(binDelta) within one. About 2.2 bytes per peak, so about 62 bytes per second of audio at the scheme's 28 peaks/s, linear in duration. Capped at 262144 peaks; a work above the cap carries NO reference and the signer is told so, because a thinned reference would fail to corroborate a legitimate transcode and report it as `changed`.
  It is extracted from the SAME buffer the hard binding hashes — after marking, not before — so it describes audio that exists on disk.
  COMPARISON. Rebuild the reference landmarks with the index-side pairing (no query smear), load them into a ONE-track index, and run the rung-5 search, rate sweep included. This selects no manifest; the manifest is already in hand. It answers one question about one record.
  ACCEPTANCE PREDICATE, in full: every rung-5 alignment gate (peak >= 15; peak >= 3x the largest bin outside +-5 frames; aligned span >= 3.0 s; query >= 5.0 s), signed duration and block-distribution consistency, zero offset discontinuities, localRegionCoverage >= 0.80, and maximumUnexplainedInteriorGap <= 0.75 s. Local coverage partitions the presented query into 0.5-second regions and counts only regions with locally coherent aligned landmarks. It never infers coverage merely from the first and last aligned landmark.
  WHY LOCAL COVERAGE. coherenceRatio is degenerate on a near miss, while the old global aligned span was structurally blind to an arbitrary substituted middle: matching landmarks on both edges made the span read 1.0. A transcode or an arbitrary crop should retain coherent local regions and a single time mapping. A substituted, duplicated, or reordered region creates uncovered local regions, an excessive interior gap, a duration/block-distribution contradiction, or an offset discontinuity. These are signed recording-association tests; mark recovery alone does not satisfy them.
  FALSE-POSITIVE QUALIFICATION. The earlier 812-pair run measured 0 affirmations (95% rule-of-three upper bound 0.0037) for the alignment gates, but it predates the local predicate and is not represented as pricing the new composite. The permanent plan is `qualification/watermark-adversarial-v1.json`; the fast structural suite and the full null/adversarial corpora report false acceptance, locator recovery, and materially-altered recording rejection separately. The long corpus must pass before the new predicate is called qualified.
  The threshold is a compile-time constant. `--threshold` moves the Watermark coverage guard and is deliberately NOT plumbed here: a verifier-tunable soft-binding gate is a gate that gets tuned until the case passes.
3.3 For an APW manifest, replay the hash-chain and evidence checks from daemon/verify.py: window_hash = SHA-256(ASCII bytes of prev_hash || raw LE float32 samples of a 4096-sample mono-summed window), genesis literal = the 7 ASCII bytes "genesis". Fold findings into recovery.diagnostics.apw. These REFINE reasons; they never override status precedence.

--- STAGE 4: ARBITRATION ---
- Descend only while the current rung yields no ADMISSIBLE candidate (manifest bytes obtained AND parseable).
- Terminal status comes from the STRONGEST rung that produced a non-discarded candidate. A signature-invalid embedded manifest is a POSITIVE FINDING: it yields `untrusted`, and a lower rung may NOT launder it back to `verified`.
  CONSEQUENCE, STATED OPENLY: an attacker can append a broken manifest to someone's file and inflict `untrusted`. That is a reputation nuisance, not a false accept, and the file genuinely does contain a broken manifest. Both the broken candidate and any lower-rung finding appear in result.recovery.
- A candidate discarded for soft-binding FAIL on rungs 4-5 is not a candidate FOR THIS AUDIO; the ladder continues and the discard is recorded in recovery.discarded[].
- After the ladder halts, dedupe survivors by SHA-256 of their received bytes. Two or more distinct survivors: `untrusted / ambiguous_binding`, both listed.
- recovery.exhausted records whether every rung actually ran. `not_found` therefore NEVER means "no manifest exists"; it means "none was recovered", and exhausted plus incomplete say how hard we looked.

--- STATUS DERIVATION: FOUR INDEPENDENT PREDICATES ---
`match` does not drive `status` — with one guard, added below. Collapsing them would make Watermark's whole point (surviving lossy transcode) report an alarm on its intended input.

  1. Trace recovered a manifest reference on any rung.
     FAIL -> not_found, match 0, identity null, signedAt null.
  2. The Ed25519 signature verifies over the received canonical bytes.
     FAIL -> untrusted.  (Note: an invalid signature is untrusted, not changed; `changed` is reserved for audio that moved under a VALID signature.)
  3. The presented audio BINDS, by one of three routes:
       a. hard binding exact (match = 1.0, proofLevel directly_observed); OR
       b. hard binding UNCOVERABLE and the Watermark soft binding at or above threshold (match = soft score, proofLevel directly_observed on a `strong` detection); OR
       c. hard binding PRESENT AND MISMATCHED, but BOTH a CRC-valid Watermark payload was recovered from this audio AND the signed reference constellation affirms it is the same work (match = alignmentCoverage, capped 0.99, proofLevel INFERRED).
     FAIL -> changed.
     REVISED, AND THE REVISION IS THE POINT. The earlier rule was that a present-and-mismatched hard binding always fails predicate 3. It made the watermark decorative: the mark located the record and then contributed nothing, so an ordinary mp3 transcode reported `changed`, which says TAMPERED about a file nobody tampered with. Route (c) is not the soft binding overriding a failed hard one; it is the soft binding answering the question the hard binding cannot reach after a lossy path — is this the same work — while the hard binding keeps sole authority over the question it does answer, are these the same bytes. The two are never conflated: route (c) can never return match 1.0 and can never return proofLevel directly_observed.
     ROUTE (c) REQUIRES THREE TERMS, and the third is the one a splice fails: the mark must also clear the block-coverage guard (softBindingThreshold, default 0.72) over the PRESENTED audio. Nine marked seconds pasted into eleven unmarked ones is genuinely part of the signed work, so the constellation aligns across the marked portion and the alignment floor is the wrong instrument; one agreeing block in two is the fact that says the rest of this file is not the recording that was signed. Measured: alignment coverage 0.44, mark coverage 0.50, verdict `changed`. This reuses the guard the spec already describes as failing "a 9-second marked insert spliced into an unmarked track" rather than reopening the reference-constellation floor, whose false-positive number is measured and frozen.
  ROUTE (c) REQUIRES BOTH SOFT SIGNALS. A fingerprint affirmation alone never verifies, on this route or any other; that invariant is unchanged. A CRC-valid payload whose reference constellation DISAGREES is the watermark-copy attack and is `changed`, which is the one case where `changed` is the true statement about a mark that decoded.
     Route (c) is priced like every other soft binding: without a measured Watermark false-positive rate it degrades to `untrusted / soft_binding_false_positive_rate_unknown`, never to `changed`. Reporting a work the soft binding affirmed as tampered because the verifier was not given a bench report would restate the same false accusation in a different place.
  4. The signer resolves through a configured trust anchor to a name.
     FAIL -> untrusted, identity null.

  All four pass -> verified.

  THE COVERAGE GUARD (resolves the one real hole in the permissive table). A soft-only `verified` additionally requires match >= softBindingThreshold, default 0.72 (the POC's alignment_threshold). Without it, one agreeing Watermark block out of 27 in a 4-minute track gives match ~= 0.037 and would read `verified`. Below threshold: `untrusted` with finding `soft_binding_coverage_low` and identity null. This passes a genuinely short marked file (one block present, match 1.0) while failing a 9-second marked insert spliced into an unmarked track.

  binding.threshold reports the threshold the reported `match` was actually gated against: softBindingThreshold for a Watermark coverage score, the reference-constellation coverage floor for route 3(c). A number gated against one threshold and printed beside another is a confidently wrong sentence, which is worse than no sentence.

  So { status: "verified", identity: "Signal Room Studios", match: 0.94 } is a legal and expected row: recovered from an mp3, same work, named signer. The honesty lives in binding.proofLevel === "inferred" and in trace, not in downgrading the status.
  identity CAN be non-null on `changed` (a known signer, altered audio). It is ALWAYS null on `untrusted` and `not_found`.

--- match SEMANTICS (graft G2, made total) ---
  matchBasis "hard_exact":        match = 1.0 exactly. THE ONLY BASIS THAT MAY RETURN 1.0.
  matchBasis "apw_watermark":          match = (blocks whose CRC passes and whose payload equals the modal payload) / (whole blocks present in the audio), capped at 0.99.
  matchBasis "fingerprint":       match = min(0.99, coherenceRatio).
  matchBasis "mark_and_fingerprint": match = min(0.99, alignmentCoverage). Route 3(c): a CRC-valid Watermark payload corroborated by the signed reference constellation. proofLevel is ALWAYS inferred, and `changed` is what this basis becomes when the constellation disagrees.
  matchBasis "none":              match = 0.
  Soft bases cap at 0.99 so `match === 1.0` is an exact iff-test for a hard binding.
  falsePositiveRateAtMatch is REQUIRED non-null for every soft basis. It is read from the bench null-test table, NOT from a formula. CONSEQUENCE, AND IT IS A DELIBERATE FORCING FUNCTION: soft-binding `verified` cannot ship before the null test passes, because there is no honest number to put in that field until then.
