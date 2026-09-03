# audio-provenance-capture-rig

Stage 0 of the GenoMark-N programme: the physical speaker-to-microphone capture campaign specified in
`docs/GENOMARK_N_SPEC.md` section 10, built before any serious training investment because it is what
decides whether the programme continues.

**The number this exists to produce does not exist anywhere.** Every physical acoustic watermark
result in the published literature is an oracle best-of-search maximum computed with knowledge of the
true bits. DeAR shifts the capture over a 113 ms range and keeps the best. DeepAWR sweeps 6000 sample
offsets, scores each against the known payload, breaks early on a perfect hit, and computes the CRC
only to discard it. No licensable method publishes a **blind, CRC-gated detection rate on a physical
path at any distance**. This tooling measures one, with its false-positive bound and the trial count
that bounds it.

## Why Python

The language was chosen by what Stage 0 must do, not by preference. Spec 10.3 makes this campaign
responsible for running AudioSeal and WavMark blind over the captures to produce the first blind
physical baselines for any licence-clean watermark, and both are PyTorch-only. A Rust rig would shell
out to Python for the one number the deliverable exists to produce. `sounddevice` (PortAudio) gives
CoreAudio enumeration and synchronous full-duplex; numpy and scipy give the sweep mathematics. The
tool adds no crate and does not touch the workspace `Cargo.toml`.

## The firewall between alignment and detection

**Alignment is dataset bookkeeping. It never touches detection.** This is the single most important
property of the tool and it is enforced, not described.

`capture_rig.alignment` locates a known source clip inside a capture, using the source clip. That is
ground truth. Anything downstream of it is oracle-contaminated by construction, which is exactly the
defect that invalidates every published physical number.

`capture_rig.detect` receives the raw capture and a threshold record frozen before the run, and
nothing else — no source clip, no payload, no arm label, no room, no alignment. The separation is
asserted by `tests/test_oracle_firewall.py`, which imports the detection path in a fresh interpreter
and fails if `capture_rig.alignment` appears anywhere in its transitive import graph. A second test
guards the guard by proving the probe would notice. `capture_rig.cli` imports the alignment package
lazily inside the `align` subcommand alone, so every other subcommand runs in a process where the
alignment code is not even loaded.

## Commands

```
capture-rig devices                  list CoreAudio devices PortAudio can open
capture-rig plan       --campaign …  expand the matrix; what is done, what is owed, how many hours
capture-rig calibrate  --campaign …  1 kHz tone; verify playback SPL and capture gain, no clipping
capture-rig run        --campaign …  take every pending capture, resuming over what is complete
capture-rig ingest     --campaign …  register a phone recording as a first-class trial
capture-rig analyse-sweeps --campaign …  deconvolve stored sweeps into IRs plus manifest.jsonl
capture-rig k0         --campaign …  evaluate kill criterion K0 from the measured responses
capture-rig analyse-ir --ir …        RT60, octave-band RT60 and DRR of one impulse response
capture-rig align      --capture …   locate known clips in a capture (BOOKKEEPING ONLY)
capture-rig report     --campaign …  run a detector BLIND over the captures and report
```

`k0` and `report` exit 2 when a kill criterion fires, 1 on error, 0 otherwise.

## What the report is

`report` emits `audio-provenance-capture-rig/1`, whose rows deliberately mirror `crates/audio-provenance-bench`'s so
the numbers sit beside its simulated acoustic rows without translation: `channel`, `family`, `params`,
`simulated_physical_path`, `exact_recovery_rate`, `payload_returned_rate`, `mean_bit_error_rate`,
`mean_detect_seconds`, `false_positive{trials,accepts,rate}`, and per-trial rows. Channels are named
`physical_<room>_<speaker>_<mic>_<distance>` per spec 10.4.

The schema id is **not** the bench's. A different producer emitting `audio-provenance-bench/1` would be a
provenance lie, and spec 10.4 makes the physical arm a proposal to that crate's owner rather than an
edit made here. The disclaimers are the inverse of the bench's: this IS a speaker-to-microphone
measurement, and zero accepts over N unmarked trials bounds the false-positive rate at 3/N, never at
zero.

`report` also evaluates K4 (locator tier), K5b (presence tier) and K6 (distance honesty). A criterion
whose coverage the campaign does not yet meet reads `unevaluated` with the gaps listed. A kill verdict
stops the programme, so it is never reachable from a dataset too thin to support one.

## Detectors

A detector is anything with `detect(audio, thresholds) -> Detection` — the same signature
`training/src/genomark_n/pipeline.py` already uses.

```
--detector null                             declines everything; the pipeline's own control
--detector python:my_pkg.mod:make_detector  factory(sample_rate=…, options=…) -> object with .detect
--detector exec:'/path/to/detect --flag'    external process over a stdout JSON contract
```

The `exec` adapter copies each capture to a temporary file named `trial.wav`, carrying no room,
speaker, distance, arm or clip identifier, because a detector that could read the arm out of a path
would not be blind.

Run `--detector null` over a finished campaign before running a real one. It must report
`exact_recovery_rate` 0.0 and `false_positive.rate` 0.0; anything else means the report arithmetic is
manufacturing detections.

## Measurement conventions, and where they diverge

These are the conventions kill criterion K0 is evaluated under, so they are named rather than implied.
Each is recorded in every measurement JSON.

- **RT60** — Schroeder backward integration, T30 fitted between −5 dB and −35 dB and extrapolated to
  60 dB, with Lundeby-style noise truncation, **band-limited to 100 Hz – 8 kHz**. The band-limiting is
  load-bearing: a swept-sine deconvolution returns a band-limited impulse whose skirt below the
  sweep's start frequency rings for 1/f_start seconds, and left in it dominates the Schroeder tail. On
  a response designed for RT60 0.45 s, broadband T30 on the raw deconvolution reads **1.39 s** and the
  same response band-limited reads **0.43 s**. Octave-band T30 is reported alongside, and Tmid (the
  mean of the 500 Hz and 1 kHz bands) with it.
- **Measurement quality is a field, not an assumption.** Every RT60 carries `noise_limited` (the
  usable decay ended before half the fitted RT60) and `poor_fit` (decay-curve r² below 0.95), and
  `trustworthy` is the conjunction. A 3 s 50 Hz–20 kHz sweep reads 0.63 s at r² 0.84 on a response
  designed for 0.30 s, where a 10 s 20 Hz–20 kHz sweep reads 0.30 s. Use a 10 s sweep.
- **DRR** — the ACE Challenge convention: a ±2.5 ms window around the direct peak against everything
  after it, on the same 100 Hz – 8 kHz band. **This is not the split the training tree uses.**
  `RirPair.measured_drr_db()` in `training/src/genomark_n/rir/image_source.py` separates image-source
  order 0 from orders ≥ 1, which is unobservable on a measured response. On that synthesiser's own
  output the ACE figure reads about **+2.4 dB above** the order-split target and tracks it one for
  one, so a DRR-parameterised training distribution maps monotonically onto measured DRR with a stated
  offset. `tests/test_acoustics.py` pins that relationship.
- **DRR from a sweep is a property of the band-limited response.** An ideal single-sample direct
  arrival is not physically realisable and its DRR is not comparable to a measured one; a real
  transducer band-limits the direct arrival, and so does the sweep. Compare measured DRR to measured
  DRR.
- **A-weighting** — the IEC 61672-1 transfer function evaluated analytically over a Welch power
  spectrum, not a bilinear-transformed digital filter, which is 1.2 dB low at 10 kHz at 48 kHz and
  outside class 1 tolerance.
- **SPL** — reported only where an operator sound-level-meter reading anchors it. Without
  `spl_calibration` on the room, `spl_dba` is `null` with a reason. Spec section 10 exists because the
  published field reports SPL nowhere; a fabricated figure is worse than a missing one.

## Resume

A campaign runs for days and gets interrupted. Resume keys on the integrity of what is on disk, never
on a counter: a cell counts as done only when its sidecar exists and the WAV it names still has the
declared frame count and byte size. A crash mid-write leaves a short WAV, which fails that check and
re-queues. `--verify-digests` additionally re-hashes the stored PCM.

Sweep captures record the stimulus they actually used, and `analyse-sweeps` and `k0` rebuild the
inverse filter from that record rather than from the campaign file's current `sweep:` block. Editing
that block after a room has been swept would otherwise reanalyse every stored capture against the
wrong inverse filter, and the result is not an error — it is a plausible-looking wrong RT60.

## Ownership boundaries

This directory is owned by the capture-rig workstream. It creates no paths outside itself, adds no
crate, and does not edit the workspace manifest.

It duplicates about twenty lines of Schroeder integration that also exist in
`training/src/genomark_n/rir/image_source.py`. That is deliberate: a path dependency on a
concurrently-edited package would break K0's evaluator whenever that package is refactored. The two
implementations are cross-checked in `tests/test_acoustics.py`, which agrees with the training tree's
`measure_rt60` to within 2% on the same convention and skips when that tree is absent.

The impulse-response corpus this rig writes is consumed by the training tree, so
`tests/test_training_interop.py` constructs a `RirCorpus` over this tool's own output inside the
training project's environment. Spec 9.2 ranks Audio Provenance's own measured responses first among IR
sources; that claim is checked rather than asserted.

## What is tested, and what is not

`uv run pytest` — 73 tests, all offline.

Verified with no hardware:

- Sweep generation, the Farina inverse filter, and deconvolution against synthetic impulse responses
  with designed answers: discrete tap recovery against the band-limited reference (max abs error
  < 1e-6), bulk delay to within 2 samples, RT60 to within 5% of a designed 0.45 s, DRR to within 1 dB.
- Harmonic pre-arrival at dt_k = T·ln(k)/ln(f2/f1), which also proves the deconvolution is linear
  rather than circular and doubles as the THD measurement that parameterises distortion-layer stage D4.
- A-weighting against the IEC 61672 table at five frequencies, to within 0.15 dB.
- Config validation, matrix expansion, channel naming, the per-stage time estimate.
- Resume: a truncated capture re-queues, an intact one does not, an altered PCM digest is caught.
- K0, K4, K5b and K6 verdict logic, including that a thin dataset reports `unevaluated` and never
  `kill`.
- The blindness contract, asserted on the detector call itself, and the alignment/detection import
  firewall.
- Alignment offset recovery through a reverberant path, to within 2 samples.
- The RIR manifest loading in the training tree's licence-gated loader.
- The CLI end to end over every offline subcommand.

**Requires hardware and is therefore unexercised here:** opening a PortAudio full-duplex stream and
its timing; real playback SPL and background dBA; real RT60 and DRR values and so K0's actual verdict;
the marked and unmarked physical corpora; every number in the report. Device *enumeration* is
exercised on this machine — PortAudio lists six CoreAudio devices — but not the specific rig devices,
which do not exist yet.

## Next

`docs/BOM.md` — what to buy. `docs/RUNBOOK.md` — how to set the rooms up, how to run the campaign, and
how long it takes.
