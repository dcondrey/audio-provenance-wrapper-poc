WATERMARK-N v1 — LEARNED ACOUSTIC MARK (PROPOSED, NOT APPROVED)
Algorithm id (reserved, not yet allocated): `apw-watermark-neural-v1`. Statistic: a learned per-bin
multiplicative log-gain mask on the 48 kHz STFT magnitude, read back by a position-agnostic
convolutional decoder with a separate per-frame presence head.

STATUS: THIS IS A GATED FEASIBILITY PROGRAM, NOT A COMMITTED FEATURE. Nothing in this document
authorises a change to `capabilities().acousticRerecording`, to `HONEST_LIMITS.md`, or to any public
claim. Those change only when the Stage 2 gate in section 12 passes on measured physical captures,
and if it does not pass, the correct outcome is to ship nothing and leave the acoustic path declared
unsupported.

=====================================================================
0. RECOMMENDATION, AND THE DECISIVE REASON
=====================================================================
BUILD IT, AS A GATED EXPERIMENT WHOSE FIRST TWO WEEKS ARE A PHYSICAL CAPTURE CAMPAIGN AND NOT A
TRAINING RUN. Lead with the presence tier; treat the locator tier as a stretch goal.

The decisive reason is narrow and should not be overstated. `HONEST_LIMITS.md` argues that room
reverberation is a time-invariant, content-correlated bias on the exact statistic LEP-QIM modulates,
so errors correlate across blocks instead of averaging. That argument is correct about a FIXED,
hand-chosen statistic and does not carry to a learned encoder, which is not committed to a statistic
the room happens to corrupt. The one licence-clean data point that speaks directly to this is
RAW-Bench's retrain result: putting reverberation inside the training loop lifted a spectral-domain
model's full-message accuracy under strict simulated reverb from 0.45 to 0.95, while the same
treatment moved a waveform-domain model only from 0.22 to 0.41. Reverb robustness is trainable, and
it is trainable much better in the spectral domain.

That is enough to justify measuring. It is not enough to justify a claim, for three reasons that the
adversarial verification established and that this spec treats as binding:

  (a) EVERY physical speaker-to-microphone number in the published literature is an ORACLE
      best-of-search maximum computed with knowledge of the true bits. DeAR shifts the capture over a
      113 ms range and keeps the best accuracy; DeepAWR's `search_do_extract()` sweeps 6000 sample
      offsets, scores each against the known payload, keeps the maximum, breaks early on a perfect
      hit, and computes `verify_crc()` only to discard the result. No commercially-licensable method
      publishes a BLIND, threshold- or CRC-gated detection rate on a physical path at any distance.
      The number this program exists to produce does not exist anywhere.
  (b) Mean bit accuracy is not a detector. DeAR's headline 92.68% at 1 m, taken at face value and
      with the optimistic independence assumption the room violates, is P(all 100 bits) = 5.0e-4.
      Roughly one capture in two thousand yields the message verbatim without ECC.
  (c) The best simulated-replay result in the literature (AWARE, BER 0.024 at 16 bps) still has
      FNR 0.11 — one watermarked clip in nine missed outright — and rises to FNR 0.58 at 20 bps. It
      is also speech-only at 16 kHz, and its "band-pass filtering (50 Hz - 8 kHz) to reflect
      microphone and loudspeaker characteristics" is a no-op at 16 kHz because Nyquist is 8 kHz. It
      models no transducer roll-off at all.

So the program is ordered to produce the missing measurement in week two rather than month six.
Stage 0 buys three consumer speakers, three microphones, and two weeks, and produces (i) a measured
room-impulse-response corpus Audio Provenance owns outright, (ii) a physical marked/captured corpus, and
(iii) the first blind physical numbers for any licence-clean watermark. If Stage 0's baselines and
Stage 1's simulated gate fail, the program stops having spent under six weeks.

INDEPENDENT OF THIS PROGRAM, AND IMMEDIATELY: the product website's claim that the mark survives
"re-recording" must be withdrawn today. Nothing in the shipped system, in this spec, or in the
published literature supports it, and `HONEST_LIMITS.md`'s `acousticRerecording: "unsupported"` is
the truthful position until section 12's gate says otherwise. A `verify()` that returns `not_found`
on most real re-recordings while the website promises otherwise is worse than one that honestly
declares the path unsupported.

WHAT IS MOST LIKELY TO SHIP, STATED UP FRONT SO THE ROADMAP IS NOT BUILT ON THE OPTIMISTIC BRANCH:
a zero-bit PRESENCE tier ("this audio carries a Audio Provenance mark") at approximately 1 m over 10-second
windows, which contributes a Finding and never an identity. The 56-bit LOCATOR tier at 30 s is the
stretch goal, and even when it lands it resolves to a candidate SET, not an identity (section 5.3).

=====================================================================
1. WHAT WE TAKE FROM WHOM, AND WHY — DISAGREEMENTS RESOLVED, NOT AVERAGED
=====================================================================
The survey and the adversarial verification disagree on four points. Each is resolved in favour of
the verification, because in each case the verification read primary artifacts (repository trees,
LICENSE bytes, source code, release tarballs, paper text) and the survey read summaries.

DeepAWR — REFERENCE DOCUMENT, NOT A DEPENDENCY. The survey ranked it first on the strength of MIT
code plus MIT weights committed in-tree plus 48 kHz music training. The declared licence is real, but
the chain of title is defective: 26% / 43% / 39% of substantive lines in `train_audio.py`,
`extract.py` and `embed.py` are verbatim from DeAR, which is an unlicensed Google Drive drop, and the
author sets do not overlap so there is no implied-consent route. The MIT grant cannot convey rights
in code the grantor does not own, and the shipped `.dat` weights were produced by running that code.
DO NOT VENDOR DeepAWR SOURCE OR WEIGHTS. Its published architecture description and DeAR's published
distortion-layer recipe are facts in papers, usable in a clean-room reimplementation. Separately, its
only shipped acoustic evidence is six WAV files per condition folder, oracle-gated, with speaker,
microphone, room, RT60 and SPL documented nowhere, and its author states in issue #1 that the model
was trained only on re-recording attacks.

SilentCipher — CODE MIT, WEIGHTS UNLICENSED. The survey's mitigation ("take the GitHub release, not
the HuggingFace mirror") mitigates nothing: the release tarball contains no LICENSE, NOTICE or
COPYING, and the repository README scopes its grant to "the code in this repository", which release
binaries are not. The HuggingFace mirror is worse, with `cardData: null` and no licence file at all.
Treat the weights as carrying no grant. This costs us nothing, because we train our own weights.
What we take is design ideas, all published: the SDR controller that guarantees a fidelity floor
without retraining, the psychoacoustic negative-message constraint, and the pseudo-differentiable
compression layers whose ablation drops MP3 accuracy to zero when removed.

AudioSeal — HARNESS DONOR, NOT ARCHITECTURE DONOR. It is the only candidate with an unambiguous
commercial grant on code AND weights (MIT since 2024-04-02, confirmed on the repository LICENSE and
the HuggingFace card), released training code (2024-06-17), and a proven MIT ONNX export. It is also
16 kHz, speech-trained, has no reverberation anywhere in its augmentation set, sits at BER 0.500 with
FNR 1.00 under AWARE's simulated replay, and its waveform-domain architecture retrained into reverb
worse than any other model tested. TAKE: the training harness, the differentiable augmentation
framework, the loudness/perceptual loss, the ONNX export path, and above all the per-sample DETECTION
HEAD, which is the piece of prior art that gives ground-truth-free localisation. DO NOT TAKE: the
architecture, the sample rate, or the weights.

AWARE — NOT A WATERMARKING SYSTEM FOR OUR PURPOSES. The survey called its weights "unresolved". They
do not exist: the repository has no checkpoint of any format, no releases, and the only `torch.load`
-adjacent line in 45 files is `torch.manual_seed(328656719)` followed by `self.apply(self._init_weights)`.
Embedding is 400 Nadam iterations of per-clip adversarial optimisation against a detector fully
determined by a published integer, so there is no key material and no real-time encoder. TAKE: the
position-agnostic bitwise readout idea (paired filter banks producing per-bit temporal activation
traces, globally averaged), which is the single most valuable idea in the survey and is the basis of
section 4.4. Note also its embedding band [1000, 4000] Hz sits inside Watermark-Q's 861-4307 Hz; we
are not using it, but the collision is the reason section 8 exists.

RAW-Bench — capacity is ~5.3 bps, not the 16 bps the survey reported, so every reverb figure cited
from it holds at roughly a third the rate claimed. Its reverb attack is RIR convolution only, with no
band-limiting and no physical capture. Its authors' own conclusion names reverb among the attacks
that "adversarial-attack training augmentations fail to overcome", and the SC* retrain that produced
0.99/0.95 used a proprietary ~1250 h corpus with training code that is still unreleased. So the one
encouraging datum in the whole survey is real, directionally strong, and NOT REPRODUCIBLE. It
justifies an experiment. It does not justify a schedule commitment.

DeAR — NOT VENDORABLE, unlicensed, confirmed by enumeration of the Drive folder rather than inferred
from silence. TAKE: its fully published analytic distortion layer, DAR(.) = GaussianNoise(BandPass(
RIRConvolution)), with band-pass 1 kHz high-pass / 4 kHz low-pass and noise SNR uniform in 20-25 dB,
and its ablation ordering (removing reverberation costs >25 points, removing noise ~23, removing
band-pass ~0.7-6.5). Those are the numbers that tell us which distortion-layer components are
load-bearing.

Timbre Watermarking — GPL-3.0, hard block, would force Audio Provenance's source open. WavMark — MIT but
invertible normalising-flow, hardest of the field to port, at chance under replay, 0.00 on both
neural codecs. Groot — generative, cannot mark pre-existing audio. IDEAW — Apache-2.0 but no weights
and the authors say the evaluated code cannot leave a company server. XAttnMark — no code at all.

=====================================================================
2. ARCHITECTURE
=====================================================================
2.1 DOMAIN AND WHY. Spectral magnitude, not waveform. Three reasons, in order of weight:
  (i) The only direct evidence that reverb robustness is trainable comes from a spectral model
      (0.45 -> 0.95) and the same treatment barely moved a waveform model (0.22 -> 0.41).
  (ii) A multiplicative log-gain mask on existing bins introduces no new spectral components, which
       is the same perceptual argument Watermark-Q makes, and it is inherently gain-invariant.
  (iii) A magnitude-only decoder is structurally indifferent to phase, and the acoustic path destroys
        phase. Anything that encodes into phase is throwing capacity into the channel's null space.

2.2 SAMPLE RATE AND THE RESIDUAL-ONLY RESAMPLE. The model runs at exactly 48 kHz. Host audio is NEVER
resampled. Instead: resample a copy of the host to 48 kHz, run the encoder, obtain the marked 48 kHz
signal, subtract to form the residual, resample the RESIDUAL back to the native rate, and add it to
the untouched host. The residual is band-limited to <= 8 kHz by construction (section 2.4), so a
polyphase 48 <-> 44.1 conversion is transparent in band, and the host's own high frequencies, dither
and sample values are bit-preserved outside the residual's support. Supported host rates: 32 kHz to
192 kHz. Below 32 kHz the mark is refused with `fs_too_low`, identical to Watermark-Q.

2.3 STFT. n_fft = 2048, hop = 512, square-root Hann, WOLA synthesis. At 48 kHz that is a 42.7 ms
window, 10.67 ms hop, 93.75 frames/s, 23.4375 Hz bin width. The long window is deliberate: RT60 in
target rooms is 0.25-0.6 s, so no window contains the tail, but a longer window reduces the fraction
of energy the tail moves across a frame boundary and gives the frequency resolution the perceptual
constraint in section 2.5 needs. The forward transform is shared between the encoder and the
perceptual loss; the inverse reuses Watermark-Q's existing WOLA code path.

2.4 BAND. Bins 9..328 inclusive = 320 bins = 210.9 Hz to 7687.5 Hz. Below 211 Hz the room's modal
region and consumer speaker roll-off make the channel unusable and the ear is most sensitive to level
change; above 7.7 kHz consumer microphone and small-speaker roll-off, capture-side codec cutoffs and
room air absorption remove it. Everything outside the band is untouched, exactly.

2.5 ENCODER E. Input: log|X| over the 320-bin band, one channel, shape (1, 320, T). Message
m in {0,1}^56 is embedded by a learned 56 x 64 table, summed, and injected twice — concatenated as a
64-channel broadcast plane at the input, and as FiLM (per-channel scale and shift) at the bottleneck.
Both, because input-only conditioning is easy for a U-Net to ignore and bottleneck-only conditioning
is easy for it to smear.

  U-Net, 4 down / 4 up stages.
    stem      Conv2d(65 -> 32, k3, p1), GroupNorm(8), GELU
    down1..4  [Conv2d(k3,s2 on frequency only, stride (2,1)), GN, GELU, Conv2d(k3), GN, GELU]
              channels 32 -> 64 -> 128 -> 256 -> 256
    bottleneck 2 x [Conv2d(k3, dilation 2 on time), GN, GELU] with FiLM from the message embedding
    up1..4    nearest-neighbour upsample on frequency x2, concat skip, Conv2d(k3), GN, GELU
    head      Conv2d(32 -> 1, k1), tanh

  DOWNSAMPLING IS ON THE FREQUENCY AXIS ONLY. Time resolution is preserved end to end at 93.75
  frames/s. This is what keeps the encoder's output a per-frame quantity and lets the presence head
  localise; a time-strided U-Net would blur the mark's temporal support.

  Output g_raw in [-1, 1], shape (1, 320, T). The applied log-gain is
    g[t,f] = g_raw[t,f] * B[t,f]
  where B is the PERCEPTUAL BUDGET (section 2.7), a per-bin, per-frame, non-negative ceiling in
  nepers computed from the cover alone. Marked magnitude is |X'| = |X| * exp(g), phase unchanged.
  This is SilentCipher's SDR-controller idea generalised to a per-bin masking budget: the encoder
  chooses direction and relative magnitude, the budget chooses absolute magnitude, and fidelity is
  therefore a property of the budget rather than a hoped-for outcome of a loss weight.

2.6 DECODER D. Input: log|Y| over the same 320-bin band from the possibly-degraded audio, one
channel, no phase, no cover, no length hint.
    stem      Conv2d(1 -> 48, k(5,7), p same), GN, GELU
    trunk     4 x [Conv2d(k3, stride (2,1)), GN, GELU, Conv2d(k3, dilation (1,2^i) on time), GN, GELU]
              channels 48 -> 96 -> 192 -> 256 -> 256, frequency 320 -> 20, time preserved
    Two heads share the trunk:
    (a) PRESENCE HEAD: Conv2d(256 -> 64, k1), GELU, Conv2d(64 -> 1, k1) over the frequency-collapsed
        trunk (mean over the 20 remaining frequency rows) -> per-frame logit p[t], T values at
        93.75 Hz. Trained with BCE against a per-frame marked/unmarked label. This is AudioSeal's
        per-sample localisation head moved to the frame grid, and it is the ground-truth-free
        synchronisation mechanism the published acoustic literature does not have.
    (b) MESSAGE HEAD: 56 paired 1x1 filter banks producing 56 pairs of per-frame activation traces
        (a_i^+[t], a_i^-[t]); each trace is masked by sigmoid(p[t]), averaged over t, and the bit
        logit is tanh((mean a_i^+ - mean a_i^-) / tau) with learned tau. This is AWARE's
        position-agnostic bitwise readout. Because the readout is a global average over the
        presence-weighted span, THERE IS NO OFFSET SEARCH. Section 4 explains why this is the
        single most important structural decision in the design.

2.7 PERCEPTUAL BUDGET B. Computed on the cover, per frame, per bin, in nepers:
    B[t,f] = clamp( kappa * 10^((M[t,f] - L[t,f]) / 20) , 0, B_max )
  where L is the cover's per-bin level in dB and M is the masking threshold from a Schroeder
  spreading function over a half-Bark partition with alpha = 0.5 tonality and an absolute-threshold
  floor anchored at 96 dB SPL for digital full scale. This is DELIBERATELY THE SAME MODEL
  `audio-provenance-bench/src/perceptual.rs` already implements, so the training objective and the bench
  measurement are the same function and a training win cannot be a measurement artifact. kappa is
  calibrated in build unit N-C0 (section 9). B_max = 0.35 nepers (+/- 3.04 dB) is a hard ceiling that
  no calibration may raise; if the mark needs more than 3 dB of per-bin swing to survive, it is
  audible and the answer is to kill the program, not to raise the ceiling.

2.8 PARAMETER COUNT AND FILE SIZE. Encoder ~3.4M parameters, decoder ~2.1M, message table 3.6k.
Total ~5.5M. Shipped as fp32 `.safetensors`: ~22 MB. With int8 quantisation of the convolution
weights only (a post-training pass, validated against the fp32 logits in bench row N-B12): ~6 MB.
The 40 MB ceiling in kill criterion K7 is the hard bound.

2.9 OP SET IS A DESIGN CONSTRAINT, NOT AN AFTERTHOUGHT. The architecture above uses exactly:
Conv2d, nearest-neighbour Upsample, GroupNorm, GELU, Linear, tanh, sigmoid, mean, concat. No
attention, no LSTM, no ConvTranspose2d, no ONNX signal-processing operators. This set is chosen so
the network ports to `candle` (section 7) without an op-coverage investigation, and so that
checkerboard artifacts from transposed convolutions are structurally impossible.

=====================================================================
3. PAYLOAD
=====================================================================
3.1 MESSAGE, 56 BITS.
  bits  0..2    version         (3)   currently 1
  bits  3..6    namespace       (4)   identical semantics and values to Watermark-Q
  bits  7..31   locator prefix  (25)  leading 25 bits of the SAME SHA-256 over the manifest's
                                      canonical signed bytes that Watermark-Q's 48-bit locator
                                      prefixes
  bits 32..55   CRC-24          (24)  poly 0x864CFB (OpenPGP), over bits 0..31

  THE LOCATOR IS A PREFIX OF Q's LOCATOR, NOT A SEPARATE IDENTIFIER. This is the whole integration
  argument in one line: same digest, same registry key, same namespace field, no new lookup path. A
  work resolvable by Q is resolvable by N to a coarser bucket, and when both marks decode the
  agreement check is free (section 8.4).

3.2 NO CONVOLUTIONAL CODE, AND A BOUNDED FLIP SEARCH RATHER THAN NONE. Watermark-Q spends a K=9
rate-1/3 code on 88 message bits. N does not, because the position-agnostic readout already
integrates every bit over the whole analysis window; temporal repetition IS the code, and it is
learned end to end rather than bolted on. Adding an outer convolutional code would consume the very
redundancy the pooling depends on.
  BUT SINGLE-SHOT HARD DECISION IS NOT ENOUGH AND MUST NOT BE LEFT TO THE IMPLEMENTER. The message
head emits soft logits. A hard decision followed by one CRC-24 check fails the whole window on a
single bit error, so at a pooled BER of even 1-2% most windows fail while the information is present.
The rule is therefore FIXED HERE, not chosen at implementation time:
    ORDERED-STATISTICS FLIP SEARCH, k = 2, OVER THE FOUR LEAST-CONFIDENT BITS.
    Sort the 56 bits by |logit|; enumerate the all-zero, all-single and all-double flip patterns over
    the 4 least confident only. That is 1 + 4 + 6 = 11 CRC trials per window, a fixed constant, with
    no dependence on the logit values. k = 2 over 4 bits and not more: the budget is what section 3.4
    counts, and a wider search buys recovery by spending false-accept margin.
  `Detection.bits_corrected` reports the number of bits flipped by the accepted pattern, 0 to 2. No
error-correcting code runs; the CRC is the only gate and it is checked 11 times.

3.3 WIDTH IS DERIVED, NOT CHOSEN. Rate budget: section 4.6 targets 56 message bits over a 30 s
window, i.e. 1.87 bits/s. That is a 4.7x discount on DeAR's 8.8 bits/s, taken because (a) blind is
strictly harder than oracle best-of-6000-offsets, (b) real rooms are harder than a varechoic chamber,
(c) music is harder than the unstated content of DeAR's test set, and (d) the speaker and microphone
are unknown. RAW-Bench's corrected ~5.3 bps for reverb-only conditions is consistent with 1.87 bps
once transducer band-limiting, background noise and AGC are added. If the measured rate needs more
than 45 s to close, kill criterion K8 fires.

3.4 FALSE-POSITIVE BUDGET FOR THE LOCATOR TIER. N's search space is structurally tiny compared to
Q's 2.646e6 preamble hypotheses, because there is no offset search and no rate grid. The only
multiplicity is the sliding analysis window: 30 s windows at 15 s hop over a 5-minute file is 19
windows, 39 over a 10-minute file. Budget 64 windows as the design maximum.
    64 windows x 11 flip patterns (section 3.2) = 704 CRC trials per file, the maximum.
    704 x 2^-24 = 4.2e-5 false accepts per file, gated on the CRC alone.
Multiplied by the presence head firing on unmarked audio (section 3.5), the compound rate is lower
still, and the message head must also produce a syntactically valid version and namespace. CRC-24
rather than Q's CRC-32C is affordable precisely because the trial count is 3,750x smaller than Q's
2.646e6, and the 8 recovered bits go to the locator where they are worth 256x fewer collisions. If
the flip budget is ever widened past k = 2 over 4 bits, this count and the CRC width must both be
redone; widening one without the other is how a false identity attribution ships.

3.5 THE PRESENCE TIER HAS NO CRC, AND THAT IS THE HARD PART OF THIS DESIGN. Zero-bit presence is a
threshold on the integrated presence-head score over a W-frame window; nothing gates it but the
threshold. Its false-positive rate is therefore a purely empirical quantity, and it is the number
most likely to sink the tier that section 0 nominates as the likely deliverable. It MUST be
calibrated on >= 5000 unmarked simulated-acoustic trials to a NAMED operating point of
`false_positive.rate` <= 1e-3, before any physical run; the physical run CONFIRMS rather than
establishes it (section 12.2), because a few hundred unmarked physical captures bound the rate only
at ~3/N ~= 1e-2. The threshold is frozen at the simulated operating point and is never re-tuned to
recover recall — kill criterion K5a is exactly this test. Presence detection contributes a Finding
and never a `match` value, never an identity, and never a status change.

=====================================================================
4. SYNCHRONISATION — WHERE THESE SYSTEMS ACTUALLY FAIL, AND WHAT WE DO INSTEAD
=====================================================================
4.1 THE FAILURE MODE, NAMED. Every published acoustic result obtains its alignment by searching
offsets and keeping the one that scores best AGAINST THE KNOWN PAYLOAD. DeAR shifts over a 113 ms
range and takes the maximum. DeepAWR sweeps 6000 offsets, breaks early on a perfect hit, and
discards the CRC it computes. A deployed detector cannot do either: it does not know the payload, and
it must operate at a stated false-accept budget. Removing that crutch is the engineering problem, not
an implementation detail, and every design decision in this section exists to remove it.

4.2 THE SOLUTION IS TO ELIMINATE SYNCHRONISATION, NOT TO SOLVE IT. The message head pools each bit's
activation trace over the whole presence-weighted window and reads the bit from the pooled
difference. Pooling is invariant to translation. Therefore:
  BULK DELAY (acoustic propagation 3-30 ms at 1-10 m, plus playback and capture buffering, typically
  20-300 ms, plus an arbitrary capture start): a pure translation. Invariant, up to the edge effect
  of losing partial frames at the window boundary, which costs at most 2/T of the integration.
  CLOCK DRIFT (independent playback DAC and capture ADC oscillators, typically +/- 50-150 ppm, worst
  case 300 ppm): a slow dilation of the time axis. Pooled averages are invariant to it to first
  order. On the FREQUENCY axis, 300 ppm at the top band bin (328) is a shift of 0.098 bins against a
  23.4 Hz bin width — negligible, and no rate grid is needed. Compare Watermark-Q, which needs 16
  coarse plus 24 fine rho hypotheses to survive the same drift. This is a genuine architectural win
  and the reason the neural mark is cheaper to synchronise than the classical one, not more expensive.

4.3 WHAT PAYS FOR IT. The payload is constant over the whole marked span. There is no per-block
payload variation and no block index. A file cannot carry two different N payloads in two sections;
if it does, the pooled read is a mixture and the CRC fails, which is the correct behaviour.

4.4 THE TRAINING MECHANISM THAT MAKES POSITION-AGNOSTICISM TRUE RATHER THAN ASSERTED. If the encoder
learns to put bit i in frames congruent to i mod K, pooling destroys the message and the design
fails silently. Three distortion-layer components force the property, all with p = 1.0:
  (a) RANDOM SAMPLE-LEVEL OFFSET. Each training example reads a 5 s window at a uniformly random
      SAMPLE offset out of a 10 s source clip, before the STFT. Not a spectrogram roll (which creates
      a wrap discontinuity the decoder can learn to key on) and not a frame-aligned shift (which lets
      the model lean on STFT frame phase). Real acoustic delay is not frame-aligned; the augmentation
      must not be either.
  (b) RANDOM CONTIGUOUS MARKED FRACTION. A uniformly random contiguous fraction U(0.2, 1.0) of each
      clip is marked and the remainder is left as cover, with the per-frame presence label following
      exactly. Without this the presence head degenerates into a whole-clip classifier, the
      localisation claim is false, and splices and partial marks are undetectable.
  (c) RANDOM WINDOW LENGTH. Analysis length is sampled from U(3 s, 5 s) at train time while inference
      uses 20-45 s. Train-short / test-long is safe for a pooled readout but must be verified, not
      assumed: bench row N-B7 measures recovery at 5, 10, 20, 30 and 45 s and the design is falsified
      if recovery is not monotone in duration.

4.5 PARTIAL MARKS AND SPLICES. The detector runs 30 s windows at 15 s hop. Each window is an
independent CRC trial and contributes to the multiplicity in section 3.4. Within a window, the
message pooling is restricted to frames where sigmoid(p[t]) exceeds 0.5 after a 0.5 s median smooth.
A window whose presence-positive span is under 8 s does not attempt a message read.

4.6 INTEGRATION AND DURATION. Target operating point: 56 bits over 30 s of continuous captured audio
at 1 m. Reported, never assumed: `minimum_duration_s` is a bench output (row N-B7), populated into
`capabilities()`, and the SDK refuses to attempt a locator read below it rather than returning a
low-confidence guess.

4.7 WHAT REMAINS UNSOLVED AND IS DECLARED SO. Playback speed or pitch changes beyond +/- 0.5% break
the frequency alignment of the learned features; the mark is not searched over rate and will not
decode. Same declared failure as Watermark-Q, at a tighter tolerance, because N has no rate grid.

=====================================================================
5. WHAT A WATERMARK-N DETECTION CAN AND CANNOT MEAN
=====================================================================
5.1 IT IS A SOFT BINDING, TWO RUNGS BELOW A HARD ONE. It resolves a registry bucket. It is not
evidence the audio is unmodified — after a room, the content hash matches nothing, by construction.

5.2 IT NEVER EXCEEDS PROOF LEVEL `inferred`. Not on a single window, not on ten agreeing windows.
Watermark-Q's `strong` class earns `directly_observed` because its false-accept budget is measured at
1.45e-7 against a counted 2.646e6-hypothesis search space. N has no such counted space and, until
section 12's gate produces a blind physical false-positive bound, no measured budget at all.

5.3 THE LOCATOR RESOLVES A CANDIDATE SET, NOT AN IDENTITY, AND AT SCALE IT IS OFTEN AMBIGUOUS. With
25 bits over 2^25 buckets and 2^24 registered works, expected occupancy is 0.5 and the fraction of
works sharing a bucket with at least one other work is 1 - e^-0.5 = 39.3%. Watermark-Q resolves its
own collisions by returning all matching refs and disambiguating each by full-digest re-derivation
plus soft-binding association. NEITHER DISAMBIGUATOR IS AVAILABLE ON THE PATH N EXISTS FOR: the
content hash cannot match after a room, and Q returned nothing (that is why N ran). So roughly two in
five acoustic locator hits at full registry scale resolve to two or more candidates with no way to
choose between them. That is not a defect to be fixed later; it is what 25 bits buys, and 25 bits is
what the channel affords. The consequence is enforced in section 8.5: an ambiguous N hit yields
`untrusted / ambiguous_binding` plus a Finding listing the candidates, never a coin flip, and never
an `identity`.

5.4 CONFIDENCE CLASSES.
  `locator_multi`  : two or more non-overlapping windows CRC-pass with identical 32 payload bits, and
                     the registry bucket contains exactly one candidate. `match` <= 0.85.
  `locator_single` : one window CRC-passes, bucket unique. `match` <= 0.70.
  `locator_ambiguous`: CRC passes, bucket holds >= 2 candidates. `match` = 0, status `untrusted`.
  `presence`       : presence integral over threshold, no CRC pass. `match` = 0, Finding only.
  `none`           : nothing.
  The 0.85 ceiling sits below Watermark-Q's soft cap of 0.99 and is not a tuning parameter; it is
  raised only if and when a blind physical false-positive rate exists to justify it.

=====================================================================
6. THE DISTORTION LAYER
=====================================================================
Applied in physical order. Probabilities are per training example. All ranges are the Phase C
(final) ranges; section 9.3 gives the curriculum that reaches them.

  D0  WATERMARK-Q EMBED, p = 0.8. The shipped order is N then Q (section 8.1), so N must survive Q's
      antisymmetric +/- 0.43 dB per-bin gain in 861-4307 Hz. Applied with the QIM offset computed
      under `torch.no_grad()` and the resulting multiplicative gain applied with gradient flowing
      through the magnitudes. Random namespace and random payload each example.
  D1  SOURCE CODEC HISTORY, p = 0.5. The cover may already be lossy. MP3 128-320, AAC 128-256, Opus
      96-192. THIS IS THE ONLY CODEC STAGE THAT MAY BE PRE-RENDERED AND CACHED, because it precedes
      embedding and is therefore a property of the source clip.
  D2  RANDOM SAMPLE-LEVEL OFFSET AND MARKED FRACTION, p = 1.0. Section 4.4(a) and 4.4(b).
  D3  PLAYBACK GAIN, U(-18, +3) dBFS peak-normalised target.
  D4  SPEAKER NONLINEARITY, p = 0.7. Memoryless third- and fifth-order polynomial soft clip
      parameterised to give total harmonic distortion U(0.3%, 3%) at the current level, plus a
      one-pole thermal-compression term with 200 ms time constant and up to 2 dB of gain reduction.
      Consumer speakers driven at demonstration SPL are not linear and no paper in the survey models
      this.
  D5  TRANSDUCER RESPONSE, p = 1.0. Cascade of: 2nd-order high-pass at U(60, 160) Hz, 2nd-order
      low-pass at U(11, 18) kHz, and 3 to 5 peaking sections at log-uniform centres in
      [200 Hz, 8 kHz] with gain U(-6, +6) dB and Q U(0.7, 3.0). THIS STAGE IS LOAD-BEARING AND IS
      WHAT AWARE'S EVALUATION DID NOT HAVE: its "band-pass 50 Hz - 8 kHz to reflect microphone and
      loudspeaker characteristics" was a no-op at 16 kHz because Nyquist is 8 kHz. The measured gap
      between WavMark's 0.73 full-message under reverb alone and BER 0.500 under reverb plus
      band-limiting is the size of the error a bench that skips this stage makes.
  D6  ROOM IMPULSE RESPONSE, p = 1.0. Convolution with an IR drawn from the corpus (section 9.2),
      resampled to 48 kHz, with the direct tap and the tail independently scaled to hit a
      direct-to-reverberant ratio sampled from U(-6, +18) dB. DRR is the distance proxy and is the
      variable the operating envelope is reported against.
  D7  BACKGROUND NOISE, p = 1.0. A MUSAN noise excerpt plus a Gaussian floor, mixed to SNR
      U(15, 40) dB against the reverberated signal.
  D8  CAPTURE AGC AND DYNAMICS, p = 0.5. Single-band compressor, ratio U(2, 6):1, threshold set to
      engage on the loudest 30% of frames, attack U(5, 50) ms, release U(50, 500) ms, plus a slow
      makeup gain with a 3 s time constant. Every phone recorder does this and no method in the
      survey models it.
  D9  CLOCK DRIFT, p = 1.0. Resample by 1 + U(-300, +300) ppm. Also a 48 -> 44.1 -> 48 round trip
      with p = 0.3.
  D10 A/D QUANTISATION, p = 1.0. 16-bit with TPDF dither. 24-bit with p = 0.2.
  D11 CAPTURE-SIDE CODEC, p = 0.7. AAC 64-256 (phone recorders write m4a), Opus 32-128, MP3 96-320.
      Forward pass through a real encoder/decoder, identity gradient on the backward pass — the
      pseudo-differentiable trick whose ablation in SilentCipher drops MP3 accuracy to zero.
      THIS STAGE CANNOT BE PRE-RENDERED. It follows AGC and quantisation in the physical chain, so
      caching it would model a chain that does not exist. Budget it as an online cost (section 9.5).
  D12 TIME CROP, p = 1.0. Random contiguous crop to the sampled analysis length.

  ELECTRONIC-ONLY BRANCH, p = 0.35 of examples: D0, D1, D2, D3, D9, D10, D11, D12 only. Keeps N
  useful on the digital path, which matters because N's value is disjointness from Q, and Q dies on
  low-pass, telephony bands and mid/side extraction where a 211-7688 Hz learned mark need not.

  DELIBERATELY ABSENT: neural codecs. RAW-Bench establishes that all four benchmarked methods score
  0.00 full-message accuracy under the Descript Audio Codec and that retraining does not fix it,
  because watermarks and neural codecs compete for the same perceptual headroom. Training against
  them would burn capacity for no gain. This is a declared limit, section 11.

=====================================================================
7. INFERENCE IN RUST
=====================================================================
7.1 RUNTIME: `candle` (candle-core + candle-nn), Metal feature on macOS, Accelerate on CPU. Weights
as `.safetensors`. The honest discriminators against `ort`:
  - No build-time C++ toolchain or ONNX Runtime binary. `ort` either downloads a prebuilt library at
    build time (unacceptable for a shippable SDK) or requires a vendored static ONNX Runtime, which
    is a large C++ dependency for a workspace that currently has none.
  - Op coverage is a solved problem here because section 2.9 constrains the architecture to the set
    candle covers natively. This is a design constraint accepted up front, not a discovery made
    during the port.
  - Numerical parity is measurable and is a gate (7.4), which is the real risk with either runtime.
  Not discriminators, and not to be cited as such: `unsafe_code = "deny"` (both crates use unsafe
  internally; neither forces unsafe into our crate) and wasm32 support (the product has not asserted
  a wasm requirement for the neural mark, and the model would be too large for one).
  `ort` is retained as an OFFLINE CROSS-CHECK ONLY: the same graph exported to ONNX opset 17 and run
  under onnxruntime in the Python harness, to catch a candle port that diverges from PyTorch.

7.2 STFT IS DONE IN RUST, NOT IN THE GRAPH. ONNX's STFT operator has patchy runtime support and
candle has none. The exported graph is pure convolution: input is a float tensor [1, 1, 320, T] of
log-magnitudes, output is [1, 1, 320, T] for the encoder and ([1, T], [1, 56]) for the decoder. The
Rust side owns windowing, FFT, band slicing, log, gain application to the complex bins, and WOLA
resynthesis, using the workspace's existing `realfft` 3.5.0 and `rubato` 5.0.0 and the same
square-root Hann WOLA path Watermark-Q already uses. Consequence: the model file contains no control
flow, no dynamic shapes beyond T, and nothing that can differ between runtimes except arithmetic.

7.3 LATENCY TARGETS, EACH A BENCH ROW. Detect over 30 s at 48 kHz (T = 2813 frames): RTF <= 0.15x on
CPU without Metal, <= 0.05x with Metal. Embed: RTF <= 0.30x. For comparison Watermark-Q detects at
RTF ~0.02x, so N is roughly 7x more expensive; the SDK therefore runs N only when Q returns `none`
or when the caller explicitly asks for the acoustic rung. `mean_detect_seconds` in the existing bench
report is the measurement.

7.4 PORT PARITY IS A GATE, NOT A HOPE. Bench row N-B12: for 200 corpus items through 6 channels,
the Rust decoder's 56 bit logits and per-frame presence logits must agree with the PyTorch reference
to max absolute error < 1e-3 (fp32) and the accept/reject decision must agree on 100% of trials. A
silently diverging port is the classic failure in this class of work and it is cheap to measure.

7.5 MODEL DISTRIBUTION AND VERSIONING. The weights are NOT vendored into the crate: 22 MB would
exceed crates.io's package limit and bloat every build. They ship as a separate artifact
`apw-watermark-neural-v1-<epoch>.safetensors` with its SHA-256 PINNED IN THE CRATE SOURCE. The loader verifies
the digest before deserialising and refuses a mismatch. THE MODEL FILE IS TRUSTED CODE-EQUIVALENT: a
swapped model is a forgery vector that would let an attacker mint accepts, so an unpinned path is
accepted only behind an explicit `allow_unpinned_model` escape that is off by default and recorded in
the result's `findings`. A build without the model file reports the N rung as `unavailable` and sets
`incomplete`, exactly as an unreachable registry does; it never becomes a verdict.

7.6 ALGORITHM AND MODEL VERSIONING. The algorithm id `apw-watermark-neural-v1` is bound to a message
layout and a band, not to a weight epoch. A retrained model that keeps both is a new epoch under the
same id and every shipped detector must decode every shipped encoder's output; row N-B13 measures
cross-epoch decode on the previous epoch's marked corpus and a regression there blocks the release. A
change to the layout or the band is `apw-watermark-neural-v2`, a new id, and both detectors run.

=====================================================================
8. COEXISTENCE WITH WATERMARK-Q
=====================================================================
8.1 ORDER: N, THEN Q, THEN HASH, THEN SIGN. Q's embed is closed-form and blind: whatever N did to the
host, Q recomputes d[s] on the N-marked audio and lands its lattice point exactly, and its two-pass
OLA closure absorbs the rest. So Q cannot be broken by N being applied first, and the asymmetry runs
the right way — the shipped, validated mark is protected by construction and the experimental one
must survive the shipped one's perturbation (distortion-layer stage D0). `sign()` continues to refuse
a mark against an already-signed input with `mark_after_sign`.

8.2 THE REAL COUPLING IS Q's NOISE FLOOR, NOT Q's CORRECTNESS. Watermark-Q section 3.3 sets
DELTA = 4.0 * sigma_d where sigma_d is the measured residual noise on d. If N's residual raises
sigma_d, Q must raise DELTA and pays for N in fidelity across every file, including ones nobody will
ever re-record. The constraint is therefore quantitative:
    std over the corpus of ( D_marked[t,k] - D_cover[t,k] ) < 0.04 nepers  ( = DELTA/20 )
on Q's exact statistic — 14 active pairs after the guard mask, trimmed mean, its own 1024-point STFT
at the native rate. 0.04 is not a new number; it is the bound Q's own OLA-closure unit test asserts.
It is enforced as a training loss (section 9.4, L_q) and MEASURED in bench row N-B3.

8.3 IT IS AN EMPIRICAL BOUND, NOT A PROOF, AND THE TEMPTING PROOF DOES NOT HOLD. A per-bin gain that
is constant across all four bins of a Q pair leaves ln E_A - ln E_B exactly unchanged, which suggests
constraining N's mask to be piecewise-constant on Q's pair grid. It does not work: Q's grid is a
1024-point STFT at the native rate (43.07 Hz bins, 172.3 Hz pairs at 44.1 kHz) and N's mask is a
2048-point STFT at 48 kHz (23.44 Hz bins) whose residual is then resampled back. The grids are
incommensurate and the resample smears bin boundaries. So exact cancellation is unavailable and must
not be claimed. What is available is the measured bound in 8.2 and a training loss that drives
toward it. Anyone who writes "N is orthogonal to Q by construction" in a README is wrong.

8.4 C0 MUST BE RE-RUN WITH N PRESENT. Watermark-Q's calibration unit C0 measures sigma_d and sets
DELTA before the embedder is written. It must be run twice — cover-only and N-marked — and BOTH
DELTA values recorded. If the N-present DELTA exceeds the cover-only DELTA by more than 5%, N is
taxing Q's fidelity on every file and the mask budget must come down or the program stops. This is
kill criterion K3.

8.5 WHICH MARK A DETECTOR TRUSTS. Strict precedence, highest first:
    1. hard binding, exact hash match                    -> `verified`, match 1.0, directly_observed
    2. Watermark-Q `strong` (two agreeing blocks)         -> match <= 0.99, directly_observed
    3. Watermark-Q `single`                               -> match <= 0.90, inferred
    4. Watermark-N `locator_multi`, bucket unique         -> match <= 0.85, inferred
    5. Watermark-N `locator_single`, bucket unique        -> match <= 0.70, inferred
    6. Watermark-N `presence`                             -> match 0, Finding only, no identity
    7. fingerprint                                       -> unchanged, never `verified`
  DISAGREEMENT IS TERMINAL. If both marks decode and N's 25 locator bits are not the leading 25 bits
  of Q's 48, the result is `untrusted / mark_disagreement` with no descent down the ladder. Two marks
  from different works on one file is a splice or a forgery, and it is the one case where running two
  marks buys a detection capability neither has alone.
  AMBIGUITY IS TERMINAL TOO. An N locator hit whose registry bucket holds two or more candidates is
  `untrusted / ambiguous_binding` with the candidate list in `findings`. See 5.3: at registry scale
  this is the expected outcome roughly 39% of the time.

8.6 API DELTA, KEPT MINIMAL AND STATED EXPLICITLY. `VerifyStatus` is unchanged — the four statuses
are fixed and N adds none. `MatchBasis` is unchanged at
`"hard_exact" | "apw_watermark" | "fingerprint" | "none"`; an N-sourced match reports `"apw_watermark"`,
because it is the same mark family resolving the same locator, and the brief's snippet must keep
compiling. Three additions, all widenings of unions the brief's snippet does not pin:
    - `BindingReport.kind` gains `"soft_apw_watermark_neural"`.
    - `RecoveryMethod` gains `"apw_watermark_neural_recovery"`.
    - `MarkReport` gains `marks: readonly ("lepqim" | "neural")[]` so an embed reports what it wrote.
  `capabilities().acousticRerecording` STAYS THE LITERAL `"unsupported"` until section 12's gate
  passes. If it ever passes it becomes a structured object, never a boolean and never `true`:
    { status: "measured_limited", maxDistanceM, minDurationS, roomsMeasured, speakersMeasured,
      microphonesMeasured, blindDetectionRate, falsePositiveRate, falsePositiveTrials }
  A boolean is what lets marketing outrun engineering. A record that carries the distance, the
  duration, the rate and the trial count behind its own false-positive bound cannot be quoted without
  its conditions.

=====================================================================
9. TRAINING
=====================================================================
9.1 AUDIO DATA, LICENCE-FILTERED. MTG-Jamendo, filtered to CC0 / CC-BY / CC-BY-SA only, excluding
every NC and ND track: approximately 25-30k tracks after filtering, from which 800 h of 48 kHz music
is drawn. Audio Provenance's own licensed catalogue is added where available and is the preferred source. 40 h
of VCTK speech and 40 h of a CC-BY environmental-sound subset for content diversity, matching the
composition RAW-Bench's retrain used.
  FMA IS NOT USED WITHOUT PER-TRACK FILTERING. Its metadata is CC BY-NC-SA 4.0 and its audio carries
  heterogeneous per-track licences, a substantial fraction NonCommercial. Both DeAR and DeepAWR
  trained on unfiltered FMA; that is a real exposure for a shipped commercial model and is one more
  reason their weights are not a shortcut.

9.2 IMPULSE RESPONSES, LICENCE-CHECKED, IN PRIORITY ORDER.
  1. AUDIO PROVENANCE'S OWN MEASURED IRs from the Stage 0 campaign (section 10). Owned outright, at 48 kHz,
     and measured with the exact speakers and microphones the product will face. These are the most
     valuable training data in the program and are the main reason Stage 0 comes first.
  2. MIT Acoustical Reverberation Scene Statistics Survey (Traer and McDermott), 271 IRs from real
     everyday spaces, CC BY 4.0, 44.1 kHz. Upsampled to 48 kHz; legitimate because nothing above
     22.05 kHz is fabricated into the band we use.
  3. EchoThief, 48 kHz, whose terms explicitly grant convolution-derivative use.
  NOT USED: OpenSLR-28 RIRS_NOISES, which is 16 kHz only and cannot drive a 48 kHz layer without
  fabricating the entire 8-24 kHz band — precisely where transducer roll-off lives. NOT USED: Aachen
  AIR standalone, which is research-use, notwithstanding that AWARE used it.
  NOISE: MUSAN (OpenSLR-17), CC BY 4.0, plus Audio Provenance's own Stage 0 room-noise recordings.

9.3 CURRICULUM, THREE PHASES.
  Phase A, steps 0-60k: D0-D3, D9-D12 only. No reverberation, no transducer, no noise, no AGC. Goal:
    clean-channel bit accuracy > 0.999, perceptual budget in range, presence head AUC > 0.99.
    A model that cannot do this cannot do anything harder; a Phase A failure is a bug, not a result.
  Phase B, steps 60k-180k: introduce D4-D8, ramped linearly. DRR from U(+12, +25) dB down to
    U(+2, +18); noise SNR from U(35, 45) dB down to U(20, 40); AGC probability 0 to 0.5.
  Phase C, steps 180k-320k: full Phase C ranges from section 6, with the Stage 0 measured IRs and
    measured transducer responses mixed in at 50% weight. Perceptual budget frozen at its calibrated
    kappa.

9.4 LOSSES.
    L = 1.0 * L_msg + 1.0 * L_det + 20.0 * L_perc + 4.0 * L_spec + 5.0 * L_q + 0.1 * L_adv
  L_msg   BCE on the 56 pooled bit logits, plus an auxiliary BCE on the per-frame bit traces
          restricted to marked frames (weight 0.2 within L_msg). The auxiliary term is what stops the
          encoder concentrating a bit into a few frames.
  L_det   BCE on the per-frame presence logits against the marked/unmarked label from D2(b), with
          50% of every batch unmarked cover so the head learns a real negative class. This term is
          what makes the presence-tier false-positive rate controllable at all.
  L_perc  ReLU(NMR[t,f]) summed over the band, where NMR is the noise-to-mask ratio in dB computed
          with the same Schroeder / half-Bark / alpha=0.5 model as `audio-provenance-bench/src/perceptual.rs`.
          Training and the bench must optimise and measure the SAME function or a training win is a
          measurement artifact.
  L_spec  Multi-scale spectral L1 between cover and marked at n_fft in {512, 1024, 2048, 4096}.
  L_q     ReLU( std_corpus( D_marked - D_cover ) - 0.04 ) on Watermark-Q's exact statistic, computed
          on the native-rate 1024-point grid with Q's guard mask and trim. Hinge, not quadratic: the
          bound is what matters, not driving the difference to zero.
  L_adv   Hinge GAN against a small multi-resolution spectrogram discriminator. BEHIND A FLAG AND OFF
          FOR THE FIRST FULL RUN, because adversarial training is the most likely source of an
          unreproducible result and the program's first job is a clean falsifiable number.

9.5 OPTIMISER AND SCHEDULE. AdamW, lr 1e-4, betas (0.9, 0.99), weight decay 1e-2, 2000-step linear
warmup then cosine to 1e-6, gradient clip 1.0. Batch 16 clips of U(3, 5) s at 48 kHz. 320k steps.
EMA of the encoder weights with decay 0.9999 for the shipped checkpoint.

9.6 COMPUTE, HONESTLY. The development machine is aarch64-apple-darwin: MPS is available, CUDA is
not. PyTorch >= 2.6 is required (DeepAWR's pinned torch 1.10.2+cu113 predates MPS entirely, so its
recipe cannot be run as written on this hardware at all).
  MPS ESTIMATE: 5.5M parameters over 320 x 470 spectrograms at batch 16 is roughly 0.6-1.2 s/step for
  the network alone. The dominant cost is not the network, it is stage D11: a real encode/decode per
  example, on CPU, that CANNOT be cached because it follows AGC in the chain. At batch 16 with 70%
  probability that is ~11 codec round trips per step, which on a single machine plausibly triples the
  step time. Realistic budget: 2.5-4.5 s/step, i.e. 9 to 17 DAYS wall clock for one 320k-step run,
  with three to five runs needed before a result is trustworthy. That is 7 to 12 weeks of calendar
  time for training alone.
  THE RECOMMENDATION IS THEREFORE TO RENT A SINGLE CUDA GPU. One A100 or 4090 at roughly $0.5-1.5/h
  completes a run in 1.5-2.5 days for $50-90, with codec augmentation parallelised across CPU workers
  the Apple machine does not have. MPS is the development, debugging and smoke-test path; it is not
  the training path. Pretending otherwise would put a 10-week schedule behind a $70 line item.
  MANDATORY SMOKE TEST BEFORE ANY BUDGET IS BELIEVED: 500 steps on the target hardware with the full
  Phase C distortion layer enabled, wall-clocked. Kill criterion K8 reads its number from this test,
  not from this paragraph.

=====================================================================
10. STAGE 0 — THE PHYSICAL CAPTURE CAMPAIGN, WHICH COMES FIRST
=====================================================================
This is the most important section in the document. Every physical result in the literature is one
room, one speaker, one microphone, with room type, RT60, background level and playback SPL reported
nowhere. Stage 0 fixes that for Audio Provenance before a single training step runs, and its outputs are
inputs to everything after it.

10.1 RIG. Three playback devices spanning the real range: a consumer powered monitor, a small
Bluetooth speaker, and a laptop's internal speakers. Three capture devices: a current iPhone, a
current Android phone, and a USB condenser microphone. Three rooms with MEASURED and REPORTED RT60
and background level: a treated small room (RT60 ~0.25 s, < 30 dBA), an untreated office
(RT60 ~0.45 s, 35-40 dBA), and a live hard-surfaced room (RT60 ~0.8 s, 40-45 dBA). Distances
0.15, 0.5, 1.0, 2.0 and 3.0 m. Playback SPL measured at 1 m and reported, at two levels.

10.2 DELIVERABLES.
  (a) A measured 48 kHz impulse response for every (room, speaker, mic, distance, SPL) cell, by
      exponential sine sweep with Farina deconvolution. This corpus is owned outright and is the
      highest-value training data in the program.
  (b) A measured background-noise recording per room.
  (c) A measured frequency response per speaker and per microphone, to parameterise stage D5 with
      real curves rather than random biquads alone.
  (d) A marked/captured corpus: 200 corpus tracks x the cell grid, played and recorded end to end.
  (e) AN UNMARKED CAPTURED CORPUS OF AT LEAST EQUAL SIZE, captured under identical conditions. The
      false-positive arm is not an afterthought and cannot be synthesised later.

10.3 BASELINE MEASUREMENT — THE NUMBER THAT DOES NOT EXIST ANYWHERE. Run AudioSeal and WavMark, both
MIT on code and weights, over the Stage 0 physical captures BLIND: no offset oracle, no
knowledge of the payload, a fixed threshold, and the false-positive arm on the unmarked captures.
This produces the first blind physical detection rate for any licence-clean audio watermark. Whatever
it is, it calibrates every expectation in this document, and it costs days.

10.4 HOW THE PHYSICAL ARM ENTERS `audio-provenance-bench`, AND WHY IT IS NOT A `Channel`. The existing
`Channel` trait is `apply(&Audio, seed, &dyn CommandRunner) -> Audio`: a pure transform of in-memory
audio. A physical capture is not a transform; it is a LOOKUP of a previously recorded counterpart
file. Do not force it into `Channel`. Model it as a paired corpus — `corpus.rs` already has
`load_wav_directory`, `load_wav_files` and `CorpusSource` — with a passthrough channel named
`physical_<room>_<speaker>_<mic>_<distance>` whose `params()` carry the measured RT60, background
level, SPL and hardware identifiers, and whose `ChannelFamily` is a new `MeasuredPhysical` variant
that `is_simulated_physical_path()` returns false for. THIS IS A PROPOSAL TO THE OWNER OF
`audio-provenance-bench`, NOT AN EDIT MADE BY THIS DOCUMENT. The same applies to every threshold in
section 12: `Thresholds::product_targets()` currently declares all three acoustic rows
`ExpectedFailure`, and changing that is the bench owner's call.

10.5 A NEW 1.0 m SIMULATED PRESET IS NEEDED. The existing `RoomPreset::MEDIUM` is 2.0 m, RT60 0.60 s,
DRR +2 dB — harder than anything in the literature's physical results and well outside this design's
stated envelope. Propose `RoomPreset::NEAR_FIELD_1M` at RT60 0.35 s, distance 1.0 m, DRR +8 dB,
room noise SNR 36 dB, drift 1.00015. Stage 1 gates on `acoustic_small_room` (0.5 m) and this new
1 m row. `acoustic_medium_room` and `acoustic_large_room` stay `ExpectedFailure` throughout; a pass
there would be a surprise, and lowering a threshold to claim one is the failure mode this whole
document is written against.

=====================================================================
11. HONEST LIMITS — THE ENVELOPE, WRITTEN SO THE API AND README CANNOT OUTRUN IT
=====================================================================
NOTHING IS DELIVERED TODAY. Until section 12's Stage 2 gate passes on measured physical captures,
`capabilities().acousticRerecording` is the literal `"unsupported"`, `verify()` on a microphone
recording returns `not_found` by design, and the product website must not claim re-recording survival. This
paragraph is the one that goes in the README.

WHAT IS THE BEST CASE IF EVERYTHING WORKS. Presence detection at approximately 1 m over 10 s windows,
and a 25-bit locator prefix over 30 s of continuous captured audio at approximately 1 m, in a quiet
room (background <= 40 dBA, RT60 <= 0.6 s), through one consumer speaker at moderate SPL into one
phone or USB microphone. That is a phone lying on a desk in front of a monitor. It is not a phone in
a pocket, not the back of a room, not a venue, and not a noisy environment.

EVEN IN THE BEST CASE, N NEVER PRODUCES AN IDENTITY ON ITS OWN. 25 bits over 2^25 buckets at 2^24
registered works leaves 39% of works sharing a bucket, and neither of Watermark-Q's two
disambiguators is available after a room. An N locator hit is a candidate set, capped at proof level
`inferred` and `match` 0.85, and it is `untrusted / ambiguous_binding` whenever the bucket is not
unique.

EVERY PUBLISHED NUMBER WE ARE COMPARING AGAINST IS AN ORACLE NUMBER. DeAR's 99.18 / 98.55 / 93.40 /
92.68 % at 5 / 20 / 50 / 100 cm and DeepAWR's 0.99 are best-of-search maxima computed with knowledge
of the true bits. DeepAWR's shipped evidence is six WAV files per condition. Our blind, CRC-gated
number will be lower than any of them, and it must always be published with its false-positive rate
and the trial count that bounds it. Anyone quoting a published acoustic accuracy as a Audio Provenance
capability is quoting a different quantity.

MEAN BIT ACCURACY IS NOT A DETECTION RATE AND WILL NOT BE REPORTED AS ONE. 92.68% mean bit accuracy
over 100 bits is P(message exactly recovered) = 5.0e-4 under an independence assumption the room
violates in the pessimistic direction.

WHERE IT WILL NOT WORK, STATED IN ADVANCE SO A FAILURE IS NOT A SURPRISE: beyond 2 m at any RT60;
occupied or noisy rooms above ~45 dBA; a microphone that is not pointed at the source; captures under
20 s for the locator tier or under 10 s for presence; laptop speakers at low SPL, where the 211 Hz
band floor is barely reproduced; playback speed or pitch changes beyond +/- 0.5%; and NEURAL CODECS,
where RAW-Bench establishes 0.00 full-message accuracy for every method tested and that retraining
does not fix it. Watermark-Q survives the transcode chain and dies in the room; Watermark-N is intended
to survive the room and will very likely die in a neural codec. That disjointness is the entire
argument for shipping both, and it is also an admission that neither covers the union.

THE MARK IS REMOVABLE IN THE PUBLIC NAMESPACE, exactly as Watermark-Q's is, and more so: a learned
encoder's residual is estimable by anyone holding the public model, and the model must be public for
detection to be public. A keyed namespace conditions the message embedding on a key-derived vector,
which raises the cost of estimation without making it hard. N is provenance RECOVERY, not tamper
resistance, and the docs say so in the same words Q's do.

THE PRESENCE TIER'S FALSE-POSITIVE RATE IS THE WEAKEST NUMBER IN THE DESIGN. It is gated by a
threshold on an integrated score with no CRC behind it. It is calibrated on >= 5000 unmarked
simulated trials and CONFIRMED, not established, on the achievable physical count, which will be in
the hundreds. Its published bound is 3/N with N stated, per the bench's own `FALSE_POSITIVE_NOTE`. A
presence rate quoted without its N is not a result.

WHAT HAS NEVER BEEN MEASURED BY ANYONE: embedding a learned watermark into audio that already carries
a different watermark. No paper in the survey evaluates it. Section 8's bounds are this program's own
invention and bench row N-B3 is the only evidence that will exist for them.

=====================================================================
12. GATES AND KILL CRITERIA
=====================================================================
12.1 STAGES AND CALENDAR.
  Stage 0, 2 weeks: capture campaign (section 10), IR corpus, blind baselines. Gate: K0.
  Stage 1, 4 weeks: distortion layer, training harness, Phase A + B model. Gate: K1, K2, K3, K7, K8.
  Stage 2, 6 weeks: Phase C with measured IRs, physical evaluation. Gate: K4, K5, K6, K9.
  Stage 3, 3 weeks: candle port, parity, SDK integration, capabilities record. Gate: N-B12, N-B13.
  Total 15 weeks, with three exit points before week 7.

12.2 HOW FALSE-POSITIVE RATES ARE ESTABLISHED, BECAUSE THE PRECISION MATTERS.
  `Thresholds::product_targets()` sets `max_false_positive_rate: 0.0` — any accept on unmarked audio
  fails the row — and `FALSE_POSITIVE_NOTE` correctly states that zero accepts over N trials bounds
  the rate at roughly 3/N, not at zero. A physical campaign yields hundreds of unmarked captures, not
  thousands, so a 1e-3 bound is NOT physically measurable at Stage 2 scale.
  THEREFORE: both thresholds (the presence integral and the CRC gate) are CALIBRATED on >= 5000
  unmarked trials through the SIMULATED acoustic channels, then CONFIRMED on the physical unmarked
  corpus with zero accepts required and the bound published as 3/N with N stated. No kill criterion
  below asks the physical arm for a precision it cannot deliver.

12.3 KILL CRITERIA. Each is a number the bench measures, using its existing metric names. Any one
firing stops the program at that stage; there is no aggregate score and no partial credit.

  K0  STAGE 0, CHANNEL SANITY. If the measured RT60 at 1.0 m in the treated room exceeds 0.6 s, or
      the measured direct-to-reverberant ratio at 1.0 m is below 0 dB in all three rooms, the target
      envelope does not exist in realistic spaces and the program stops before training. Measured
      from the Stage 0 IRs, not simulated.

  K1  STAGE 1, SIMULATED RECOVERY. On `acoustic_small_room` (0.5 m) AND the proposed
      `acoustic_1m_room` preset, over 200 corpus items at 30 s, `exact_recovery_rate` < 0.60 with
      `false_positive.rate` = 0 over >= 5000 unmarked trials -> KILL. Rationale: simulation is
      strictly easier than physical, and the best published simulated replay result corresponds to
      roughly 0.89. Failing to reach 0.60 in simulation makes physical hopeless.

  K2  STAGE 1, AUDIBILITY. At the budget that achieves K1, if `frames_above_mask_fraction` > 0.10 on
      the `harmonic_pad` or `near_silence` content classes, or `noise_to_mask_max_db` > +3 dB on any
      class, or `segmental_snr_db` < 22 dB -> KILL. A mark that is audible on sparse tonal material
      is not shippable at any robustness.

  K3  STAGE 1, WATERMARK-Q COEXISTENCE. Run the full existing Q matrix twice, with and without N
      present. If Q's `exact_recovery_rate` on `identity`, `mp3_128`, `aac_128` or
      `chain_transcode_mp3_128_aac_128` drops by more than 0.01 absolute, OR the measured
      std(D_marked - D_cover) exceeds 0.04 nepers, OR C0's N-present DELTA exceeds its cover-only
      DELTA by more than 5% -> KILL OR REDESIGN THE BUDGET. N is not permitted to tax the shipped
      mark.

  K4  STAGE 2, THE DECISIVE PHYSICAL GATE, LOCATOR TIER. Over >= 3 rooms x >= 2 speakers x >= 2
      microphones x >= 200 clips at 1.0 m and 30 s, BLIND and CRC-gated with a threshold fixed before
      the run and with the k = 2 over 4 bits flip budget of section 3.2 and nothing wider: if
      `exact_recovery_rate` < 0.50, with `false_positive.rate` = 0 required on the unmarked physical
      corpus and its 3/N bound reported -> KILL THE LOCATOR TIER. Below 0.5 the feature returns
      `not_found` on most real re-recordings, which is worse than honestly declaring the path
      unsupported. A run that widens the flip budget to reach 0.50 has not passed this gate; it has
      moved the false-accept cost somewhere the row does not print.

  K5  STAGE 2, PRESENCE TIER FALLBACK — TWO GATES, THE FIRST OF WHICH RUNS BEFORE ANY PHYSICAL RUN.
      K5a, SIMULATED CALIBRATION, and it is the one that can actually reject the tier. Fix the
      presence threshold at the operating point where `false_positive.rate` <= 1e-3 over >= 5000
      unmarked trials through `acoustic_small_room` and the 1 m preset. If recall at that threshold,
      measured as `payload_returned_rate` for the zero-bit detector over 10 s windows, is < 0.80 ->
      KILL THE PRESENCE TIER HERE, before Stage 2 spends a capture campaign on it. Do NOT re-tune the
      threshold upward to recover recall; the FPR is the constraint and the recall is the result.
      K5b, PHYSICAL CONFIRMATION, at that same frozen threshold: `payload_returned_rate` >= 0.80 at
      1.0 m on the physical corpus AND zero accepts on the unmarked physical corpus. K5b CONFIRMS;
      it cannot establish an FPR, because zero accepts over the few hundred unmarked physical
      captures bounds the rate only at ~3/N ~= 1e-2, which is one false fire per hundred clips with
      no CRC behind it and is not a shippable signal on its own evidence. The 1e-3 number comes from
      K5a and is published as such, with the physical trial count reported beside it.
      If K5a or K5b fails -> KILL THE ENTIRE PROGRAM, ship nothing, leave
      `acousticRerecording: "unsupported"`, and record the measured numbers in `HONEST_LIMITS.md` so
      the question is settled with evidence rather than re-opened annually.

  K6  STAGE 2, DISTANCE HONESTY. If the largest distance at which K4's 0.50 threshold is met is below
      0.5 m -> KILL. A mark that needs the microphone within half a metre is a coupling test, not
      re-recording, and no defensible product claim can be scoped to it.

  K7  ANY STAGE, RUNTIME. `mean_detect_seconds` implying RTF > 0.5x for a 30 s window on the
      development machine's CPU without Metal, or a shipped model file above 40 MB -> KILL OR
      REDESIGN. A rung that costs half of real time cannot run by default.

  K8  ANY STAGE, SCHEDULE. From the mandatory 500-step smoke test (section 9.6): if the projected
      wall clock for one 320k-step run on the chosen hardware exceeds 14 days, the hardware is wrong
      and must change before training starts. If four full runs complete without K1 passing -> KILL.
      If the measured capture duration required to reach K4's threshold exceeds 45 s -> KILL, because
      the duration has left the range of a realistic capture.

  K9  STAGE 2, THE SYNCHRONISATION CLAIM ITSELF. Chain `drift_plus_0p1pct` with the 1 m acoustic row.
      If `exact_recovery_rate` falls more than 0.10 below the acoustic row alone, the
      position-agnostic readout claim in section 4 is false, the design has an unsolved
      synchronisation problem, and it must be redesigned or killed. Additionally, if recovery in row
      N-B7 is not monotone non-decreasing in capture duration across 5, 10, 20, 30 and 45 s, the
      pooling is not integrating and the same conclusion follows.

12.4 WHAT A PASS ENTITLES US TO SAY. Exactly the numbers measured, with the room count, speaker and
microphone count, distance, duration, blind detection rate, false-positive rate and trial count
attached, in the structured `capabilities()` record of section 8.6. Nothing else. Not "survives
re-recording". Not "works in a room". The sentence the marketing site is permitted to publish is
generated FROM that record, and if the record is absent there is no sentence.
