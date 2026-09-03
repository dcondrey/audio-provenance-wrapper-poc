GENOMARK v1 — LEP-QIM
Algorithm id: `genomark-lepqim-v1`. Statistic: dither-modulated QIM over the trimmed mean of adjacent-cell log-spectral-energy differences.

=====================================================================
0. HOW THIS SPEC WAS RESOLVED, AND WHICH CONSTANTS ARE DE-RISKED
=====================================================================
The two judge panels disagreed. Panel 1 saw only SS-LGC and declared it winner by default while flagging its K=15 Viterbi as a schedule risk, its core noise constant as uncited, and multiband loudness as an unmitigated gap on the default distribution path. Panel 2 saw both and ranked LEP-QIM 30 over SS-LGC 23.5 on every axis, decisively on build risk.

RESOLUTION, NOT AVERAGE: LEP-QIM is the core. It wins because (a) its embed is closed-form — scaling A-cell bins by e^(d/4) and B-cell bins by e^(-d/4) shifts every pair difference by exactly d, hit in one shot, no nonlinear fixed point, versus SS-LGC's asserted-convergence two-pass pre-cancellation; (b) its gain invariance is exact and unit-testable on day one rather than statistical; (c) its 256-state K=9 Viterbi retires panel 1's single largest schedule risk; (d) its band ceiling sits below every practical SBR crossover; (e) it is ~10x cheaper to detect, which is what makes a 200-track x 20-condition bench runnable in hours instead of days.

THREE GRAFTS FROM SS-LGC, each because it fixes a named LEP-QIM gap:
  G1. The explicit false-positive count, and CRC width derived from it (LEP-QIM asserted ~1e4 alignments without counting). Recounted from scratch below for LEP-QIM's own search space — NOT inherited.
  G2. The operational definition of result.match (LEP-QIM had none, and the brief fixes `result.match` as public API).
  G3. Per-rate-hypothesis band-edge rescaling over a widened rho grid, with the coarse/fine step derived from coherent-integration span. This fixes LEP-QIM's worst named defect: total, non-graceful failure outside a +-1% rate window.

REJECTED FROM SS-LGC: the 2-7 kHz band (crosses SBR crossovers), PM1 masking (LEP-QIM's antisymmetric +-0.43 dB is milder than PM1's 1.0 dB cap, and PM1 has no temporal masking so it buys pre-echo), K=15 r=1/6, and Profile A entirely (section 12).

PROVENANCE OF CONSTANTS — read before trusting any number here.
  AUDIOWMARK-ANCHORED (de-risked by a shipped C++ implementation; an implementer can diff behavior against a working system): frame N=1024, hop=512, frames_per_bit=2, band bins 20..100, Odenwalder K=9 rate-1/3 generators 557/663/711 octal, sync carried on every 4th chip.
  SET HERE, NOT DE-RISKED (bench-measured or derived in this document, and every one is a bench row): QIM step DELTA, dither construction, the 32-slot preamble and its PN, the pilot pattern, the guard mask, the trim depth, the interleaver, the rho grid, tau_sync, the payload layout, CRC-32C.
  Do not describe the second list as validated by audiowmark. Panel 1 modeled this disclosure correctly for its own truncation; this spec matches it.

=====================================================================
1. REFERENCE RATE AND RATE INDEPENDENCE
=====================================================================
All numbers below are at fs = 44100 Hz. Every frequency constant is specified in Hz; bin indices are DERIVED as round(f * N / fs), never hardcoded. Supported fs: 32000 <= fs <= 96000. Below 32000 the band top is clipped and the mark is refused with `fs_too_low`. Above 96000 the input is decimated to 48000 for analysis.
  N = 1024 for fs <= 52000; N = 2048 for fs > 52000. Hop H = N/2.
  At 44100: frame rate 86.1328/s, bin width 43.0664 Hz, frame period 11.610 ms.

=====================================================================
2. THE STATISTIC
=====================================================================
2.1 TRANSFORM. Stereo is reduced to mid M = (L+R)/2 for ANALYSIS; the computed per-bin gains are applied identically to L and R so the mark survives mono downmix. Window: square-root Hann, length N, analysis and synthesis. sqrt-Hann squared is Hann, which is COLA at 50% overlap, so weighted overlap-add reconstruction is exact.

2.2 CELLS. A cell is 2 adjacent bins (86.13 Hz at 44.1 kHz). Band = bins 20..99 inclusive = 80 bins = 40 cells = 20 pairs. In Hz: 861.3 Hz to 4306.6 Hz.
  Pair k (k = 0..19) = (cell 2k, cell 2k+1) = bins (20+4k .. 21+4k) as A, (22+4k .. 23+4k) as B. Pair span 172.3 Hz.

2.3 THE GUARD MASK — the multiband-compression mitigation. Panel 1's sharpest unaddressed criticism of SS-LGC was that multiband loudness processing sits on the default distribution path and attacks the exact statistic the mark lives in. LEP-QIM partly answers this by construction: an LTI response smooth over 86 Hz cancels to first order between a pair's two cells. The residual hazard is a compressor crossover falling BETWEEN a pair's two cells, where the two cells get different gains. That is placement, not trim, so it is fixed by placement:
  GUARD_HZ = {1000, 1200, 2000, 2500, 3000, 3500}  (crossovers of common 3/4/5-band mastering and broadcast processors)
  A pair whose 172.3 Hz span contains any guard frequency is GUARDED and excluded from the statistic. At 44.1 kHz this guards k = 0, 1, 6, 9, 12, 15, leaving 14 ACTIVE pairs. The mask is computed from Hz at load time, so it moves correctly with fs.

2.4 ENERGY AND DIFFERENCE. For active pair k in frame t:
  E_A[t,k] = sum of |X[t,b]|^2 over the two A bins; E_B likewise.
  D[t,k] = ln(E_A[t,k] + 1e-20) - ln(E_B[t,k] + 1e-20)

2.5 THE SLOT STATISTIC. A slot is frames_per_bit = 2 consecutive frames. Over the 2 x 14 = 28 values of D in a slot, sort, drop the 2 highest and the 2 lowest, and take the mean of the remaining 24:
  d[s] = trimmedMean(D over slot s, trim = 2 each tail)
The trim absorbs a few modal notches and any single pair the guard mask missed. It does NOT absorb the density of notches a real room produces (section 12).

2.6 WHY THIS SURVIVES GAIN AND SMOOTH EQ, EXACTLY. A scalar gain g multiplies every bin, adding ln(g^2) to both ln(E_A) and ln(E_B), so it cancels in D with no memory and no error propagation. This is why LEP-QIM has no normalizer to be corrupted, unlike Rational Dither Modulation, whose normalizer is itself damaged by reverberation and noise. Any LTI response smooth over 86 Hz cancels to first order for the same reason. Both properties are deterministic unit tests, written before the embedder (build unit W1).

=====================================================================
3. EMBEDDING
=====================================================================
3.1 DITHERED QIM. DELTA = 0.8 nepers (see 3.3). Per slot s carrying coded bit b in {0,1}:
  dither[s] = (b * DELTA / 2) + u[s], where u[s] is a key-derived uniform offset in [0, DELTA) from the keystream (section 7). u[s] is what makes the lattice keyed; without it the mark is readable by anyone who knows DELTA.
  target[s] = DELTA * round((d[s] - dither[s]) / DELTA) + dither[s]
  delta[s]  = target[s] - d[s]        (|delta[s]| <= DELTA/2 by construction)

3.2 APPLICATION, CLOSED FORM. For every frame in the slot and every ACTIVE pair k:
  X'[t,b] = X[t,b] * exp(+delta[s]/4)  for b in the A cell
  X'[t,b] = X[t,b] * exp(-delta[s]/4)  for b in the B cell
  Guarded pairs and all bins outside the band are untouched. Phase is never touched.
  Amplitude gain e^(delta/4) scales energy by e^(delta/2), so ln E_A rises delta/2 and ln E_B falls delta/2: D shifts by exactly delta, for every pair, with no iteration. The trimmed mean of a set all of whose members shift by delta also shifts by exactly delta, which is why the trim does not break the closed form.

3.3 DELTA AND ITS CALIBRATION. DELTA = 0.8 nepers gives typical |delta| = DELTA/4 = 0.2 nepers, a per-bin amplitude change of e^0.05 = +-0.43 dB, and worst case |delta| = DELTA/2 = 0.4 nepers, e^0.1 = +-0.87 dB. The change is antisymmetric between A and B cells, so pair energy is preserved to first order and there is no band tilt or level shift for the ear to latch onto.
  DELTA IS NOT A FREE CHOICE AT SHIP TIME. Panel 1's sharpest criticism of SS-LGC — that every downstream number hung on one uncited noise constant — applies verbatim here: LEP-QIM's residual noise sigma_d on d is equally unmeasured. Build unit C0 measures sigma_d over the corpus and sets DELTA = 4.0 * sigma_d, clamped to [0.5, 1.2] nepers. 0.8 is the pre-registered prediction (i.e. sigma_d ~= 0.2 nepers) and the default the code ships with if C0 confirms it. C0 runs BEFORE the embedder is written.

3.4 PUNCTURING. A slot is punctured (left unmodified, its coded bit transmitted as an erasure) when either:
  - total in-band energy across the slot is below -70 dBFS, or
  - fewer than 10 active pairs have E_A and E_B both above 1e-12 (a spectral hole).
  The slot index still advances so the mapping stays blind-reproducible. The detector never needs to know a slot was punctured: the reliability weight (4.4) demotes it automatically.

3.5 TWO-PASS CLOSURE. Overlap-add of the modified frames perturbs neighbouring frames slightly, because a sample belongs to two frames. Run the embed, resynthesize, recompute d on the resynthesized signal, and apply one corrective pass with the same closed form. Two passes. Unlike SS-LGC's pre-cancellation loop, this converges by construction rather than by assertion, because the correction target is a fixed lattice point and not a function of the host. Residual after pass 2 is bounded by the OLA leakage, measured in C0 and asserted under DELTA/20 in a unit test.

3.6 SAMPLE-EXACT REVERSIBILITY IS NOT CLAIMED. embed() writes a new file. The hard binding in the manifest is computed over the MARKED audio, after embedding. Order is fixed in the SDK: embed, then hash, then sign. Marking a signed file invalidates its hard binding, and `sign()` refuses `{ mark: ..., }` against an already-signed input with `mark_after_sign`.

=====================================================================
4. DETECTION
=====================================================================
Fully blind: audio PCM, its sample rate, and the profile key. No original, no sidecar, no length hint.

4.1 OVERSAMPLED ANALYSIS. STFT with the same N and window but hop H_d = 64 (N/16). Detector frame rate 689.06/s at 44.1 kHz. Striding this by 8 recovers the embed hop-512 grid at 8 distinct sub-phases, so frame offset needs no separate search. Residual straddle is bounded at 0.73 ms in an 11.61 ms frame. Forward transform only, no inverse.

4.2 RATE HYPOTHESES — GRAFT G3. For a rate factor rho, frequencies scale by rho. Rather than tolerating the shift, the cell bin edges are recomputed per hypothesis as round(f_edge * rho * N / fs) and the cell energies re-summed FROM THE SAME, ALREADY-COMPUTED FFT MAGNITUDES. The FFT is computed once for the file and never recomputed per hypothesis; per-hypothesis cost is 80 adds per frame plus one linear interpolation along t. This is the graft that turns LEP-QIM's +-1% cliff into a +-6% supported range.
  COARSE GRID, step derived from the preamble's coherent span: the preamble is 32 slots = 64 embed frames = 0.743 s. Residual slip under a quarter frame over that span requires |rho error| < 0.25/64 = 3.9e-3, so step = 7.8e-3. Over [0.94, 1.06] that is 16 hypotheses. Range covers 44.1/48 kHz mismatch, film pull-up/pull-down (+-0.1% and +-4.096%), and free-running DAC/ADC clock offset.
  FINE GRID, step derived from the block's span: the block is 768 embed frames; cumulative slip under a quarter frame requires 0.25/768 = 3.26e-4. Over the +-3.9e-3 coarse residual that is 24 fine hypotheses.

4.3 NORMALIZATION. For each rho: compute D[t,k] on the rescaled edges, form d[t] as in 2.5, then subtract a per-slot sliding median of d over +-64 embed frames (1.49 s) and divide by 1.4826 x the sliding MAD. Call it Z[t]. The sliding median is what removes any residual slow spectral tilt the pair differencing did not already cancel; the MAD gives the per-slot scale the QIM decision needs.

4.4 RELIABILITY WEIGHT. w[t] = 1 / (1 + (MAD_local[t] / MAD_global)^2). This is matched-filter weighting; it also demotes punctured and silent regions with no side information.

4.5 PREAMBLE ACQUISITION. Cross-correlate the 32-slot keyed bipolar preamble along t, FFT-accelerated along the time axis. Normalized peak statistic against tau_sync, set at 4.27 robust sigma, i.e. per-hypothesis P_fa = 1e-5. Retain all (offset, rho) peaks above tau_sync.

4.6 FINE RATE REFINEMENT. Per surviving candidate, score the 24 fine rho by the PILOT energy metric: sum over the 96 pilot slots of w[t] * pilot_polarity[t] * Z[t]. This is payload-blind, so no Viterbi runs until a rate is chosen. Top 2 fine rates per candidate go forward. Unlike SS-LGC, where the analogous gate is load-bearing for tractability and its failure mode is silent recall loss, here it is a 5x optimization on an already-cheap decoder: with the gate removed the detector still finishes (24 x 26 = 624 decodes x 49k ACS = 31M ACS, well under a second), so the gate is checked against a no-gate reference run in the bench rather than trusted.

4.7 SOFT DEMODULATION. For slot s, the QIM soft metric is the signed distance to the nearer of the two dithered lattices:
  q[s]   = ((Z[s] - u[s]) mod DELTA_z) / DELTA_z, in [0,1), where DELTA_z is DELTA expressed in the normalized units of Z
  llr[s] = w[s] * (0.5 - |q[s] - 0.5|) * 2 * sign(q[s] - 0.5)
  This is a real-valued LLR proxy. Hard-deciding here throws away ~2 dB; soft-decision Viterbi is required, not optional.

4.8 DE-INTERLEAVE AND DECODE. De-interleave (section 6), then soft Viterbi over the K=9 rate-1/3 code: 256 states, 96 trellis steps, 2 branches, 3 LLR adds per branch metric = 49,152 ACS per decode. Milliseconds in scalar JS. Output 96 bits, strip 8 tail bits, giving 88.

4.9 CRC AND ACCEPT. CRC-32C (Castagnoli, 0x1EDC6F41) over the leading 56 bits versus the trailing 32. Fail: discard the candidate silently. Pass: accept the 56-bit payload.

4.10 CONFIDENCE CLASSES, and the rule that binds them to proof level.
  strong: two or more NON-OVERLAPPING blocks CRC-pass with identical payloads. confidence >= 0.95.
  single: exactly one block CRC-passes. confidence in [0.6, 0.9].
  none:   no block passes.
  A `single` detection MUST NOT be promoted above proof level `inferred`. A soft binding recovered once is not directly_observed evidence of anything. Wiring it to `verified` would violate the prior-art invariant that a claim never carries a proof level higher than what was observed.

4.11 BLOCK-PAIR RATE TRACKING. Blocks repeat back to back, so two consecutive preamble detections give a direct rate estimate: measured inter-preamble frame spacing divided by 832 (the block in embed frames). Accurate to peak localization (~0.25 frame) over 832 frames, i.e. 3.0e-4. This tracks slow oscillator drift across a long file instead of assuming one global rate.

=====================================================================
5. PAYLOAD — 56 BITS
=====================================================================
Three source documents disagreed: GenoTrace said 96 bits, the SDK package spec said 128, SS-LGC said 56. RESOLVED TO 56, because the rate budget (section 8) does not support more and because a payload width must be derived, not chosen.

  bits  0..3    version   (4)   currently 1
  bits  4..7    namespace (4)   registry namespace selector; 0 = public
  bits  8..55   locator   (48)  leading 48 bits of SHA-256("audio-provenance-locator-v1" || 0x00 || signer Ed25519 public key || locator_salt)
  bits 56..87   CRC-32C   (32)  over bits 0..55

LOCATOR IS A COMMITMENT, NOT A NAME — the most important semantic in the payload, kept, with the direction of the commitment reversed. The MANIFEST commits to the locator, not the other way round: `locator_salt` is 16 CSPRNG bytes in a REQUIRED, SIGNED root field of the Audio Provenance v1 record, and the locator is the leading 48 bits of SHA-256 over the fixed-length 68-byte preimage `"audio-provenance-locator-v1" || 0x00 || signer_ed25519_public_key[32] || locator_salt[16]`. After fetch, GenoTrace re-derives that value from the RECEIVED bytes, using the received document's OWN declared `portable_signature.public_key_hex` and its OWN `locator_salt` — never the backend's index key and never a trust anchor — and compares it against the payload's locator; mismatch is `untrusted / locator_mismatch`, terminal, no descent down the ladder. A record served under a locator it does not itself derive is refused exactly as before.

WHY IT IS NOT A DIGEST OF THE MANIFEST. It was, and that made the mark rung structurally dead. A Audio Provenance record MUST carry a hard binding, the binding covers the MARKED audio, and the locator has to be embedded before the audio is marked, so a manifest-derived locator would have to be a 48-bit fixed point of its own embedding. Nothing in the preimage above is derived from the audio, the manifest or the signature, so the order resolves: allocate the salt, derive the locator, embed the mark, sign the record over the marked audio with that salt inside it, publish under the derived locator.

WHY THE PUBLIC KEY IS IN THE PREIMAGE. Without it, a squatter copies a victim's published salt into their own signed manifest and holds the victim's locator at zero cost. With it, doing so costs a ~2^48 search for a (key, salt) pair — the same work factor the old digest form charged, no more and no less. What bounds the damage is not the work factor but the outcome: two records under one locator are two survivors, which is `ambiguous_binding` → `untrusted`, never a misattribution, because attribution still requires signature admission, a trust anchor and a hard binding, none of which the locator touches.

NOT RANDOM, AND NOT CONTENT-DERIVED. A free-form random locator lets anyone declare any locator for free. A content-derived one is a confirmation oracle over unmarked masters and collides with itself whenever one master is signed twice.

48 BITS IS AN INDEX, NOT AN IDENTITY. Stated correctly, because the SS-LGC source states it wrongly: at 2^24 registered works the EXPECTED NUMBER of 48-bit collisions is n^2 / 2^49 = 2^48 / 2^49 = 0.5, i.e. of order one, not "under 1e-9" (that figure conflated a per-pair probability with an expected count). This is harmless only because collisions are HANDLED, not assumed away: `lookupByGenoMark` returns ALL matching refs, each is disambiguated by full-digest re-derivation plus soft-binding association, and two survivors yield `ambiguous_binding` rather than a coin flip.

=====================================================================
6. CHANNEL CODING
=====================================================================
INNER: convolutional, K=9, rate 1/3, Odenwalder generators 557, 663, 711 octal. Soft-decision Viterbi, zero-tail terminated with 8 tail bits.
  56 payload + 32 CRC = 88 message bits; + 8 tail = 96 trellis steps; x 3 = 288 coded bits.
  K=9 rather than SS-LGC's K=15 is the single decision that retires panel 1's largest schedule risk: 256 states versus 16,384, a 64x cheaper decode, at a cost the rate budget absorbs.

OUTER: CRC-32C, width derived in section 9, not chosen by convention.

INTERLEAVER: block interleaver over the 288 coded bits, written row-wise into 24 x 12 and read column-wise, so adjacent coded bits land 12 slots apart. At 43.07 slots/s that is 279 ms, longer than an MP3 granule (26 ms) and longer than a typical short dropout. It is NOT longer than a 50-200 ms reverb tail plus its late field, which is deliberate: interleaving is not the tool for reverb (section 12).

=====================================================================
7. KEYING
=====================================================================
Two keyed streams, both from ChaCha20 seeded by HKDF-SHA256(profile_key, "genomark-lepqim-v1" || namespace || floor(blockIndex / 256)):
  - u[s], the uniform dither offset in [0, DELTA), one per slot
  - the preamble PN and the pilot polarities, one bipolar value per preamble/pilot slot
For namespace 0 (public) the profile_key is a PUBLISHED CONSTANT. This is a deliberate, disclosed trade: the public mark is removable by an informed adversary who can estimate and subtract it. The public namespace provides provenance RECOVERY for cooperative and accidental cases; it is not tamper resistance. Keyed namespaces resist a casual remover and remain vulnerable to estimation and collusion attacks given many works under one key.

=====================================================================
8. BLOCK LAYOUT AND RATE BUDGET
=====================================================================
  slot = frames_per_bit(2) x 1 = 2 embed frames -> 43.066 slots/s at 44.1 kHz
  BLOCK = 32 preamble slots + 384 body slots = 416 slots
    body = 288 data slots + 96 pilot slots, pilots at every 4th body position (audiowmark's every-4th-chip sync)
  BLOCK DURATION = 416 / 43.066 = 9.66 s   (832 embed frames)
  Blocks repeat back to back for the whole file.

  Typical decode (file starts on a block boundary): 9.66 s
  Guaranteed decode, arbitrary crop start:        19.32 s   (two blocks)
  `strong` class (two agreeing blocks):           19.32 s typical, 28.98 s guaranteed
  Net useful throughput: 56 bits / 9.66 s = 5.80 payload bits/s

  CALIBRATION FALLBACK LADDER, specified now so a C0 failure is a config change and not a redesign:
    frames_per_bit 2 -> block 9.66 s, guaranteed 19.32 s   (default)
    frames_per_bit 3 -> block 14.49 s, guaranteed 28.98 s
    frames_per_bit 4 -> block 19.32 s, guaranteed 38.64 s
  Each step buys 1.76 dB of per-slot SNR. DELTA is NOT the fallback knob; raising it trades fidelity, and fidelity is the constraint C0 holds fixed.

=====================================================================
9. FALSE-POSITIVE BUDGET — GRAFT G1, RECOUNTED FOR THIS SEARCH SPACE
=====================================================================
CRC-32C is NOT inherited from SS-LGC by authority. SS-LGC derived 32 bits from ITS search space; LEP-QIM's is structurally different (8 frame phases by striding, block phase by circular correlation, single profile), so the count is redone. On a 4-minute track at 44.1 kHz:

  detector positions: 240 s x 689.06/s              = 165,375
  x coarse rho hypotheses                           x 16
  x profiles (one; no Profile A)                    x 1
  = preamble hypotheses                             = 2.646e6

That is essentially identical to SS-LGC's 2.65e6 — the single-profile saving is cancelled by the finer detector hop. So the answer is the same but now it is EARNED:
  at tau_sync P_fa = 1e-5 -> ~26 surviving candidates per clean track
  x 24 fine rho = 624 fine scores; top 2 per candidate -> 52 Viterbi+CRC (624 in the worst accounting)
  CRC-32C: 624 x 2^-32 = 1.45e-7 false accepts per clean track (1 in 6.9 million)
  CRC-24:  624 x 2^-24 = 3.72e-5 per clean track (1 in 27,000)
  CRC-16:  624 x 2^-16 = 9.5e-3 per clean track (1 in 105)

DECISION: CRC-32C. At 24 bits, a registry serving 10^6 verifications produces ~37 false identity attributions; that is product-destroying, and the 8 bits it would buy back for the locator are not worth it. The width must not be reduced to buy payload space.
Even degenerately, with the preamble gate removed entirely and all 2.646e6 hypotheses reaching CRC: 2.646e6 x 2^-32 = 6.2e-4 per track.

=====================================================================
10. INAUDIBILITY
=====================================================================
The modification is a smooth antisymmetric gain on bins that ALREADY EXIST, in 861-4307 Hz. It introduces no new spectral components: it reweights partials already present, which is perceptually far more forgiving than additive spread spectrum at equal power. Because A and B cells move in opposite directions by the same log amount, pair energy is preserved to first order, so there is no band tilt and no level shift.
  Typical per-bin change +-0.43 dB, worst case +-0.87 dB.
  PREDICTED PEAQ basic ODG (ITU-R BS.1387): median around -0.5, tail to -1.2 on sparse tonal content. These are model predictions, not measurements; nothing may be published as measured until bench unit B4 runs.
  NO TRANSPARENCY CLAIM IS MADE. The honest expectation is "perceptible but not annoying" on the worst content class, not "inaudible". `capabilities().measuredTransparency` is null until B4 fills it, and the package will not describe itself as inaudible on an unmeasured claim.
  A per-track PEAQ guard is part of the embedder, not a nice-to-have: if measured ODG falls below -1.2, DELTA is reduced in 0.05 steps and the track re-embedded, and the resulting reduction in robustness is RECORDED IN THE MANIFEST rather than hidden. Degradation is never silent.
  Worst content class: sparse tonal material (solo piano, harpsichord, clean electric guitar, unaccompanied voice), where few pairs carry energy and the puncture rule fires. Note this is the same class that is weakest for robustness, so fidelity and recovery degrade together rather than trading off.

=====================================================================
11. CHANNEL SURVIVAL — PREDICTIONS, EVERY ONE A BENCH ROW
=====================================================================
MP3 128 kbps CBR and above: SURVIVES with margin. 861-4307 Hz is fully waveform-coded (LAME lowpass ~16 kHz, no parametric band replication in MP3). Cell energies over 2 bins are coarser than the quantizer noise, and pair differencing absorbs the codec's slow spectral shaping. Predicted >99% exact 56-bit decode within 20 s.
MP3 64-96 kbps: ~95%. LAME's lowpass drops but stays well above 4.3 kHz; joint-stereo intensity coding is the risk to the mid channel.
AAC-LC 96 kbps and above: SURVIVES, same reasoning. Predicted >99%.
HE-AAC v1 48-64 kbps: SURVIVES. This is where the band ceiling earns its keep — typical SBR crossover is 8-10 kHz at 64 kbps and 6-7 kHz at 48 kbps, both entirely ABOVE 4.3 kHz, so the mark is in the waveform-coded region. SS-LGC's 2-7 kHz band would lose its top groups here. Predicted ~95%.
HE-AAC v2 / xHE-AAC at 32 kbps: DEGRADED BUT NOT DESTROYED, unlike SS-LGC. Crossover falls to ~4.5-5.5 kHz, at or just above the band top, so the mark survives the SBR cut; the real damage is Parametric Stereo collapsing the mid channel. Predicted ~70%, and this is the row most likely to surprise in either direction.
Opus 64 kbps and above: SURVIVES (CELT is waveform-coded in this band). 32 kbps: degraded.
Resampling 44.1<->48, 44.1->22.05, 44.1->16: SURVIVES BY CONSTRUCTION. Band edges are in Hz, bins derived from actual fs. Even 16 kHz keeps Nyquist at 8 kHz, above the 4.3 kHz top.
Speed change within +-6%: SURVIVES via the rho grid (graft G3). Beyond +-6%: NOT SEARCHED, will not decode.
Gain, dither, requantize to 8 bit, hard clip, DC offset: SURVIVES. Gain invariance is exact.
Loudness normalization to -14 LUFS, broadcast multiband chains: PARTIALLY MITIGATED by 86 Hz pair spacing plus the guard mask; predicted ~90%. This is the honest residual and the highest-value bench row.
LOWPASS — THE TRADEOFF STATED IN BOTH DIRECTIONS. The 4.3 kHz ceiling is why this design beats SS-LGC on SBR. It is also why it is WORSE under lowpass: a 4 kHz lowpass cost SS-LGC 6 of 12 groups but costs LEP-QIM nearly the entire band, and telephony-band processing at 3.4 kHz is FATAL. Predicted <20% at a 4 kHz lowpass and 0% through a telephony codec. This is a real, named regression versus the losing candidate and must not be omitted from the docs.
Pitch-preserving time-stretch, independent pitch shift: EXPECTED FAILURE. Breaks the single-rho model; time and frequency axes move by different factors. A 2-D search would cost a 16x false-positive multiplier and a wider CRC; deliberately unimplemented.
Mid/side manipulation: the mark lives in the mid channel; a widener attenuating M weakens it proportionally, and an S-only extraction removes it entirely.
ACOUSTIC RE-RECORDING: see section 12.

=====================================================================
12. ACOUSTIC RE-RECORDING — A DECLARED NON-GOAL
=====================================================================
The brief named AR as a requirement. Both judge panels scored it 2 and 3.5 out of 10 and both called the failure STRUCTURAL. This spec accepts that verdict and declares AR out of scope for v1, rather than shipping a mode that will not work.

WHY ECC CANNOT RESCUE IT. Room reverberation is a dense multi-tap convolution smearing energy over 50-200 ms. On log-spectral-energy differences it acts as a SYSTEMATIC, CONTENT-CORRELATED BIAS, not as independent additive noise: the same room and the same content produce the same distortion in every block. Repetition, interleaving and block combining all attack independent noise, so none of them touch it. This is why the losing candidate's own analysis of block combining ("the room and transducer response are time-invariant, so the same cells are corrupted in every block and errors are correlated, not averaging") was called the sharpest argument in the whole candidate set — and it argues against both designs equally.
The published comparables agree. The closest apples-to-apples measurement, a frequency-domain spread-spectrum scheme at ~25 dB SNR, scores 56% bit accuracy at 5 cm — chance — while acing every electronic channel. The strongest published non-neural result, a patchwork method explicitly engineered for recapture, reaches only 66% at 1 m. Below ~66% accuracy the BSC capacity falls under any usable code rate, so no redundancy at any length decodes.

CONSEQUENCES, ENFORCED IN THE TYPE SYSTEM:
  - There is NO Profile A. The SS-LGC acoustic profile is dropped: it was audible (predicted ODG -1.5 to -2.5), needed 37.5 s of continuous near-field capture, and had NEGATIVE capacity margin at 1 m.
  - `capabilities().acousticRerecording` returns the literal `"unsupported"`, NOT `null`. Null reads as "not yet measured" and invites hope.
  - Marketing copy in the genomark package description must not say "built for acoustic re-recording", and must not sell the rho sweep or median normalization as room-response handling. They are clock-drift and spectral-tilt handling.
  - verify() on a microphone recording returns `not_found` BY DESIGN, not by defect, and the docs say so.

THE ONLY PATH THAT WORKS, recorded so the interface does not have to change later: a trained encoder/decoder with a differentiable distortion layer containing measured room impulse responses, band-pass and Gaussian noise (the DeAR/Timbre family; DeAR reports 99.18/98.55/93.40/92.68% at 5/20/50/100 cm at ~8.8 bits/s). That is out of scope for a pure-TypeScript, no-native-dependency SDK. The 4-bit `version` field in the payload and the namespace selector exist so such a detector can be added later behind the SAME locator format and the SAME registry key, without changing verify()'s contract.

=====================================================================
13. TYPESCRIPT IMPLEMENTATION NOTES
=====================================================================
Pure TypeScript, typed arrays only, no native addon, no WASM. Surface: one iterative radix-2 complex FFT of length 512 with a real-input split step for the 1024-point real spectrum, precomputed twiddle/bit-reversal tables, a sqrt-Hann table, a sliding median and MAD over a 129-frame ring buffer, a 256-state soft Viterbi, a CRC-32C table, ChaCha20. Nothing needs 64-bit integers or SIMD.
  Embed, 4-min stereo: 20,672 frames x (one forward + one inverse real-1024) x 2 passes. ~5-10 s, RTF ~0.03x.
  Detect, 4-min: 165,375 forward transforms (~2.1 GFLOP) plus 16 x 165,375 x 80 adds for cell re-summation plus FFT-accelerated preamble correlation plus 52 x 49k ACS. ~5 s total, RTF ~0.02x. This is what makes a 200-track x 20-condition bench ~5 hours instead of days, and it is a primary reason this candidate won.
  MEMORY IS THE REAL CONSTRAINT, not FLOPs: the hop-64 magnitude array for a 4-min track is 165,375 x 513 Float32 = 340 MB. It MUST be streamed in 10-second chunks with 1.5 s of overlap to carry the 129-frame sliding median across chunk boundaries. The ring-buffer and overlap bookkeeping is where the bugs will be, not in the FFT; unit W3 tests chunk-boundary equivalence against a whole-file reference before anything else uses it.
  SCOPE BOUNDARY. The DSP core takes { pcm: Float32Array, sampleRate: number } and nothing else. WAV parsing is pure TS and ships in-package. MP3/AAC/FLAC/Ogg decode is NOT implementable in pure TS at acceptable quality and is delegated to an explicit DecoderPort (an ffmpeg child process on Node, decodeAudioData in a browser). Hiding a native ffmpeg dependency behind a "pure TypeScript" claim would be dishonest; the adapter is explicit so the core stays dependency-free and testable.
