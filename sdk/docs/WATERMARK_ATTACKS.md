# Attacks on GenoMark-Q

A motivated adversary, not a codec. Everything below is measured, on the corpus and with the
commands named at the end. Nothing in it is a prediction.

## The result in one paragraph

Against an attacker who has read `GENOMARK_SPEC.md`, `genomark-lepqim-v1` provides **no removal
resistance and no forgery resistance**. The mark can be erased with a residual the same size as the
mark itself, by three independent methods, none of which needs the profile key or any analysis the
specification does not publish. It can be transplanted onto unrelated audio from a single 8.9 s
block of a marked donor, and an attacker who holds one marked file whose payload is public registry
data can recover the key-derived dither and write **any payload of their choosing** under the
victim's secret key. All three forgery rows succeeded on 6 of 6 items at detector confidence class
`strong`. What the mark does resist is an attacker who does not read the spec: an off-the-shelf
denoiser failed on 6 of 6 while destroying the audio, and small timing and pitch manipulations need
to reach roughly 1% of tempo or 10 cents of pitch before they break it.

This does not make the product broken, and the reason is that Audio Provenance does not rest a verdict on
the watermark. It rests it on a signature over a hash chain, with the mark as a locator and the
signed reference constellation as a second, independent question. What it does mean is that every
sentence describing the watermark has to say `soft binding` and mean it. The claims that follow are
folded into `docs/HONEST_LIMITS.md`.

## The threat model, and the one this is not

`crates/audio-provenance-bench/src/channel` measures **incidental** degradation: mp3, AAC, Opus, resampling,
gain, crops, limiting. Nothing in that matrix is trying to remove the mark, and its numbers say
nothing about an adversary. This document is the other question. The two are not interchangeable and
neither substitutes for the other.

Every row states what the attacker must possess. That column is the finding, not decoration:

| knowledge | means |
|---|---|
| `spec only` | the published algorithm document. No key, no marked file, no analysis of anything. |
| `one marked file` | one marked file, contents unknown. |
| `marked file + its payload` | one marked file and the payload it carries. For a **registered release the payload is registry data**, so this is the ordinary condition, not an exotic one. |
| `many marked copies of one recording` | several differently marked copies of the same recording under one key. A licensee, screener or per-recipient forensic scenario, not a released track. |
| `the key (control)` | the profile key. Possession of the key is not an attack; the row exists to give the perceptual reference. |

## Why it works: the coset leak

The statistic is `d[s]`, the trimmed mean of adjacent-cell log-spectral-energy differences over one
two-frame slot, and the embedder quantises it onto a lattice of period `DELTA = 0.8` nepers with a
key-derived dither `u[s]` and a bit `b[s]` selecting one of two half-period cosets. The detector
reads nothing else. So the only quantity that matters is

    v[s] = (u[s] + b[s] * DELTA/2)  mod DELTA

and **`v[s]` is directly observable from a marked file with no key.** Every block of a key epoch
carries the same dither and the same coded word, so the residues at a given slot-in-block index are
the same in every block, and reading them off is a circular mean over three or four samples.

Measured, on the six-item corpus: the mean resultant length of those residues is **0.9965 to 0.9998
on marked audio and 0.4545 to 0.5475 on the same audio unmarked**. The lattice is not hidden; it is
the loudest structure in the file.

That single fact drives everything below. Removal is "move off the lattice". Transplant is "put
another file's residues on your own audio". Forgery is "subtract the known bit from the observed
residue to get the dither, then write the bit you want".

The guard mask at 1000, 1200, 2000, 2500, 3000 and 3500 Hz and the 86 Hz pair differencing defeat a
**smooth** equaliser. They do not defeat an equaliser shaped like the published cell layout, and the
cell layout is published.

## How the attacks were applied, and the trap in doing it

Weighted overlap-add returns only part of a per-frame spectral change to the next analysis of the
same slot; the embedder measures that closure gain per file and finds it near 0.45. An attack that
applied one spectral multiply and then booked the shift it *asked for* would land at roughly half
strength and would report the mark as far more robust than it is. Every closed-loop row here
iterates to a **measured** target exactly as the embedder does, and every row carries the shift it
actually achieved.

The two `static_eq_open_loop` rows are the control on that. They apply one fixed curve with no
measurement at all: asked 0.4 nepers, achieved a median of 0.19; asked 0.9, achieved 0.43. That is
the 0.45 closure gain, visible directly, and it is why the open-loop attack needs a curve of
±1.95 dB to do what a closed-loop shift of 0.4 nepers does.

## Removal, and what it costs


| attack | attacker needs | removed | seg-SNR of the attack, dB (range over 6 items) | NMR mean, dB | NMR worst frame, dB | median statistic shift achieved, nepers |
|---|---|---:|---|---:|---:|---:|
| `static_eq_open_loop_0.4_nepers` | spec only | 4/6 | 30.0 to 51.3 | -17.10 | -9.52 | 0.19 |
| `static_eq_open_loop_0.9_nepers` | spec only | 6/6 | 22.6 to 44.1 | -9.85 | -2.23 | 0.43 |
| `static_eq_eighth_step` | spec only | 0/6 | 34.5 to 58.7 | -22.74 | -16.29 | 0.10 |
| `static_eq_quarter_step` | spec only | 4/6 | 28.2 to 51.9 | -16.74 | -9.09 | 0.20 |
| `static_eq_half_step` | spec only | 6/6 | 21.1 to 45.7 | -10.29 | -2.27 | 0.40 |
| `random_offset_half_step` | spec only | 0/6 | 36.0 to 58.1 | -23.31 | -11.19 | 0.10 |
| `random_offset_full_step` | spec only | 6/6 | 29.8 to 52.0 | -17.22 | -4.59 | 0.20 |
| `flatten_median_5_slots` | spec only | 6/6 | 30.4 to 54.5 | -18.19 | 1.03 | 0.16 |
| `overwrite_with_attacker_key` | spec only | 6/6 | 29.1 to 52.3 | -17.02 | -4.49 | n/a |
| `spectral_subtraction_denoise` | spec only | 0/6 | 1.1 to 18.4 | -2.94 | 7.64 | n/a |

Reference row, not an attack:

| attack | attacker needs | removed | seg-SNR of the attack, dB (range over 6 items) | NMR mean, dB | NMR worst frame, dB | median statistic shift achieved, nepers |
|---|---|---:|---|---:|---:|---:|
| `embed_reference` | the key (control) | 0/6 | 29.5 to 52.5 | -17.26 | -4.97 | n/a |


The `seg-SNR of the attack` column is measured between the **marked** file and the attacked file.
The reference row is measured between the **original** and the marked file, so it is the perceptual
cost of the mark itself, and every attack row can be read against it.

Paired per item, segmental SNR in dB, higher is a smaller residual:

| item | embedding the mark | randomising the coset | flattening the statistic | half-step static EQ |
|---|---:|---:|---:|---:|
| real_electronic_dense | 52.47 | 51.99 | 54.47 | 45.75 |
| real_hiphop_dense | 40.98 | 40.68 | 41.88 | 34.39 |
| real_pop | 35.47 | 35.49 | 35.52 | 28.51 |
| real_rock_loud | 30.64 | 30.32 | 32.01 | 23.70 |
| real_orchestral | 29.51 | 29.85 | 30.40 | 21.12 |
| real_piano_solo | 43.10 | 43.21 | 44.77 | 35.64 |

Read that table twice. **Randomising the coset removes the mark for a residual within 0.5 dB of what
embedding it cost, on every item. Flattening the statistic removes it for a residual that is
smaller than the mark on all six.** An attack that destroys the audio is vandalism rather than an
attack; these do not destroy the audio, they leave it where the embedder left it.

Two caveats against over-reading that, both from the same measurement. The noise-to-mask ratio of
the worst frame is `-4.97 dB` for the embed and `+1.03 dB` for the flatten row, so flattening does
push above this model's masking threshold somewhere in the file where the embed does not, even
though its mean NMR is 0.9 dB quieter. And the half-step static EQ, which is the simplest removal of
all, is genuinely more audible than the mark: it costs 6 to 8 dB of segmental SNR and lifts mean NMR
from -17.3 to -10.3 dB. Neither figure is a listening test. See the limits note at the end.

The failed rows matter as much. `static_eq_eighth_step` and `random_offset_half_step` moved the
statistic by 0.10 nepers and removed nothing on 6 of 6, so the mark is not fragile to small
perturbation; it is fragile to a perturbation of the right size, which the specification names.
`spectral_subtraction_denoise` is the naive attack, an off-the-shelf stationary-noise restoration
pass, and it **failed on 6 of 6 while wrecking the audio** (segmental SNR 1.1 to 18.4 dB, mean NMR
above this model's masking threshold on two items). The mark is not noise-shaped and a denoiser does
not find it.

`overwrite_with_attacker_key` is the same story from the other side: an attacker who runs the
published embedder under a key of their own erases the victim's payload on 6 of 6, at 29.1 to
52.3 dB, and the file then carries the attacker's payload under the attacker's key. The detector
does not return the attacker's payload to the victim's key; it returns nothing. So overwriting is a
removal, not a substitution, and it costs exactly what an embed costs because it is one.

## Desynchronisation: where the knee actually is

The specification already declares pitch-preserving time stretch unsupported. That is not the useful
statement. The useful statement is how much is needed, because that number is what an attacker wants
and what a defender has to publish.


| attack | items losing the payload |
|---|---:|
| `playback_rate_0.05pct` | 0/6 |
| `playback_rate_0.1pct` | 0/6 |
| `playback_rate_0.2pct` | 0/6 |
| `playback_rate_0.3pct` | 1/6 |
| `playback_rate_0.4pct` | 2/6 |
| `playback_rate_0.5pct` | 2/6 |
| `playback_rate_0.75pct` | 3/6 |
| `playback_rate_-0.5pct` | 2/6 |
| `time_stretch_0.1pct` | 0/6 |
| `time_stretch_0.25pct` | 0/6 |
| `time_stretch_0.5pct` | 0/6 |
| `time_stretch_0.6pct` | 0/6 |
| `time_stretch_0.75pct` | 3/6 |
| `time_stretch_1pct` | 6/6 |
| `time_stretch_-1pct` | 6/6 |
| `pitch_shift_1_cents` | 0/6 |
| `pitch_shift_2_cents` | 0/6 |
| `pitch_shift_3_cents` | 0/6 |
| `pitch_shift_5_cents` | 2/6 |
| `pitch_shift_6_cents` | 2/6 |
| `pitch_shift_7_cents` | 3/6 |
| `pitch_shift_8_cents` | 3/6 |
| `pitch_shift_10_cents` | 6/6 |
| `pitch_shift_25_cents` | 6/6 |
| `pitch_shift_-10_cents` | 3/6 |
| `drop_1_sample_in_100000` | 0/6 |
| `drop_1_sample_in_10000` | 0/6 |
| `drop_1_sample_in_1000` | 0/6 |
| `drop_1_sample_in_200` | 2/6 |
| `random_warp_0.05pct_2s_segments` | 0/6 |
| `random_warp_0.2pct_2s_segments` | 0/6 |

The knees, on this corpus:

- **Pitch-preserving tempo change: nothing at 0.6%, half the corpus at 0.75%, all of it at 1%.**
  Symmetric: -1% also takes all six.
- **Tempo-preserving pitch shift: nothing at 3 cents, a third at 5, all of it at 10 cents.**
  A tenth of a semitone. The direction is not symmetric: -10 cents took 3 of 6 where +10 took all
  six, which is the rate grid being asymmetric about unity rather than anything about the audio.
- **Coupled playback-rate change is the most robust axis and never went to zero in the searched
  range**: nothing at 0.2%, one item at 0.3%, half at 0.75%. The rate grid is doing its job here.
- **Sample dropping is essentially free to survive.** One frame in a thousand, which is a 0.1% rate
  change delivered as 1440 discontinuities, cost nothing on 6 of 6. Only one in two hundred started
  to bite.
- **Random piecewise warping at ±0.05% and ±0.2% over 2 s segments cost nothing on 6 of 6.** The
  per-block re-anchoring absorbs it.

Pitch shift is the cheapest attack in this document by a wide margin. Ten cents is inaudible as a
pitch change to most listeners on most material without a reference, it is one ffmpeg filter, and it
takes the payload on every item.

None of the desynchronisation rows carries a perceptual number, and that is deliberate rather than
an omission. Every one of them changes the length or the time base, and a sample-aligned segmental
SNR against the marked file is not a defined quantity for a time-scaled signal: a pure 0.1% speed
change with no audible artefact at all reads as a large negative number because the two signals have
drifted apart. The honest report of the cost of these attacks is the parameter itself, in percent or
in cents, and the reader's own judgement about it.

## Collusion

Several differently marked copies of the same recording, under one key. This is a licensee,
screener or per-recipient forensic distribution, not a released track, and Audio Provenance does not
currently claim that use.


| attack | verdict | seg-SNR of the attack, dB | NMR mean, dB |
|---|---|---:|---:|
| `collude_average_2_copies` | survived (detector returns 110000a1000000) | 86.9 | -61.8 |
| `collude_average_3_copies` | substituted (detector returns 110000a1000065) | 82.2 | -54.2 |
| `collude_average_5_copies` | erased (detector returns nothing) | 79.4 | -50.0 |
| `collude_average_8_copies` | erased (detector returns nothing) | 78.5 | -47.9 |
| `collude_subtract_residual_x1` | erased (detector returns nothing) | 78.5 | -47.9 |
| `collude_subtract_residual_x1.6` | erased (detector returns nothing) | 75.8 | -43.8 |

Three things in that table.

**Averaging works, and it is nearly free.** Five copies erase the mark at a segmental SNR of 79 dB,
which is about 41 dB quieter than the mark itself. Averaging near-identical masters costs almost
nothing perceptually, which is what makes this row matter more than its removal count suggests.

**Three copies produced a substitution, not an erasure.** The detector, holding the correct key,
returned locator `0000a1000065`, which is a different colluder's payload, at confidence class
`strong`. It named a real colluder rather than an innocent party, so this is not a frame-up
primitive, but two of the three colluders walked and the tracing decision was arbitrary.

**Averaging copies that carry the SAME payload does not help an attacker and is not measured here.**
Same key and same payload produce a bit-identical embed, so the mean of such copies is the input;
there is nothing to average away. Collusion needs the payloads to differ, which is exactly the
condition per-recipient forensic marking creates.

## Forgery

The question is whether an attacker without the profile key can make audio the detector accepts.


| attack | attacker needs | detector accepted the planted payload | seg-SNR against the carrier, dB |
|---|---|---:|---|
| `transplant_coset_onto_other_audio` | one marked file | 6/6 | 29.4 to 52.0 |
| `transplant_from_one_block` | one marked file | 6/6 | 29.8 to 52.0 |
| `forge_chosen_payload_under_victim_key` | marked file + its payload | 6/6 | 29.9 to 52.1 |

All three rows succeeded on 6 of 6 items, every one at confidence class `strong` with 4 of 4 blocks
accepted, and the carrier in every case was a **different recording** from the donor.

- `transplant_coset_onto_other_audio` reads the residues off a marked donor and drives unrelated
  audio onto them. The forged file then decodes to the donor's payload. No key.
- `transplant_from_one_block` does the same from the first block alone: **8.87 s of donor audio at
  48 kHz is enough**. More blocks only clean up an estimate that is already at 0.9965 concentration.
- `forge_chosen_payload_under_victim_key` is the complete break. The attacker holds one marked file
  and knows the payload it carries. The convolutional code, the interleaver and the slot roles are
  published, so the coded bit of every data slot is computable; subtracting it from the observed
  residue leaves the key-derived dither, and any chosen payload can be written against it. Preamble
  and pilot slots carry key-derived polarity that no payload changes, so their observed residues are
  reused unchanged and never have to be separated into their parts. The result is a file, made of
  audio the attacker chose, that the victim's own detector accepts as carrying a locator the
  attacker chose.

**The premise of that last row is not exotic.** A registered release's locator is registry data. The
payload is `version | namespace | locator`, and version and namespace are constants. So for any
published, registered work, the attacker's required knowledge is: the file, and the registry.

The CRC is doing exactly what it was designed to do and it is not the thing that fails here.
Acceptance is gated by a 32-bit CRC-32C over the decoded message with at most `MAX_CRC_ATTEMPTS`
candidates examined per detection, which bounds a blind false accept near 6e-8 per run. **Measured
empirically: 0 accepts in 300 random-coset trials**, which is consistent with that bound and cannot
by itself confirm a rate that small. The assumption the bound rests on is that the attacker cannot
choose the payload, and the transplant and forgery rows are precisely the attacks that break that
assumption. A CRC stops a guess. It does not stop a copy.

## What an attacker can and cannot do

**Can, with the published specification alone:**

- Erase the payload on every item tested, by any of: a fixed ±1.95 dB equaliser of the published cell
  shape applied open loop with no analysis; a half-period offset of the statistic; a full-period
  random offset; a five-slot median flattening; or running the published embedder under a key of
  their own.
- Do it for a residual within 0.5 dB of what embedding the mark cost, on every item, by two of those
  five methods.
- Take the payload off every item with one ffmpeg filter: a 1% pitch-preserving tempo change or a
  10-cent pitch shift.

**Can, with one marked file:**

- Transplant the payload onto unrelated audio of their choice, from 8.87 s of donor.

**Can, with one marked file whose payload is public:**

- Forge an arbitrary payload under the victim's secret key, accepted at confidence class `strong`.

**Can, with several differently marked copies of one recording:**

- Erase the mark for a residual 41 dB below the mark itself, from five copies.
- Make a three-way collusion name one arbitrary colluder instead of all three.

**Cannot:**

- Recover the profile key itself. The dither recovery above yields the per-slot lattice offsets of
  one epoch, which is enough to forge under that key, but it is not the key and it does not
  generalise to another epoch, another namespace or another work marked under a different epoch.
- Get anywhere with an off-the-shelf denoiser. It failed on 6 of 6 and cost 20 to 50 dB of
  segmental SNR doing it.
- Beat the CRC by search. 0 accepts in 300 random-coset trials, against an arithmetic bound near
  6e-8 per detection.
- Break the mark with small timing damage. One dropped frame in a thousand, and random ±0.2%
  piecewise warping, both cost nothing on 6 of 6.
- Produce a `verified` verdict. Nothing here touches the signature, the hash-chain hard binding, or
  the signed reference constellation.

## What this does not measure

Nothing in this document exercises the layers above the watermark, and those layers are where a
verdict actually comes from. Read from `STATUS.md` and `docs/GENOTRACE_SPEC.md` 3.2a, and stated
here as read-from-spec rather than as a measurement made in this campaign: a transplanted or forged
file has no matching hash-chain hard binding and no matching signed reference constellation, so
route 3(c) cannot reach `verified` for it; a payload that decodes while the constellation disagrees
is the watermark-copy attack and is mapped to `changed`. That mapping was not exercised by this
campaign and should not be reported as measured until it is. It is also the single most valuable
follow-up: drive a forged file end to end through `audio-provenance verify` against a real registered
record and confirm the verdict.

Also unmeasured here: whether a forged or erased file survives a subsequent lossy transcode; every
attack in this document was measured on uncompressed audio.

## Perceptual figures: what they are and are not

Segmental SNR is an energy ratio per short frame. It says how much smaller a residual is than the
signal and nothing about whether a listener can hear it. The masking measure is a noise-to-mask
ratio from a Schroeder spreading function over a half-Bark partition with a fixed tonality
assumption and an absolute-threshold curve anchored on digital full scale being 96 dB SPL. Both are
measured per channel and reported for the worst channel. Neither is PEAQ, neither yields an ODG, and
neither is a listening test. The full statement travels in the report JSON as
`perceptual_limits`, verbatim from `audio_provenance_bench::perceptual::PERCEPTUAL_LIMITS`.

The comparison this document rests on is a **relative** one: the residual an attack leaves against
the residual the mark itself leaves, measured the same way on the same file. That comparison is
robust to the model's absolute assumptions in a way that a claim of inaudibility would not be.

## Reproducing this

```
cargo build --release -p audio-provenance-attack
C=corpus/characterisation
F="--file $C/real_electronic_dense.wav --file $C/real_hiphop_dense.wav \
   --file $C/real_pop.wav --file $C/real_rock_loud.wav \
   --file $C/real_orchestral.wav --file $C/real_piano_solo.wav"
target/release/audio-provenance-attack probe    --items 6 $F
target/release/audio-provenance-attack campaign --items 6 --collusion-copies 8 \
    --out bench-out/attacks/report.json $F
target/release/audio-provenance-attack null     --items 6 --null-trials 300 $F
```

The campaign wrote 276 rows in 264 s and the null arm ran 300 trials in 258 s, on 30 s stereo
48 kHz excerpts of six real recordings, all six of which decode `strong` with 4 blocks accepted
before any attack. The victim key is generated inside the process and never leaves it; every row
marked `spec only` or `one marked file` runs without it.

Code: `crates/audio-provenance-bench/src/attacks` (the attacks, which know nothing about GenoMark) and
`crates/audio-provenance-attack` (the wiring, which reads only published items out of `genomark`). The
invariant that every closed-loop attack reaches the shift it asked for is pinned by
`crates/audio-provenance-bench/tests/attack_invariants.rs`.

Measured 2026-08-31 against `crates/genomark` at source digest
`906e7803d539641d47d310dbefcd7fe6d2958106e057479e3d38cc2fba9df152`
(`find crates/genomark/src -name '*.rs' | sort | xargs shasum -a 256 | shasum -a 256`), which did not
change during the run.
