CORPUS. 200 tracks >= 60 s, STRATIFIED AND LABELLED by class: dense pop/EDM, rock, orchestral, solo piano, unaccompanied voice, jazz small ensemble, spoken word, plus 20 pathological items (near-silence, sustained pure tone, white noise, a clipped loudness-war master, mono-in-stereo, heavily widened stereo). Class labels are mandatory, not decoration: every failure mode in this design is class-correlated, so a pooled average would hide exactly what the bench exists to find.

CHANNELS, each applied independently plus three named compound chains (transcode: MP3 128 -> AAC-LC 96 -> resample 48; broadcast: -14 LUFS multiband + MP3 192; capture: RIR + AAC-LC 128):
  1. identity
  2. MP3 CBR 320/192/128/96/64 and VBR V0/V2 (LAME)
  3. AAC-LC 128/96/64; HE-AAC v1 64/48; HE-AAC v2 32 (fdk-aac)
  4. Opus 128/64/32
  5. resample 44.1->48->44.1, 44.1->22.05->44.1, 44.1->16->44.1
  6. speed +-0.1/0.5/1/2/5/8% (resample, pitch follows)
  7. phase-vocoder time-stretch +-2%, pitch-preserving  [expected failure]
  8. requantize to 8 bit; TPDF dither; +-6 dB gain; hard clip at -0.1 dBFS; DC offset
  9. loudness normalization to -14 LUFS with a multiband limiter
  10. band-limit: 1 kHz HP; 5 kHz LP; 4 kHz LP  [expected failure]; 3.4 kHz telephony  [expected failure]
  11. multiband compression with crossovers DELIBERATELY OFF the guard mask (e.g. 1500/2800 Hz) — the guard mask's own adversarial row, without which it is untested decoration
  12. simulated AR: measured RIRs from an open corpus at 0.05/0.2/0.5/1/2/5 m, then 1-4 kHz bandpass, then AWGN at 20 and 25 dB SNR, then a rate drift drawn from +-1%, then a start offset from [0,1) s
  13. real AR: >= 20 tracks played on a consumer loudspeaker and captured on a phone and a USB mic at 5/20/50/100 cm, in one treated and one untreated room

PER-RUN METRICS. Exact 56-bit payload match (THE ONLY PASS/FAIL METRIC); result.match; confidence class; preamble offset error and rho error; seconds of audio consumed before first decode; wall-clock RTF. Embedder-informed slot accuracy is recorded FOR DIAGNOSIS ONLY and may never appear in a pass criterion — it is the metric that makes a broken detector look healthy.

PASS THRESHOLDS.
  >= 99% of 200 tracks decode exactly within 30 s of audio: identity, MP3 >= 128, AAC-LC >= 96, HE-AAC v1 >= 48, Opus >= 64, all resample rows, speed within +-2%, gain, dither, requantize, clip, DC, 1 kHz HP, 5 kHz LP, transcode chain.
  >= 90%: MP3 64-96, Opus 32, LUFS normalization, broadcast chain, off-mask multiband (row 11), speed +-5%.
  >= 60%: HE-AAC v2 32.
  NO THRESHOLD, recorded as EXPECTED FAILURES: 4 kHz LP, 3.4 kHz telephony, phase-vocoder stretch, speed +-8%, all of rows 12 and 13. A pass here is a pleasant surprise; a fail is not a defect.

THE NULL TEST — THE GATE, NOT A REPORT. Detector over 1,000 UN-watermarked tracks crossed with every channel above, ~24,000 runs, at the production tau_sync and the production CRC-32C. PASS = ZERO payload accepts. A single accept invalidates the section 9 false-positive arithmetic and forces a wider CRC or a higher tau_sync; it is not waived as an outlier. Additionally instrument the observed sync-candidate count per clean track against the predicted ~26: a large discrepancy means tau_sync was calibrated against the wrong noise distribution, which is a latent defect even when the accept count is zero. THIS TEST PRODUCES `falsePositiveRateAtMatch`, so until it passes, soft-binding `verified` cannot ship — there is no honest value for a required field.

HARNESS SELF-VALIDATION, RUN FIRST. Before any real embedder exists, run the harness against a stub embedder that writes noise and a stub detector that returns a fixed payload. The null test MUST fail loudly and the channel matrix MUST read zero. A harness that has never failed a known-bad implementation has been executed, not validated, and "bench before watermark" is ceremony without it.

INAUDIBILITY GATE. PEAQ basic (ITU-R BS.1387) ODG on all 200 embedded tracks. Pass: median >= -1.0, 5th-percentile-worst >= -2.0, no track below -2.5. Then a 12-subject blinded ABX on the 10 worst-ODG tracks; pass = no track significant at p < 0.01 after Holm correction. Only on completion may `measuredTransparency` stop being null.

THE FALSIFIABLE AR PREDICTION, STATED IN ADVANCE SO THE BENCH CAN REFUTE IT. This design predicts rows 12 and 13 fail. It is falsified only if BOTH hold: simulated AR at 0.05 and 0.2 m yields >= 90% exact decode over 60 s, AND real AR at 20 cm in the treated room yields >= 80%. Anything less and the acoustic claim stays withdrawn and the docs keep saying verify() on a microphone recording returns not_found by design.

CROSS-LANGUAGE SUITE (unit 1, and the one that runs on every commit). A generator script drives the POC venv python over every real manifest in the POC plus a fuzz corpus and emits expected canonical bytes; TS asserts byte equality. Fixture provenance rule: negative vectors may NOT be produced by this package's own writer, grammar, or docstrings — writing the parser and its fixtures from the same mental model in one turn only confirms what was already believed. Mandatory vectors, all confirmed differentially in this session: 1.0, 1e15, 1e16, 1e-5, 1e-7, -0.0, NaN/Infinity rejection, lone-surrogate rejection, the astral key-sort case {z, U+FFFD, U+1F3B5}, and legacyAsciiJsonBytes against generator.py:425's implicit ensure_ascii=True form.

TESTS THAT EARN THEIR PLACE ELSEWHERE (default is no new test; these pin invariants or guard boundaries):
  - Gain invariance: a scalar gain leaves d[s] unchanged to float epsilon. The single test that justifies choosing this design.
  - Smooth-EQ invariance over an 86 Hz span, to first order.
  - Chunk-boundary equivalence of the streaming spectrogram against a whole-file reference.
  - Two-pass OLA closure residual below DELTA/20.
  - A payload lifted from track A and pasted into track B fails the soft-binding association check (the watermark-copy attack).
  - Locator mismatch after registry fetch is terminal and does not descend the ladder.
  - A registry that returns a fabricated fingerprint score does not change the verdict (the client re-derives the histogram).
  - An unreachable backend sets `incomplete` and never changes `status`.
  - Coverage guard: one agreeing block out of 27 yields `untrusted / soft_binding_coverage_low`, not `verified`.
  - Boundary coverage is limit-1/limit/limit+1, not "small" and "large": audio of 9.65/9.66/9.67 s and 19.31/19.32/19.33 s; match at 0.71/0.72/0.73.
  - A self-generated signature yields identity null and unknown_unobserved even when cryptographically valid.

DETERMINISM AND RATCHET. Every run is seeded; the (track, channel, seed) triple is recorded so any failure reproduces from the tuple alone. The bench emits a JSON matrix; `audio-provenance bench ratchet` compares it against the committed baseline and fails CI on any pass-rate regression on a row that previously met threshold. Delete-on-sight applies: any pass-always, vacuous, source-grep, or elapsed-time-based test is deleted and replaced in the same change, never skipped or exempted.
