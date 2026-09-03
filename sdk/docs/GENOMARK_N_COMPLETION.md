GENOMARK-N COMPLETION GUIDE
Written 2026-08-31 for an engineer who was not here. Working document, not a report.

The design is fixed in /Volumes/A/audio-provenance/sdk/docs/GENOMARK_N_SPEC.md (843 lines) and this guide
does not restate it. Read the spec first. This document is the DELTA between that spec and what is
actually on disk, plus the order the remaining work has to be done in, plus the exact commands that
decide whether the program continues.

Every number quoted below was produced by running the command printed next to it on this machine on
2026-08-31, or read out of a file whose path is given. Where a number came from another agent's run
rather than mine, it says so.

=====================================================================
0. THE ONE-PARAGRAPH STATE OF THE PROGRAM
=====================================================================
The scaffolding is done and the science has not started. A PyTorch training harness, a Rust
inference crate, an ONNX contract binding them, licence gating enforced in code, and a type system
that makes it structurally impossible to report an acoustic capability without a validated
measurement all exist and all run. What does not exist: a trained model, a licence-cleared audio
corpus, a measured impulse-response corpus, any physical capture hardware, any captured audio, and
therefore every number that decides whether GenoMark-N is real. A Stage 0 capture-rig package landed
mid-way through the writing of this guide and is still being built; section 2.1 is a timestamped
snapshot and the first thing you should do is re-inventory it. `capabilities().acoustic_rerecording`
is the literal `"unsupported"` and nothing in this tree can change that. The next thing to do is not
a training run. It is the capture campaign.

=====================================================================
1. WHAT IS DONE, AND THE COMMAND THAT DEMONSTRATES IT
=====================================================================
1.1 RUST INFERENCE CRATE: /Volumes/A/audio-provenance/sdk/crates/genomark-n/ (3,188 lines)

Loads a model card, verifies the pinned SHA-256 of each graph, builds two ONNX sessions with the
declared shapes, runs the Rust-side STFT, runs a dense shift-equivariant presence pass in bounded
chunks, pools the message over the presence-gated span, and runs the fixed 11-trial CRC flip search.
Implements `audio_provenance_bench::watermark::WatermarkCodec` so the existing bench matrix can measure it
with no bench changes.

    cd /Volumes/A/audio-provenance/sdk
    CARGO_TARGET_DIR=/Volumes/C/rust-target cargo test -p genomark-n
    # measured: 22 passed

    CARGO_TARGET_DIR=/Volumes/C/rust-target cargo clippy -p genomark-n --all-targets -- -D warnings
    # measured: clean, exit 0

    CARGO_TARGET_DIR=/Volumes/C/rust-target cargo run --quiet -p genomark-n --example run_card -- \
      crates/genomark-n/fixtures/genomark-n-fixture-v1.card.json 12
    # measured output:
    #   model_id        genomark-n-fixture-v1 epoch 0 fixture true
    #   acoustic        unsupported
    #   band            210.9 Hz .. 7687.5 Hz, 56 message bits, can_embed true
    #   detect(cover)   class None presence_score 0.152721 positive 0.000 s frames 1129
    #   shift 137 samp  presence 0.152721 vs 0.152197, delta 5.245e-4
    #   embed           576000 frames out, peak residual 0.068660 (-23.3 dBFS)
    #   detect(marked)  class None presence_score 0.143642

`class None` is the CORRECT result. The fixtures are hand-written arithmetic graphs with no learned
parameters (`crates/genomark-n/tools/make_fixture_models.py`). They prove the plumbing executes and
nothing else.

DO NOT run `cargo test --workspace` to check this crate. Five crates in this workspace are being
edited concurrently by another workflow and a failure there says nothing about GenoMark-N. The
workspace figure of 144 passing tests in /Volumes/A/audio-provenance/sdk/STATUS.md is another agent's
measurement of a different tree state; the 22 above is mine, scoped to `-p genomark-n`.

Load-bearing invariants already enforced by the type system, not by convention:

  - `crates/genomark-n/src/envelope.rs`: `AcousticEnvelope` has NO public constructor and
    `EnvelopeClaim::validate` is `pub(crate)`. `AcousticRerecording::MeasuredLimited(AcousticEnvelope)`
    therefore cannot be named from outside the crate. A build without a model card whose envelope
    passes every K-gate is STRUCTURALLY incapable of reporting anything but `"unsupported"`.
    The gates are coded at envelope.rs lines 140-193 and referenced by K number.
  - `crates/genomark-n/src/lib.rs` lines 68-76: no function in the crate takes an expected payload,
    a time offset, or a threshold as a detection input. The oracle-search bug is unrepresentable,
    not merely avoided.
  - `crates/genomark-n/src/thresholds.rs`: every decision boundary comes from the model card. There
    is no call-time threshold argument anywhere in the public API.
  - `crates/genomark-n/Cargo.toml` + `lib.rs` `compile_error!`: `download-binaries` and
    `load-dynamic` are mutually exclusive, so `--all-features` is not a valid configuration for this
    crate. The second gate command is
    `cargo clippy -p genomark-n --all-targets --no-default-features --features bench,load-dynamic -- -D warnings`.

DECLARED DIVERGENCE FROM THE SPEC, ALREADY WRITTEN DOWN: spec 7.1 names `candle` + `.safetensors` as
the SHIPPING runtime with `ort` as an offline cross-check. This crate implements the `ort` path, and
`download-binaries` is exactly what 7.1 rules out for a shippable SDK. The candle port and the 7.4
parity gate (row N-B12) are Stage 3 work this crate does not close. See `crates/genomark-n/README.md`
"Runtime divergence from the spec".

1.2 PYTHON TRAINING HARNESS: /Volumes/A/audio-provenance/sdk/training/ (5,617 lines)

    cd /Volumes/A/audio-provenance/sdk/training
    uv run pytest -q
    # measured: 35 passed in 15.28s

    ./scripts/overfit_check.sh
    # one clip, one payload, DISTORTION CHAIN BYPASSED. Another agent measured
    # final bit accuracy 1.0000, CRC decoded True, bits_corrected 0, RESULT PASS.
    # This is spec 9.3's Phase A precondition in three minutes. It proves the wiring
    # encode -> budget -> encoder -> gain -> ISTFT -> STFT -> decoder -> pooled logits -> CRC
    # is correct. It says nothing about whether the mark survives anything.

    ./scripts/smoke.sh                                   # ~3 min, every stage runs
    ./scripts/loop_closure.sh configs/smoke_300.yaml runs/smoke300 300
    # PyTorch -> ONNX -> Rust, end to end. Artifacts already on disk in
    # /Volumes/A/audio-provenance/sdk/training/runs/smoke300/

Modules, all present and all exercised by the smoke path: `config.py` (every spec hyperparameter
typed, hard bounds enforced), `stft.py` (the transform that lives OUTSIDE the graph, spec 7.2),
`perceptual.py` (torch port of `crates/audio-provenance-bench/src/perceptual.rs` plus the budget `B[t,f]`),
`payload.py` (56-bit message, CRC-24/OPENPGP, the fixed 11-trial flip search), `model.py`
(frequency-only-downsampling U-Net encoder, two-head decoder), `qmark.py` (GenoMark-Q's statistic as
distortion stage D0 and the `L_q` hinge), `pipeline.py` (Marker, losses, `FrozenThresholds`, blind
`Detector`), `channels.py` (eval channels mirroring audio-provenance-bench), `rir/` (image-source synthesizer
with a measured-decay correction), `data/` (licence gating, manifests, synthetic corpus),
`distortion/` (spec 6's D0-D11 with a real ffmpeg round trip), `rust_card.py` (the loader card the
Rust crate reads), plus `train/calibrate/evaluate/export_onnx/parity/model_card/overfit_check`.

1.3 THE CONTRACT BETWEEN THEM

/Volumes/A/audio-provenance/sdk/training/docs/ONNX_CONTRACT.md is the prose; `export_onnx.py` emits
`onnx_contract.json` beside the weights as its machine-readable twin; `crates/genomark-n/src/lib.rs`
lines 26-57 carries the numbered clause list; `ModelCard::validate` checks the card's `transform`
block against this build's constants and refuses a mismatch. The detail that gets silently wrong:
the lead pad is 2048 samples, the WINDOW LENGTH, not 1024. `torch.stft(center=True)` pads 1024 and
would put every frame index four hops out of step while both sides claimed agreement.

1.4 LICENCE GATING, ENFORCED IN CODE

/Volumes/A/audio-provenance/sdk/training/src/genomark_n/data/licences.py. `ALLOWED_LICENCES` is an
allowlist; `DENIED_SOURCES` names FMA (unfiltered), OpenSLR-28, Aachen AIR, SilentCipher weights,
DeepAWR, DeAR and Timbre Watermarking with the reason for each. A manifest entry outside the
allowlist RAISES rather than being skipped, because a silent skip lets a corpus shrink to nothing
and a run report a result over data nobody checked. `training/tests/test_licences.py` pins it.
`crates/genomark-n/NOTICE` and `training/NOTICE` record what was and was not vendored, with the
licence position read from the primary artifact in each case. Nothing third-party is vendored.

1.5 MEASURED FACTS THAT ARE ALREADY WORTH CARRYING FORWARD

  (a) MODEL SIZE, K7's second arm: 8,229,523 parameters (encoder 5,343,681, decoder 2,885,842),
      32,918,092 bytes fp32, against K7's 41,943,040-byte ceiling. K7 size PASSES. Read from
      /Volumes/A/audio-provenance/sdk/training/runs/smoke300/model_card.json `architecture.parameters`.
      This is 8.23 M against spec 2.8's estimate of ~5.5 M; taking the channel widths of spec
      2.5/2.6 literally gives the larger number. Accept it or narrow `encoder.stage_channels`.

  (b) WALL CLOCK, K8's hardware arm: `seconds_per_step` 10.687, `projected_320k_days` 39.58, at
      batch 2 on CPU, from the last line of
      /Volumes/A/audio-provenance/sdk/training/runs/smoke300/train_log.jsonl. K8 fires at 14 days.
      IT FIRES TODAY. See section 4.

  (c) THE KAPPA TRAP: on an UNTRAINED encoder every kappa passes K2, because an untrained encoder
      does not use its budget. Verified by me just now:

          cd /Volumes/A/audio-provenance/sdk/training
          uv run python -m genomark_n.calibrate --config configs/smoke_300.yaml \
            --checkpoint runs/smoke300/checkpoints/step_00000300.pt \
            --out /tmp/kappa_sweep.json --kappa-sweep 0.1,0.2,0.5
          # kappa 0.100 nmr_max -40.12 dB above_mask 0.000 segSNR 54.86 dB K2 pass
          # kappa 0.200 nmr_max -33.65 dB above_mask 0.000 segSNR 49.16 dB K2 pass
          # kappa 0.500 nmr_max -26.05 dB above_mask 0.000 segSNR 43.48 dB K2 pass

      The overfit check, where the encoder IS trained to saturate its budget, lands at segmental SNR
      14.87 dB at kappa 0.5, UNDER K2's 22 dB floor (another agent's measurement, recorded in
      training/README.md). Conclusion: kappa must be calibrated against a CONVERGED encoder, never
      an initial one, and `L_perc` must be doing real work rather than sitting at zero. Expect the
      shipped kappa nearer 0.2 than 0.5.

  (d) DETECT COST, an early and encouraging signal with caveats: `mean_detect_seconds` 0.4380 to
      0.4617 for a 4.0 s clip across three channels in
      /Volumes/A/audio-provenance/sdk/training/runs/smoke300/eval.json, i.e. RTF about 0.11x, on the FULL
      8.23 M-parameter architecture. K7's runtime arm fires above 0.5x. Caveats: this is PyTorch on
      CPU, not the Rust `ort` path K7 actually names, and the dense presence chunking in
      `crates/genomark-n/src/params.rs` (`DENSE_CHUNK_SECONDS` 60.0, margin 2.0) changes the cost
      profile at 30 s. Treat 0.11x as "K7 runtime is probably not the binding constraint", not as a
      pass.

=====================================================================
2. WHAT IS NOT DONE
=====================================================================
2.1 THE CRITICAL PATH: THE CAPTURE RIG IS UNDER ACTIVE CONSTRUCTION. RE-INVENTORY IT BEFORE
    TRUSTING ANYTHING BELOW.

WARNING ON A MOVING TARGET. When I began this guide at roughly 18:00Z on 2026-08-31,
/Volumes/A/audio-provenance/sdk/tools/ DID NOT EXIST at all (`ls` returned "No such file or directory" and
`find -iname "*capture*"` returned nothing). By 19:19Z a concurrent workflow had landed
/Volumes/A/audio-provenance/sdk/tools/capture-rig/ and was still adding modules WHILE I WAS LOOKING: three
paths (`calibration.py`, `alignment/`, `detect/`) appeared between two listings sixty seconds apart.
Everything in this subsection is a snapshot at 19:19:47Z. FIRST COMMAND YOU RUN:

    date -u +"%Y-%m-%dT%H:%M:%SZ"
    find /Volumes/A/audio-provenance/sdk/tools/capture-rig -type f -not -path "*/.venv/*" -not -name "*.pyc" | sort
    cd /Volumes/A/audio-provenance/sdk/tools/capture-rig && uv sync && uv run pytest -q

WHAT EXISTED AT 19:19:47Z, 1,706 lines across 13 files:

  `pyproject.toml`   package `audio-provenance-capture-rig`, deps numpy / scipy / soundfile / sounddevice /
                     pyyaml, console script `capture-rig = "capture_rig.cli:main"`.
  `signals.py`       `SweepPair`, `exponential_sweep()`, `sweep_stimulus()`, raised-cosine fades.
  `sweep.py`         `deconvolve()`, `extract_response()`, `measure_response()`, `HarmonicPacket`.
                     This is the Farina inverse-filter path, with harmonic-distortion packets
                     separated out rather than folded into the linear response.
  `acoustics.py`     `rt60_t30()` (Schroeder backward integration with Lundeby noise truncation, T30
                     extrapolated to 60 dB), `octave_band_rt60()` (bands above Nyquist SKIPPED, not
                     fabricated), `drr_db()` (ACE convention, returns None when there is no
                     reverberant energy), exact IEC 61672 A-weighting, and `dba_from_dbfs()` which
                     converts a capture-side dBFS reading to dB SPL(A) from one operator meter
                     reading. Its module docstring states outright that K0 is defined on MEASURED
                     RT60 and MEASURED DRR and that these are the conventions K0 is evaluated under.
  `config.py`        436 lines. `Room`, `Speaker`, `Microphone`, `SplCalibration`, `SweepConfig`,
                     `Stage`, `ClipSources`, `Campaign`, with identifier validation. Its docstring
                     cites K4's >= 3 rooms x >= 2 speakers x >= 2 microphones directly.
  `campaign.py`      `CaptureTask`, `expand_stage()`, `expand()`, `estimate_seconds()`, and the
                     `physical_<room>_<speaker>_<mic>_<distance>` token builders.
  `devices.py`       `list_devices()`, `resolve()`, `describe_pair()` over sounddevice.
  `playback.py`      `play_and_record()`, `record_only()`.
  `record.py`        `condition_record()`, `level_report()`, per-capture metadata.
  `state.py`         `CaptureStore`, `CellStatus`, atomic JSON writes. Resumable campaign state.
  `audio.py`         I/O helpers.
  `calibration.py`, `alignment/`, `detect/`   appeared mid-inventory; unread, and `alignment/` and
                     `detect/` were still empty directories at 19:19:47Z.

WHAT DID NOT EXIST AT 19:19:47Z, AND WHAT I THEREFORE COULD NOT VERIFY:

  - `src/capture_rig/cli.py`. The console script `capture-rig` declared in `pyproject.toml` points
    at `capture_rig.cli:main`, and that module was absent. THE PACKAGE HAD NO RUNNABLE ENTRY POINT.
  - `tests/`. The directory existed and was EMPTY, so `[tool.pytest.ini_options] testpaths =
    ["tests"]` collects nothing.
  - `README.md` was a 3-line placeholder reading "See README body written at the end of the build".
  - I ran NO command in this package. Every description above is read from source, not executed.
    Do not treat any of it as demonstrated.

WHAT TO CHECK BEFORE RELYING ON IT, in this order:
  1. Does `capture-rig --help` run? If not, the CLI is still missing and the modules are a library
     with no driver.
  2. Does `uv run pytest -q` collect and pass anything? An empty `tests/` is the single biggest risk
     here: `rt60_t30`, `drr_db` and `deconvolve` are the functions K0 is decided on, and a sign
     error or an off-by-one in the Schroeder integration produces a plausible wrong number rather
     than a crash. Pin them against a SYNTHETIC IR with a known decay before trusting a measured
     one, and pin `deconvolve` on a known-delay impulse.
  3. Is the UNMARKED arm of spec 10.2(e) in the campaign matrix? `campaign.py` has `Stage` and
     `expand_stage()`; confirm one of the stages captures unmarked audio at equal size. The
     false-positive corpus "is not an afterthought and cannot be synthesised later", so a campaign
     that omits it has to be re-run in full.
  4. Are the blind AudioSeal and WavMark baselines of spec 10.3 covered? `detect/` was an empty
     directory; that is presumably where they go.

CONSEQUENCE FOR THE KILL CRITERIA. K0's arithmetic (RT60 T30, octave-band RT60, DRR) is IMPLEMENTED
and untested. K4, K5b and K6 need the campaign to have actually been run, which needs hardware,
which is a procurement and scheduling problem this tree cannot solve. Treat K0 as "runner exists,
unverified" and K4 / K5b / K6 as "no data exists" rather than "no code exists".

2.2 NO TRAINED MODEL AND NO CORPUS

The only checkpoints on disk are /Volumes/A/audio-provenance/sdk/training/runs/smoke300/checkpoints/, a
300-step run at batch 2 over SYNTHESIZED audio, three orders of magnitude short of spec 9.5's
320k-step schedule. Its `training_provenance.status` says so. Its eval.json reports
`exact_recovery_rate` 0.0 and `presence_recall` 0.0 on every channel, which is the correct result for
an untrained model and is not a measurement of anything.

`configs/phase_c.yaml` has `data.manifests: []`, `data.noise_manifests: []` and
`rir.corpus_dirs: []` because no licence-cleared corpus is on this machine. Filling those in is
section 4.3.

2.3 NO PHYSICAL ARM IN THE RUST BENCH, AND THAT HALF IS NOT YOURS TO WRITE

`crates/audio-provenance-bench/src/channel/mod.rs` line 18: `ChannelFamily` has ten variants and none is
`MeasuredPhysical`. `is_simulated_physical_path()` is consulted at `runner.rs:234`, `372` and `485`.
Spec 10.4 explicitly makes the bench change A PROPOSAL TO THE OWNER OF `audio-provenance-bench`, not an edit
this program makes, and the same holds for `Thresholds::product_targets()`, which currently declares
all three acoustic rows `ExpectedFailure`. Write the proposal; do not land the edit unilaterally.

2.4 NO GENOMARK-N BENCH BINARY

`crates/genomark-n/src/bench.rs` implements `WatermarkCodec` correctly, but nothing runs the matrix
against it. `crates/audio-provenance-bench/src/bin/audio-provenance-bench.rs` `select_codec()` accepts only
`fixture_lsb16`, `fixture_always_accept`, `fixture_silent`. GenoMark-Q solved this by shipping its
own binary at `crates/genomark/src/bin/genomark-bench.rs`. The analogous file to create is
`/Volumes/A/audio-provenance/sdk/crates/genomark-n/src/bin/genomark-n-bench.rs`, modelled on the Q one, with
a `--card PATH` argument for the model card. Note the payload constraint documented at
`crates/genomark-n/src/lib.rs` lines 59-66: the decoder refuses any version but its own, so
`audio-provenance-bench`'s default payload will NOT work and the binary must construct one whose leading 3
bits are `audio_provenance_core::GENOMARK_PAYLOAD_VERSION`.

2.5 NO CHAIN CHANNEL, SO K9's FIRST ARM CANNOT BE RUN

`training/src/genomark_n/channels.py` `ChannelBank.build()` handles `identity`, the four room
presets, `mp3_*`/`aac_*`/`opus_*`, `resample_via_*`, `gain_*`, `noise_snr_*`, `lowpass_*` and
`drift_*`. There is no chain combinator. Verified:

    cd /Volumes/A/audio-provenance/sdk/training
    uv run python -c "
    from genomark_n.channels import ChannelBank
    from genomark_n.config import load_config
    c = load_config('configs/smoke_300.yaml')
    b = ChannelBank(c.stft.sample_rate, c.codec, c.rir)
    print(b.build('drift_plus_0p1pct').params)
    b.build('chain_drift_plus_0p1pct_acoustic_1m_room')"
    # {'rate_ratio': 1.001}
    # KeyError: "unknown channel 'chain_drift_plus_0p1pct_acoustic_1m_room'"

K9 requires `drift_plus_0p1pct` CHAINED with the 1 m acoustic row. Add a `chain_a+b` form to
`build()`. It is a small change and it is a prerequisite for a kill criterion.

2.6 NO DURATION SWEEP, SO K9's SECOND ARM HAS NO SINGLE COMMAND

`evaluate.py` takes `clip_seconds` from the config and has no CLI override. Bench row N-B7's
monotonicity check over 5, 10, 20, 30 and 45 s is therefore five config copies and a hand
comparison. Either add `--clip-seconds` to `evaluate.py` or write the sweep as a script. Nothing
asserts monotonicity today.

2.7 NO K3 MEASUREMENT PATH

Three arms, none runnable:
  (a) Q's matrix with and without N present. `crates/genomark/src/bin/genomark-bench.rs` runs the Q
      matrix once, over cover audio. Nothing wires an N embed in between. Needs a flag on the Q
      binary or a wrapper that N-marks the corpus first.
  (b) `std(D_marked - D_cover) < 0.04` nepers on Q's statistic.
      `training/src/genomark_n/qmark.py` implements the statistic and the `L_q` hinge, but it has no
      CLI: it is only reachable from inside the training loss. The 0.04 bound is
      `OLA_RESIDUAL_LIMIT` at `crates/genomark/src/params.rs:29` (`DELTA / 20.0`).
  (c) C0 re-run with N present. `DELTA` is a hardcoded `0.8` at `crates/genomark/src/params.rs:24`.
      The C0 harness that produced it is not in this tree, so this arm needs the measurement
      rebuilt before it can be compared against an N-present run.

2.8 NO GRADIENT-FLOW TEST THROUGH THE DISTORTION CHAIN

`grep -rn "backward\|requires_grad" /Volumes/A/audio-provenance/sdk/training/tests/` returns only two
`torch.no_grad()` hits in `test_equivariance.py`. Nothing asserts that a gradient reaches the
encoder through D11's identity-gradient codec round trip. `overfit_check.sh` BYPASSES the chain
entirely, so it structurally cannot catch this. See section 5.1: this is the failure that bites
first and it currently has no detector.

2.9 N-B12 PARITY IS PREPARED AND NOT RUN

`genomark_n.parity` writes the reference side as raw little-endian f32 blobs plus
`parity_manifest.json`. Nothing on the Rust side reads them back. The gate (max abs error < 1e-3,
100% accept/reject agreement over 200 items x 6 channels) is Stage 3.

2.10 SUMMARY TABLE

  | piece                                   | state        | blocking                    |
  |-----------------------------------------|--------------|-----------------------------|
  | capture rig (`tools/capture-rig/`)       | IN FLIGHT    | K0; see 2.1, re-inventory   |
  | captured physical audio                 | ABSENT       | K4, K5b, K6, Stage 2        |
  | measured IR corpus                      | ABSENT       | Phase C, K0                 |
  | licence-cleared audio corpus            | ABSENT       | every training result       |
  | trained checkpoint                      | ABSENT       | K1, K2, K5a, K7-runtime, K9 |
  | `genomark-n-bench` binary               | ABSENT       | Rust-side K1/K3             |
  | chain channel in `channels.py`          | ABSENT       | K9(a)                       |
  | duration sweep                          | ABSENT       | K9(b)                       |
  | K3 measurement path (3 arms)            | ABSENT       | K3                          |
  | distortion gradient-flow test           | ABSENT       | section 5.1                 |
  | `MeasuredPhysical` bench family         | PROPOSAL     | physical rows in the report |
  | N-B12 parity consumer                   | PREPARED     | Stage 3                     |
  | candle port                             | ABSENT       | Stage 3 / spec 7.1          |
  | Rust inference crate                    | DONE         | -                           |
  | PyTorch harness                         | DONE         | -                           |
  | ONNX contract + loop closure            | DONE         | -                           |
  | licence gating                          | DONE         | -                           |

=====================================================================
3. THE ORDERED PLAN
=====================================================================
The order is not negotiable and the reason is in 3.1. Spec 12.1's calendar is 15 weeks with three
exit points before week 7.

3.1 STAGE 0, WEEKS 1-2: THE PHYSICAL CAPTURE CAMPAIGN. FIRST. BEFORE ANY SERIOUS TRAINING SPEND.

WHY THIS COMES FIRST, stated plainly because it is the single decision most likely to be reversed by
someone who wants to see a loss curve:

The go/stop number for this entire program is a BLIND PHYSICAL DETECTION RATE: a fixed threshold, no
knowledge of the payload, no offset search, CRC-gated, with a false-positive arm on unmarked
captures. THAT NUMBER DOES NOT EXIST ANYWHERE IN THE LITERATURE. Every published speaker-to-
microphone figure is an oracle best-of-search maximum computed with knowledge of the true bits: DeAR
shifts over a 113 ms range and keeps the best, DeepAWR's `search_do_extract()` sweeps 6000 offsets,
breaks early on a perfect hit, and computes `verify_crc()` only to discard the result. So there is no
prior against which to calibrate expectations, and no amount of training tells you what the room
does to a blind detector.

Two weeks, three speakers and three microphones produce it. Concretely, Stage 0 buys four things
that everything downstream needs and that cannot be obtained any other way:

  (a) A measured 48 kHz IR corpus Audio Provenance owns outright, from the exact transducers the product
      will face. Spec 9.2 ranks these FIRST among all IR sources, above MIT Traer/McDermott and
      EchoThief, and spec 9.3's Phase C mixes them in at 50% weight. Without them Phase C trains
      against synthesized rooms only, which is precisely failure mode 5.3.
  (b) Measured speaker and microphone frequency responses, which parameterise distortion stage D5
      with real curves instead of random biquads. D5 is load-bearing: the measured gap between
      WavMark's 0.73 full-message under reverb alone and BER 0.500 under reverb plus band-limiting
      is the size of the error a bench that skips this stage makes.
  (c) The blind baselines of spec 10.3: run AudioSeal and WavMark (both MIT on code AND weights)
      over the physical captures blind, with the false-positive arm. This is days of work and it
      produces the first blind physical detection rate for any licence-clean audio watermark.
      Whatever it is, it calibrates every expectation in the program. If AudioSeal and WavMark both
      sit at chance, that is a strong prior about the channel that no simulation would have told
      you.
  (d) K0 itself: if the measured RT60 at 1.0 m in the treated room exceeds 0.6 s, or the measured
      DRR at 1.0 m is below 0 dB in all three rooms, the target envelope does not exist in realistic
      spaces and the program stops having spent two weeks and about a thousand dollars.

The counter-argument ("train first, then we know what to capture") is wrong because the capture grid
is fixed by the spec, not by the model: three rooms with measured RT60 and background level, three
speakers, three microphones, distances 0.15/0.5/1.0/2.0/3.0 m, two SPLs, marked corpus AND unmarked
corpus of at least equal size. None of that depends on a checkpoint.

Build order inside Stage 0:
  1. Finish and TEST `/Volumes/A/audio-provenance/sdk/tools/capture-rig/`. Section 2.1 lists what was
     present at 19:19:47Z on 2026-08-31 and what was not: sweep generation, Farina deconvolution,
     Schroeder T30, octave-band RT60, ACE-convention DRR, IEC 61672 A-weighting, the campaign
     matrix and a resumable capture store all exist as source; the `capture_rig.cli:main` entry
     point its own `pyproject.toml` declares did not, and `tests/` was empty. Add the CLI, add
     tests that pin `rt60_t30`, `drr_db` and `deconvolve` against synthetic responses with KNOWN
     decay and KNOWN delay, and confirm the campaign matrix includes the unmarked arm of 10.2(e).
     Then check the cell manifest is readable by `training/src/genomark_n/rir/corpus.py` and passes
     `data/manifest.py`'s licence gate with `licence: "audio-provenance-owned"`, which is already in
     `ALLOWED_LICENCES`.
  2. Room characterisation: RT60 and background level for all three rooms, at all distances.
  3. K0 evaluation. STOP HERE if it fires.
  4. IR capture across the full cell grid.
  5. Speaker and microphone response capture.
  6. Marked and unmarked corpus capture (10.2(d) and 10.2(e), equal size).
  7. Blind AudioSeal and WavMark baselines (10.3).

3.2 STAGE 1, WEEKS 3-6: TRAINING HARNESS COMPLETION AND THE PHASE A + B MODEL. Gates K1, K2, K3,
K7, K8.

  1. Acquire and manifest the audio corpus (section 4.3). This can run in parallel with Stage 0.
  2. Close the gaps that block the gates: the chain channel (2.5), the duration sweep (2.6), the K3
     measurement path (2.7), the gradient-flow test (2.8), the `genomark-n-bench` binary (2.4).
  3. Run the mandatory 500-step smoke on the CHOSEN training hardware with the full Phase C
     distortion layer enabled. Read `projected_320k_days`. K8's hardware arm fires at 14 days. Do
     not start a full run until this passes.
  4. Full run to Phase B (step 180k), then evaluate.
  5. Calibrate kappa against the CONVERGED encoder (section 1.5c), not the initial one.
  6. K2, then K1, then K3, then K7. Any one firing stops the program.

3.3 STAGE 2, WEEKS 7-12: PHASE C WITH MEASURED IRs, AND THE PHYSICAL EVALUATION. Gates K4, K5, K6,
K9.

  K5a runs BEFORE any physical evaluation and is the gate that can actually reject the presence
  tier cheaply. Run it first. Then the physical arm, at the threshold K5a froze, never re-tuned.

3.4 STAGE 3, WEEKS 13-15: CANDLE PORT, PARITY (N-B12, N-B13), SDK INTEGRATION, THE CAPABILITIES
RECORD.

Only reached if every gate above passed. `capabilities().acoustic_rerecording` becomes
`MeasuredLimited(AcousticEnvelope)` if and only if a model card carrying a passing envelope loads.
Nothing else can produce that variant.

=====================================================================
4. THE TRAINING RUN
=====================================================================
4.1 HARDWARE. APPLE SILICON IS FOR SMOKE TESTS. IT IS NOT THE TRAINING PATH.

This is not a preference, it is a measured fact on this machine:

    tail -1 /Volumes/A/audio-provenance/sdk/training/runs/smoke300/train_log.jsonl
    # seconds_per_step 10.687, projected_320k_days 39.58, at batch 2 on CPU

K8 fires at 14 days. 39.58 days at batch 2 is 2.8x over, and spec 9.5's real schedule is batch 16
with the full Phase C chain, which is worse. MPS will improve on CPU for the network itself, but
spec 9.6 identifies the dominant cost correctly and it is not the network: stage D11 is a real
encode/decode per example, on CPU, that CANNOT be cached because it follows AGC in the physical
chain. At batch 16 with p = 0.7 that is about 11 codec round trips per step. The Apple machine has
no CPU worker pool to hide them behind.

RENT ONE CUDA GPU. One A100 or 4090 at roughly $0.5-1.5/h completes a 320k-step run in 1.5-2.5 days
with the codec augmentation parallelised across CPU workers. Pick a box with at least 8 vCPU per
GPU, because `data.num_workers: 8` in `configs/phase_c.yaml` is sized for the codec stage, not the
data loading. Note `training/pyproject.toml` requires `torch>=2.6`; the measured environment here is
torch 2.13.0 on Darwin arm64, so the CUDA box needs its own `uv sync` and a CUDA wheel.

4.2 COST, HONESTLY, AND WITH THE MULTIPLIERS THE SPEC LEAVES IMPLICIT

  Stage 0 hardware (one-time, buy or borrow):
    powered monitor                                    $150-350
    small Bluetooth speaker                            $50-80
    laptop internal speakers                           $0
    USB condenser microphone                           $100-150
    measurement microphone (calibrated, e.g. UMIK-1)   $110-150
    audio interface                                    $150-250
    SPL meter                                          $40-80
    stands, cables, isolation                          $80-150
    Android phone if not already owned                 $0-250
                                                       ----------
    Stage 0 hardware total                             $700-1,500

  Training compute:
    one 320k-step run, A100/4090 at $0.5-1.5/h, 1.5-2.5 days   $50-90
    spec 9.6 says THREE TO FIVE RUNS before a result is trustworthy
    plus aborted/failed runs, hyperparameter probes, dev time on the box
                                                              ----------
    realistic compute total                            $400-900

  Data storage and egress:
    800 h of 48 kHz stereo is about 553 GB as PCM16, roughly 300-350 GB as FLAC.
    Cloud NVMe at $0.08-0.15/GB/month for 2-3 months                 $80-350
    egress on the way back out                                       $50-100
                                                                     ----------
    data total                                         $130-450

  PROGRAM CASH TOTAL, EXCLUDING LABOUR: roughly $1,200 to $2,900.
  WALL CLOCK: 15 weeks per spec 12.1, with exit points at week 2 (K0), week 6 (K1/K2/K3/K7/K8) and
  week 12 (K4/K5/K6/K9).

  Uncertainty I will not paper over: the compute figure assumes the D11 codec stage parallelises
  cleanly across CPU workers. It has never been measured at batch 16 on a CUDA box. If it does not,
  the per-run figure could be 2-3x higher and K8's hardware arm might require a second GPU or a
  pre-rendered codec cache that the spec explicitly forbids for D11. MEASURE THIS IN THE 500-STEP
  SMOKE ON THE RENTED BOX BEFORE COMMITTING TO A RUN.

4.3 DATASET ACQUISITION, WITH LICENCES

Every source below must land as a JSONL manifest with a per-item `licence` field.
`training/src/genomark_n/data/manifest.py` REFUSES an entry whose licence is outside
`ALLOWED_LICENCES` or whose source is in `DENIED_SOURCES`, so an unfiltered download fails loudly at
load time rather than quietly training on it.

  MUSIC, target 800 h at 48 kHz.
    MTG-Jamendo (github.com/MTG/mtg-jamendo-dataset). Download with the repo's own scripts, then
    filter on the per-track licence metadata to CC0-1.0 / CC-BY-* / CC-BY-SA-* ONLY. Exclude every
    NC and every ND track. Spec 9.1 estimates 25-30k tracks survive. Emit one manifest line per
    track with its own licence string and attribution.
    Audio Provenance's own licensed catalogue is the PREFERRED source where available; `licence:
    "audio-provenance-owned"` or `"audio-provenance-licensed"`, both already allowlisted.
    FMA IS DENIED as a source id. Its metadata is CC BY-NC-SA 4.0 and its audio carries
    heterogeneous per-track licences, a substantial fraction NonCommercial. Both DeAR and DeepAWR
    trained on unfiltered FMA; that is a real exposure for a shipped commercial model. A per-track
    filtered FMA subset is usable only if each surviving track declares its own licence, and it must
    not carry the `fma` source id.

  SPEECH, 40 h. VCTK, CC BY 4.0, from Edinburgh DataShare.

  ENVIRONMENTAL, 40 h. Spec 9.1 says "a CC-BY environmental-sound subset". FSD50K is the obvious
  candidate but its clips carry heterogeneous per-clip CC licences including NC, so it needs the
  same per-clip filtering FMA does. I have not verified FSD50K's current licence distribution; do
  that before downloading.

  NOISE. MUSAN, OpenSLR-17, CC BY 4.0. Wire it through `data.noise_manifests` in
  `configs/phase_c.yaml`. Until then `training/src/genomark_n/distortion/noise.py` synthesises a
  tilted room floor (U(-6,-3) dB/octave about 250 Hz, flat below 50 Hz) plus a Gaussian floor 20 dB
  beneath it. That synthesis exists because a WHITE floor puts far too much power at the top of the
  211-7688 Hz band and too little at the bottom, which flatters the result exactly where the mark is
  hardest to place. MUSAN adds ON TOP of that floor, it does not replace it.
  Audio Provenance's own Stage 0 room-noise recordings go here too.

  IMPULSE RESPONSES, in the spec's priority order:
    1. Stage 0 measured IRs. `licence: "audio-provenance-owned"`. Highest value, and Phase C mixes them at
       50% weight against synthetic.
    2. MIT Acoustical Reverberation Scene Statistics Survey (Traer and McDermott), 271 IRs,
       CC BY 4.0, 44.1 kHz, upsampled to 48 kHz. Legitimate because nothing above 22.05 kHz is
       fabricated into the 211-7688 Hz band we use.
    3. EchoThief, 48 kHz, terms explicitly grant convolution-derivative use.
       `licence: "echothief-convolution-grant"`, already allowlisted.
    DENIED: OpenSLR-28 RIRS_NOISES (16 kHz only; driving a 48 kHz layer with it fabricates the whole
    8-24 kHz band, exactly where transducer roll-off lives). DENIED: Aachen AIR standalone
    (research-use only, notwithstanding that AWARE used it).

  MODEL WEIGHTS: none, from anyone. SilentCipher's weights carry no grant (the release tarball has
  no LICENSE/NOTICE/COPYING and the README scopes its MIT grant to "the code in this repository";
  the HuggingFace mirror has `cardData: null`). DeepAWR's chain of title is defective. DeAR is
  unlicensed. AudioSeal is MIT on code AND weights and is the harness donor, but this program trains
  its own weights, so nothing is vendored. If you ever do vendor something, record the licence and
  the URL you read it from in the relevant NOTICE file.

4.4 HYPERPARAMETERS TO START FROM

`/Volumes/A/audio-provenance/sdk/training/configs/phase_c.yaml` already encodes spec 9.3 and 9.5. Start
there and change only what a measurement tells you to:

    optim:  lr 1.0e-4, betas [0.9, 0.99], weight_decay 1.0e-2, warmup_steps 2000,
            total_steps 320000, min_lr 1.0e-6, grad_clip 1.0, batch_size 16,
            ema_decay 0.9999, unmarked_fraction 0.5
    curriculum: phase_a_end 60000, phase_b_end 180000, phase_c_end 320000
    loss:   message 1.0, message_per_frame 0.2, detection 1.0, perceptual 20.0,
            spectral 4.0, q_coexistence 5.0, adversarial 0.1, adversarial_enabled FALSE
    detector: window 30.0 s, hop 15.0 s, presence_window 10.0 s,
            min_presence_span 8.0 s, max_windows 64
    rir:    synthetic_fraction 0.5, pool_size 512, max_order 12, max_ir_seconds 1.5,
            rt60_s [0.18, 0.80], source_distance_m [0.3, 3.0]

  KEEP `adversarial_enabled: false` FOR THE FIRST FULL RUN. Spec 9.4 is right: adversarial training
  is the most likely source of an unreproducible result and the program's first job is a clean
  falsifiable number.

  KAPPA is the one number NOT in the config to be taken on faith. Calibrate it in build unit N-C0
  against a converged encoder and expect roughly 0.2. `MAX_BUDGET_NEPERS` = 0.35 at
  `crates/genomark-n/src/params.rs` is a HARD ceiling that no calibration may raise. If the mark
  needs more than 3.04 dB of per-bin swing to survive, it is audible, and the answer is to kill the
  program, not to raise the ceiling.

  EMA SCOPE is a declared deviation: the harness EMAs the WHOLE model including the decoder, because
  the decoder is what the frozen threshold is calibrated against, and an EMA encoder paired with a
  raw decoder would calibrate one model and ship another. Spec 9.5 says "EMA of the encoder
  weights". If the spec's scope is meant literally, narrow it in `runlog.Ema`, and then fix the
  calibration to match.

=====================================================================
5. FAILURE MODES, IN THE ORDER THEY USUALLY BITE
=====================================================================
5.1 THE DISTORTION LAYER DOES NOT BACKPROPAGATE. BITES FIRST, COSTS THE MOST.

Symptom: training loss falls, `bit_accuracy` on the clean path is fine, and robustness never
improves under any channel that involves a codec. The gradient is being silently killed at a
`.detach()`, at a numpy round trip, at a subprocess boundary, or at a `torch.no_grad()` that was
meant to cover only the QIM offset in D0.

Why it is first: it is invisible. Nothing crashes. The loss curve looks correct because the clean
terms dominate.

DETECTOR: THERE ISN'T ONE TODAY. `grep -rn "backward\|requires_grad" /Volumes/A/audio-provenance/sdk/training/tests/`
finds only two `no_grad` hits in `test_equivariance.py`, and `overfit_check.sh` bypasses the chain
entirely by design so it cannot catch this. WRITE THE TEST BEFORE THE FIRST FULL RUN: build a
one-item batch, enable every stage D0-D12 at p = 1.0, run forward and backward, and assert that
`encoder` parameter `.grad` is non-None, finite, and non-zero, stage by stage. Then assert it again
with the codec stage forced on, because D11's identity-gradient trick
(`training/src/genomark_n/distortion/codec.py`) is the specific place a real ffmpeg round trip can
sever the graph. SilentCipher's own ablation drops MP3 accuracy to ZERO when the pseudo-
differentiable compression layers are removed, so this stage is not optional and a silently dead
gradient through it looks exactly like "the design does not work".

5.2 THE MODEL HIDES THE MARK WHERE THE CODEC THROWS IT AWAY.

Symptom: excellent bit accuracy on `identity` and on the acoustic rows, and a cliff on `mp3_128` /
`aac_128` / `opus_64`. The encoder has learned to place energy above the codec's cutoff, in a band
parametric stereo collapses, or in bins the psychoacoustic model of the encoder discards.

DETECTOR: the eval channel set in `training/src/genomark_n/config.py` `EvalConfig.channels` already
includes `mp3_128`, `aac_128`, `opus_64` and `lowpass_16k` alongside the acoustic rows. Read the
per-channel `exact_recovery_rate` in `eval.json` as a PROFILE, not a scalar. A large spread between
identity and the codec rows IS this failure.

MITIGATION: D1 (source codec history) and D11 (capture-side codec) are both in the chain at
p = 0.5 and p = 0.7. If the cliff persists, raise D11's probability before touching the band, and
check that D11 is actually running rather than silently no-oping because ffmpeg was not found at
/opt/homebrew/bin/ffmpeg.

5.3 THE DETECTOR OVERFITS SYNTHESIZED RIRs AND COLLAPSES ON REAL ONES.

Symptom: `acoustic_1m_room` recovery in the 0.8s during training, and the physical corpus at chance.
This is the failure that makes Stage 0 come first, and it is the most expensive one because it is
only visible after a capture campaign.

Why it happens: an image-source synthesizer produces a rectangular geometry with a smooth,
parametric decay. Real rooms have furniture, non-rectangular boundaries, frequency-dependent
absorption, and a diffuse tail with a different statistical character. A decoder given only
synthetic rooms will learn the synthesizer.

DETECTORS AND MITIGATIONS:
  - `rir.synthetic_fraction: 0.5` in `configs/phase_c.yaml` exists exactly for this: half the
    training IRs must be MEASURED, not synthesized.
  - SPLIT MEASURED IRs BY ROOM, NOT BY CELL. A cell-level split leaks: two cells in the same room at
    two distances share the room's modal structure and its absorption, so a decoder that memorises
    the room passes a cell-level held-out set. Hold out entire rooms, and if the budget allows, hold
    out an entire room type.
  - Note the harness's own honesty about its synthesizer, from `training/README.md`: a rectangular
    image-source field does not decay at the Eyring rate, because reflection count per metre varies
    with direction, so by Jensen the direction-averaged decay is slower. A room asked for 0.40 s
    measured 0.55 s uncorrected. `rir/image_source.py` applies a measured-decay correction and
    carries the ACHIEVED T30 on the result. Read that field; do not trust the requested RT60.

5.4 SYNC ASSUMPTIONS CREEP BACK IN AS AN ORACLE SEARCH.

Symptom: someone adds "just a small offset sweep" to recover a few points, or an evaluation harness
picks the best-scoring window, or a helper takes the expected payload "for debugging" and never
loses it. The numbers improve and become meaningless, because they are then the same quantity every
published paper reports and not the quantity this program exists to produce.

DETECTORS, and these are strong:
  - `training/tests/test_blindness.py` pins `Detector.detect`'s signature to exactly
    `{self, audio, thresholds}`. Adding a payload parameter fails the test suite.
  - `crates/genomark-n/src/lib.rs` lines 68-76 state the rule as a crate invariant, and no public
    function in the crate takes an expected payload, a time offset, or a threshold as a detection
    input.
  - `training/src/genomark_n/channels.py` channels truncate to the input length and never compensate
    the room's propagation delay or a codec's encoder delay. Sync is the detector's job.
  - K9 is the empirical version: chain `drift_plus_0p1pct` with the 1 m acoustic row and require the
    recovery drop to stay within 0.10 of the acoustic row alone.
  THE STANDING RISK is that these defend the Python and Rust detectors but not an ad-hoc analysis
  script someone writes in a notebook. If a number appears that is better than the harness's, ask
  which command produced it before believing it.

5.5 THRESHOLD TUNING AFTER THE FACT, WHICH QUIETLY DESTROYS THE FALSE-POSITIVE GUARANTEE.

Symptom: recall is disappointing, someone lowers `presence_accept_score` or widens the flip budget,
recall improves, and the published false-positive rate is now a rate that was never measured at the
shipped threshold. This is the failure the entire spec is written against, and it is the one that
turns an honest negative result into a false claim.

DETECTORS, and one of them is provable:
  - `training/src/genomark_n/calibrate.py` NEVER SEES A MARKED CLIP. It produces a `FrozenThresholds`
    record over unmarked audio only, carrying its trial count and the 3/N bound that count implies,
    and it refuses to hide when the count is too small to support the stated rate.
  - `runs/*/thresholds.json` carries `weights_digest` AND `config_digest`. Verified in
    `/Volumes/A/audio-provenance/sdk/training/runs/smoke300/thresholds.json`:
    `weights_digest: "9a3bb30e..."`, `config_digest: "0f6a92ca..."`. A threshold file whose
    `weights_digest` does not match the evaluated checkpoint IS EVIDENCE the threshold moved. Check
    it in review, every time.
  - The Rust side has no call-time threshold at all: `crates/genomark-n/src/thresholds.rs` takes
    everything from the model card.
  - K5a states the rule as a gate: the FPR is the CONSTRAINT and the recall is the RESULT. Do not
    re-tune upward to recover recall.
  - Spec 3.4's arithmetic ties the flip budget to the CRC width: 64 windows x 11 patterns x 2^-24 =
    4.2e-5 false accepts per file. Widening k = 2 over 4 bits without widening CRC-24 moves the
    false-accept cost somewhere the report does not print. `crates/genomark-n/src/params.rs`
    `FLIP_SEARCH_BITS`/`FLIP_SEARCH_WEIGHT`/`FLIP_PATTERNS` and
    `training/tests/test_blindness.py::test_flip_search_is_a_fixed_constant_independent_of_truth`
    both pin it. K4 says a run that widens the budget to reach 0.50 HAS NOT PASSED.

5.6 THE ONE NOBODY EXPECTS: THE PERCEPTUAL BUDGET LOOKS FINE ON AN UNTRAINED MODEL.

Covered in section 1.5(c) and repeated here because it will cost someone a run. Every kappa from
0.05 to 0.50 passes K2 on an untrained encoder, because an untrained encoder does not use its
budget. The converged overfit check lands at segSNR 14.87 dB at kappa 0.5, well under K2's 22 dB
floor. Calibrate kappa on a converged encoder or the K2 result is an artifact.

=====================================================================
6. THE TEN KILL CRITERIA, WITH THE EXACT COMMAND AND THE THRESHOLD THAT FIRES
=====================================================================
A kill criterion nobody can run is decoration. Each entry below is labelled with one of three
states: RUNS TODAY, RUNS AFTER A TRAINED CHECKPOINT, or NO RUNNER EXISTS. Any one firing stops the
program at that stage. There is no aggregate score and no partial credit.

Preamble on false-positive rates, spec 12.2, because the precision matters: both thresholds are
CALIBRATED on >= 5000 unmarked trials through the SIMULATED acoustic channels, then CONFIRMED on the
physical unmarked corpus with zero accepts required and the bound published as 3/N with N stated.
Zero accepts over N trials bounds the rate at roughly 3/N, not at zero. A physical campaign yields
hundreds of unmarked captures, so 1e-3 is NOT physically measurable at Stage 2 scale, and no
criterion below asks the physical arm for a precision it cannot deliver.

---------------------------------------------------------------------
K0  STAGE 0, CHANNEL SANITY.                     RUNNER EXISTS, UNVERIFIED, NO DATA.
---------------------------------------------------------------------
FIRES IF: measured RT60 at 1.0 m in the treated room > 0.6 s, OR measured direct-to-reverberant
ratio at 1.0 m is below 0 dB in ALL THREE rooms. Measured from the Stage 0 IRs, not simulated.

THE ARITHMETIC IS IMPLEMENTED AND UNTESTED, AND THERE IS NO DATA.
`/Volumes/A/audio-provenance/sdk/tools/capture-rig/src/capture_rig/acoustics.py` provides `rt60_t30()`
(Schroeder backward integration with Lundeby noise truncation, T30 extrapolated to 60 dB),
`octave_band_rt60()`, `drr_db()` (ACE convention) and `dba_from_dbfs()`, and its module docstring
states these are the conventions K0 is evaluated under.
`.../src/capture_rig/sweep.py` provides `deconvolve()` and `measure_response()` for the Farina path.

What was missing at 19:19:47Z on 2026-08-31: the `capture_rig.cli:main` entry point declared in
`pyproject.toml`, and any test at all (`tests/` was an empty directory). So the command shape below
is what to BUILD toward, not what to run today. Re-inventory first, per section 2.1.

    cd /Volumes/A/audio-provenance/sdk/tools/capture-rig
    uv sync && uv run pytest -q                  # must collect and pass before any number is trusted
    uv run capture-rig measure-room --room treated --distance 1.0 --out captures/treated/
    uv run capture-rig k0 captures/              # per-room RT60 and DRR, non-zero exit on fire

DO NOT SKIP THE TESTS. A sign error or an off-by-one in the Schroeder integration produces a
PLAUSIBLE WRONG NUMBER rather than a crash, and K0 is the gate that decides whether the program
continues. Pin `rt60_t30` against a synthetic exponentially-decaying noise burst with a known T60,
and `deconvolve` against a known-delay impulse, before pointing either at a measured capture.

CHEAPEST GATE IN THE PROGRAM AND IT IS FIRST. Two weeks and about $1,000 buys the answer to "does
the target envelope exist in realistic spaces at all".

---------------------------------------------------------------------
K1  STAGE 1, SIMULATED RECOVERY.                RUNS AFTER A TRAINED CHECKPOINT.
---------------------------------------------------------------------
FIRES IF: on `acoustic_small_room` (0.5 m) OR `acoustic_1m_room`, over 200 corpus items at 30 s,
`exact_recovery_rate` < 0.60 while `false_positive.rate` = 0 over >= 5000 unmarked trials.

Both presets exist: `training/src/genomark_n/channels.py` lines 44-45,
`acoustic_small_room` = RoomPreset(0.28 s, 0.5 m, 40 dB SNR, drift 1.0001, DRR 8 dB) and
`acoustic_1m_room` = RoomPreset(0.35 s, 1.0 m, 36 dB SNR, drift 1.00015, DRR 8 dB, proposed=True).
`configs/phase_c.yaml` already sets `corpus_items: 200`, `clip_seconds: 30.0`,
`false_positive_trials: 5000`.

    cd /Volumes/A/audio-provenance/sdk/training
    uv run python -m genomark_n.calibrate --config configs/phase_c.yaml \
      --checkpoint runs/phase_c/checkpoints/latest.pt --trials 5000 \
      --out runs/phase_c/thresholds.json
    uv run python -m genomark_n.evaluate --config configs/phase_c.yaml \
      --checkpoint runs/phase_c/checkpoints/latest.pt \
      --thresholds runs/phase_c/thresholds.json --out runs/phase_c/eval.json
    uv run python -c "
    import json,sys
    d=json.load(open('runs/phase_c/eval.json'))
    fired=False
    for c in d['channels']:
        if c['name'] in ('acoustic_small_room','acoustic_1m_room'):
            fp=c['false_positive']
            print(c['name'], 'recovery', c['exact_recovery_rate'],
                  'fp_rate', fp['locator_rate'], 'fp_trials', fp['trials'])
            if c['exact_recovery_rate'] < 0.60 or fp['locator_rate'] != 0.0 or fp['trials'] < 5000:
                fired=True
    print('K1 FIRES' if fired else 'K1 passes')
    sys.exit(1 if fired else 0)"

RATIONALE: simulation is strictly easier than physical, and the best published simulated replay
result corresponds to roughly 0.89. Failing 0.60 in simulation makes physical hopeless.

---------------------------------------------------------------------
K2  STAGE 1, AUDIBILITY.                                        RUNS TODAY.
---------------------------------------------------------------------
FIRES IF: `frames_above_mask_fraction` > 0.10 on the `harmonic_pad` or `near_silence` content class,
OR `noise_to_mask_max_db` > +3 dB on any class, OR `segmental_snr_db` < 22 dB, at the budget that
achieves K1.

    cd /Volumes/A/audio-provenance/sdk/training
    uv run python -m genomark_n.calibrate --config configs/phase_c.yaml \
      --checkpoint runs/phase_c/checkpoints/latest.pt \
      --out runs/phase_c/kappa_sweep.json \
      --kappa-sweep 0.05,0.10,0.15,0.20,0.25,0.30,0.40,0.50
    # prints one line per kappa: nmr_max, above_mask, segSNR, and K2 pass/FAIL

Verified working on the smoke checkpoint (section 1.5c). Note the trap in 5.6: this command is only
meaningful against a CONVERGED encoder. On an untrained one every kappa passes.

`L_perc` and the bench measure THE SAME function: `training/src/genomark_n/perceptual.py` is a torch
port of `crates/audio-provenance-bench/src/perceptual.rs`. That is deliberate, so a training win cannot be a
measurement artifact.

---------------------------------------------------------------------
K3  STAGE 1, GENOMARK-Q COEXISTENCE.                        NO RUNNER EXISTS.
---------------------------------------------------------------------
FIRES IF ANY OF: Q's `exact_recovery_rate` on `identity`, `mp3_128`, `aac_128` or
`chain_transcode_mp3_128_aac_128` drops by more than 0.01 absolute with N present; OR
`std(D_marked - D_cover)` on Q's statistic exceeds 0.04 nepers; OR C0's N-present DELTA exceeds its
cover-only DELTA by more than 5%.

Three arms, none runnable today (section 2.7). What to build:

  (a) Q matrix twice. `crates/genomark/src/bin/genomark-bench.rs` runs it once over cover audio.
      Add an N-embed pre-pass, or write a wrapper that N-marks the corpus to WAV first and passes it
      via `--real-wav`:
          CARGO_TARGET_DIR=/Volumes/C/rust-target cargo run --release -p genomark \
            --bin genomark-bench -- --out-dir bench-out/k3-without-n --duration 30
          # then the same with an N-marked corpus, and diff the four named rows
  (b) The 0.04 nepers bound. `training/src/genomark_n/qmark.py` has the statistic and the hinge but
      no CLI. Add one that loads a checkpoint, marks a corpus, and prints
      `std_corpus(D_marked - D_cover)`. The bound is `OLA_RESIDUAL_LIMIT` at
      `crates/genomark/src/params.rs:29` (= `DELTA / 20.0` = 0.04). It is not a new number; it is
      the bound Q's own OLA-closure unit test already asserts.
  (c) C0 with N present. `DELTA` is a hardcoded `0.8` at `crates/genomark/src/params.rs:24` and the
      C0 harness that produced it is not in this tree. Rebuild the measurement, run it cover-only
      and N-present, record BOTH values.

N IS NOT PERMITTED TO TAX THE SHIPPED MARK. Note also spec 8.3: exact cancellation of N's mask
against Q's pair grid is UNAVAILABLE, because Q's grid is a 1024-point STFT at the native rate and
N's is a 2048-point STFT at 48 kHz whose residual is then resampled. Anyone who writes "N is
orthogonal to Q by construction" is wrong.

---------------------------------------------------------------------
K4  STAGE 2, THE DECISIVE PHYSICAL GATE, LOCATOR TIER.      NO RUNNER EXISTS.
---------------------------------------------------------------------
FIRES IF: over >= 3 rooms x >= 2 speakers x >= 2 microphones x >= 200 clips at 1.0 m and 30 s,
BLIND, CRC-gated, threshold fixed BEFORE the run, flip budget k = 2 over 4 bits and nothing wider:
`exact_recovery_rate` < 0.50. `false_positive.rate` = 0 is REQUIRED on the unmarked physical corpus
and its 3/N bound must be reported.

COMMAND: does not exist. Needs (i) the Stage 0 physical corpus, (ii) the physical arm in
`audio-provenance-bench` per spec 10.4, which is a PROPOSAL to the bench owner and not an edit this program
makes, and (iii) `crates/genomark-n/src/bin/genomark-n-bench.rs` (section 2.4). The physical arm is
NOT a `Channel`: `Channel::apply` is a pure transform of in-memory audio, and a physical capture is a
LOOKUP of a previously recorded counterpart file. Model it as a paired corpus with a passthrough
channel named `physical_<room>_<speaker>_<mic>_<distance>` whose `params()` carry measured RT60,
background level, SPL and hardware ids, under a new `ChannelFamily::MeasuredPhysical` for which
`is_simulated_physical_path()` returns FALSE.

Below 0.50 the feature returns `not_found` on most real re-recordings, which is worse than honestly
declaring the path unsupported. A run that widens the flip budget to reach 0.50 HAS NOT PASSED THIS
GATE; it has moved the false-accept cost somewhere the row does not print.

---------------------------------------------------------------------
K5a STAGE 2 PRECONDITION, PRESENCE CALIBRATION.  RUNS AFTER A TRAINED CHECKPOINT.
---------------------------------------------------------------------
THIS IS THE GATE THAT CAN ACTUALLY REJECT THE PRESENCE TIER, AND IT RUNS BEFORE ANY PHYSICAL RUN.
Run it before spending a capture campaign on the tier.

FIRES IF: at the presence threshold frozen where `false_positive.rate` <= 1e-3 over >= 5000 unmarked
trials through `acoustic_small_room` and `acoustic_1m_room`, recall measured as
`payload_returned_rate` for the zero-bit detector over 10 s windows is < 0.80.

    cd /Volumes/A/audio-provenance/sdk/training
    uv run python -m genomark_n.calibrate --config configs/phase_c.yaml \
      --checkpoint runs/phase_c/checkpoints/latest.pt --trials 5000 \
      --out runs/phase_c/thresholds.json
    uv run python -c "
    import json,sys
    t=json.load(open('runs/phase_c/thresholds.json'))
    print('threshold',t['presence_threshold'],'fpr',t['measured_false_positive_rate'],
          'trials',t['calibration_trials'])
    sys.exit(0 if t['calibration_trials']>=5000 and t['measured_false_positive_rate']<=1e-3 else 1)"
    # then, with a config whose evaluation.clip_seconds is 10.0:
    uv run python -m genomark_n.evaluate --config configs/phase_c_10s.yaml \
      --checkpoint runs/phase_c/checkpoints/latest.pt \
      --thresholds runs/phase_c/thresholds.json --out runs/phase_c/eval_10s.json
    uv run python -c "
    import json,sys
    d=json.load(open('runs/phase_c/eval_10s.json'))
    rows=[(c['name'],c['presence_recall']) for c in d['channels']
          if c['name'] in ('acoustic_small_room','acoustic_1m_room')]
    print(rows); sys.exit(1 if any(r<0.80 for _,r in rows) else 0)"

`configs/phase_c_10s.yaml` does not exist; make it a copy of `phase_c.yaml` with
`evaluation.clip_seconds: 10.0`, because `evaluate.py` has no CLI override for it (section 2.6).

DO NOT RE-TUNE THE THRESHOLD UPWARD TO RECOVER RECALL. The FPR is the constraint and the recall is
the result.

---------------------------------------------------------------------
K5b STAGE 2, PHYSICAL CONFIRMATION.                         NO RUNNER EXISTS.
---------------------------------------------------------------------
FIRES IF: at the SAME frozen threshold, `payload_returned_rate` < 0.80 at 1.0 m on the physical
corpus, OR any accept occurs on the unmarked physical corpus.

K5b CONFIRMS; it cannot ESTABLISH an FPR. Zero accepts over the few hundred unmarked physical
captures bounds the rate only at ~3/N ~= 1e-2, one false fire per hundred clips with no CRC behind
it, which is not a shippable signal on its own evidence. The 1e-3 number comes from K5a and is
published as such, with the physical trial count reported beside it.

IF K5a OR K5b FIRES, THE ENTIRE PROGRAM STOPS. See section 7.

`crates/genomark-n/src/envelope.rs` already encodes both arms: lines 140-147 refuse a card with
fewer than `MIN_CALIBRATION_TRIALS` (5000) or an FPR above `MAX_PRESENCE_FALSE_POSITIVE_RATE` (1e-3);
lines 166-171 refuse an empty physical false-positive arm ("it cannot be synthesised later") and any
non-zero accept count. `envelope.rs` lines 233-258 test that both bite.

---------------------------------------------------------------------
K6  STAGE 2, DISTANCE HONESTY.                              NO RUNNER EXISTS.
---------------------------------------------------------------------
FIRES IF: the largest distance at which K4's 0.50 threshold is met is below 0.5 m.

Needs the physical grid at 0.15/0.5/1.0/2.0/3.0 m, so it comes free with K4's harness: evaluate K4's
criterion per distance cell and take the maximum passing distance.
`crates/genomark-n/src/envelope.rs` line 177 enforces it on the card (`MIN_DISTANCE_M` = 0.5).

A mark that needs the microphone within half a metre is a coupling test, not re-recording, and no
defensible product claim can be scoped to it.

---------------------------------------------------------------------
K7  ANY STAGE, RUNTIME AND SIZE.       SIZE RUNS TODAY. RUNTIME NEEDS A CHECKPOINT.
---------------------------------------------------------------------
FIRES IF: `mean_detect_seconds` implies RTF > 0.5x for a 30 s window on the development machine's
CPU without Metal, OR the shipped model file exceeds 40 MB.

SIZE, runs today:

    cd /Volumes/A/audio-provenance/sdk/training
    uv run python -c "
    import json,sys
    c=json.load(open('runs/smoke300/model_card.json'))['architecture']
    print(c['parameters']['total'],'params', c['fp32_bytes'],'bytes',
          'ceiling',c['k7_size_ceiling_bytes'],'pass',c['k7_size_pass'])
    sys.exit(0 if c['k7_size_pass'] else 1)"
    # measured: 8229523 params, 32918092 bytes, ceiling 41943040, pass True

RUNTIME: read `mean_detect_seconds` and `p95_detect_seconds` per channel from `eval.json` and divide
by `clip_seconds`. Measured on the smoke run at 4 s: 0.438-0.462 s, RTF about 0.11x. Caveats in
section 1.5(d): that is the PyTorch CPU path, and K7 names the Rust CPU path without Metal. The Rust
measurement comes from `mean_detect_seconds` in a `genomark-n-bench` report once section 2.4's
binary exists.

If size fires, narrow `encoder.stage_channels`, or take the int8 post-training quantisation of the
convolution weights that spec 2.8 sizes at ~6 MB, validated against the fp32 logits in row N-B12.

---------------------------------------------------------------------
K8  ANY STAGE, SCHEDULE.        HARDWARE ARM RUNS TODAY AND FIRES TODAY.
---------------------------------------------------------------------
THREE SEPARATE ARMS. Do not conflate them.

(a) HARDWARE SELECTION ARM: if the projected wall clock for one 320k-step run on the chosen hardware
    exceeds 14 days, THE HARDWARE IS WRONG AND MUST CHANGE BEFORE TRAINING STARTS. This is not a
    program kill; it is a procurement instruction.

    THE SMOKE NEEDS A CONFIG THAT HAS DATA. `configs/phase_c.yaml` cannot run as shipped: it has
    `data.manifests: []`, `data.noise_manifests: []` AND `synthetic_items: 0`, and
    `training/src/genomark_n/data/dataset.py` raises on that combination by design ("a dataset that
    silently yields nothing would let a run report a result over no data"). Verified by reading the
    guard. So either fill in the section 4.3 manifests first, or make a
    `configs/phase_c_smoke.yaml` copy with `data.synthetic_items` non-zero (say 64) and
    `optim.total_steps: 500`. SYNTHETIC AUDIO IS FINE HERE AND ONLY HERE: the number K8 wants is
    seconds per step through the full Phase C distortion chain, which is a property of the chain and
    the hardware, not of the corpus. Nothing else about such a run means anything.

        cd /Volumes/A/audio-provenance/sdk/training
        uv run python -m genomark_n.train --config configs/phase_c_smoke.yaml --steps 500 \
          --out runs/smoke500
        tail -1 runs/smoke500/train_log.jsonl
        # read seconds_per_step and projected_320k_days; fires above 14.0

    MEASURED TODAY on the existing 300-step run
    (/Volumes/A/audio-provenance/sdk/training/runs/smoke300/train_log.jsonl, last line):
    `seconds_per_step` 10.687, `projected_320k_days` 39.58, at batch 2 on CPU.
    THIS ARM FIRES NOW. Section 4.1 is the response: rent a CUDA GPU, then re-run this on the rented
    box at batch 16 with the full Phase C chain before believing any budget.

(b) PROGRAM KILL ARM: if FOUR FULL RUNS complete without K1 passing, kill. No runner needed; count
    the runs.

(c) DURATION ARM: if the measured capture duration required to reach K4's threshold exceeds 45 s,
    kill, because the duration has left the range of a realistic capture. Needs the physical corpus.
    `crates/genomark-n/src/envelope.rs` line 183 enforces it on the card (`MAX_DURATION_S` = 45.0).

---------------------------------------------------------------------
K9  STAGE 2, THE SYNCHRONISATION CLAIM ITSELF.              NO RUNNER EXISTS (BOTH ARMS).
---------------------------------------------------------------------
(a) FIRES IF: `drift_plus_0p1pct` chained with the 1 m acoustic row gives an `exact_recovery_rate`
    more than 0.10 below the acoustic row alone. Then the position-agnostic readout claim in spec
    section 4 is FALSE and the design has an unsolved synchronisation problem.

    THE CHAIN CHANNEL DOES NOT EXIST. Verified in section 2.5: `ChannelBank.build()` raises KeyError
    on any chained name. Add a `chain_a+b` form to
    `/Volumes/A/audio-provenance/sdk/training/src/genomark_n/channels.py`, then:

        uv run python -m genomark_n.evaluate --config configs/phase_c.yaml \
          --checkpoint runs/phase_c/checkpoints/latest.pt \
          --thresholds runs/phase_c/thresholds.json --out runs/phase_c/eval.json
        # with evaluation.channels including both 'acoustic_1m_room' and
        # 'chain_acoustic_1m_room+drift_plus_0p1pct', then compare the two rates

(b) FIRES IF: recovery in bench row N-B7 is not monotone non-decreasing in capture duration across
    5, 10, 20, 30 and 45 s. Then the pooling is not integrating and the same conclusion follows.

    NO SINGLE COMMAND. `evaluate.py` has no `--clip-seconds`. Today this is five config copies
    differing only in `evaluation.clip_seconds`, five `evaluate` invocations, and a hand comparison
    of `exact_recovery_rate`. Add the flag or write the sweep script; nothing asserts monotonicity.

K9 is the criterion that tests the SINGLE MOST IMPORTANT STRUCTURAL DECISION in the design. Give it
a real runner rather than treating it as a formality.

=====================================================================
7. THE HONEST OFF-RAMP
=====================================================================
IF A KILL CRITERION FIRES, SHIP NOTHING. That is the correct outcome, not a fallback.

Concretely, in this order:

  1. LEAVE `capabilities().acoustic_rerecording` AS THE LITERAL `"unsupported"`. You do not have to
     do anything to achieve this and you CANNOT accidentally do otherwise.
     `crates/genomark-n/src/envelope.rs`: `AcousticEnvelope` has no public constructor, its fields
     are private, and `EnvelopeClaim::validate` is `pub(crate)`. The only route to
     `AcousticRerecording::MeasuredLimited` is loading a model card whose envelope passes every
     numeric gate at envelope.rs lines 140-193. A build without one is STRUCTURALLY incapable of
     reporting anything else. Do not add an escape hatch. A boolean is what lets marketing outrun
     engineering; that is why the field is a string and, on a pass, a structured record carrying its
     own distance, duration, rate and trial count.

  2. DO NOT SHIP THE MODEL. `crates/genomark-n` can stay in the tree: it is clippy-clean, tested,
     vendors nothing, and reports `unsupported` with no model present. Spec 7.5's rule already
     covers this: a build without the model file reports the N rung as `unavailable` and sets
     `incomplete`; it never becomes a verdict.

  3. RECORD THE MEASURED NUMBERS IN /Volumes/A/audio-provenance/sdk/docs/HONEST_LIMITS.md. Note that file
     PREDATES this program: its acoustic paragraph says the learned path is "out of scope for a
     pure-TypeScript SDK", which is no longer the reason. So the off-ramp is an ADDED GenoMark-N
     section, not an edit to the existing paragraph. It must carry, at minimum:
       - which criterion fired, with its measured value and its threshold;
       - the blind physical detection rate, if Stage 2 was reached, with the room / speaker /
         microphone / distance / duration conditions attached;
       - every false-positive rate with its TRIAL COUNT and its 3/N bound, following the bench's own
         `FALSE_POSITIVE_NOTE` convention. A rate quoted without its N is not a result;
       - the AudioSeal and WavMark blind physical baselines from spec 10.3, which are the comparison
         nobody else has published;
       - the measured RT60, DRR and background level of the three rooms, so the next person knows
         what channel the number was measured over.
     The point of writing it down is that the question is then SETTLED WITH EVIDENCE rather than
     re-opened annually by someone who read a paper abstract.

  4. WITHDRAW ANY EXTERNAL CLAIM THAT DOES NOT MATCH. Spec 0 is emphatic and independent of the
     program's outcome: the product website's claim that the mark survives "re-recording" must be withdrawn
     regardless. A `verify()` that returns `not_found` on most real re-recordings while the website
     promises otherwise is worse than one that honestly declares the path unsupported.

  5. TREAT THE NEGATIVE RESULT AS A GENUINE CONTRIBUTION, BECAUSE IT IS ONE. A measured BLIND
     physical detection rate, with a false-positive arm and a stated trial count, is a number the
     field currently lacks ENTIRELY. Every published speaker-to-microphone figure is an oracle
     best-of-search maximum computed with knowledge of the true bits. DeAR's 92.68% at 1 m, taken at
     face value with the optimistic independence assumption the room violates, is
     P(all 100 bits) = 5.0e-4, one capture in two thousand yielding the message verbatim without
     ECC. Nobody publishes the blind number. If Audio Provenance runs the campaign and the answer is "no,
     not at any distance we can defend", that is a real, citable, reusable result, and it cost under
     six weeks and about $1,500 rather than the ten weeks of training a paper-shaped schedule would
     have spent to arrive at the same place less certainly.

  6. WHAT SURVIVES A KILL. The Stage 0 IR corpus, the measured transducer responses, and the room
     noise recordings are owned outright, are useful to any future audio work, and do not expire.
     The distortion layer, the licence gating, the blind-by-construction detector API and the model
     card contract are reusable. Keep them.

=====================================================================
8. THE ODDS, BLUNTLY
=====================================================================
PRESENCE DETECTION AT ROUGHLY 1 m IN A QUIET ROOM: PLAUSIBLE. This is the tier to lead with and the
one most likely to ship if anything does. The evidence for it is real and narrow: RAW-Bench's
retrain result put reverberation inside the training loop and lifted a SPECTRAL-domain model's
full-message accuracy under strict simulated reverb from 0.45 to 0.95, while the same treatment
moved a WAVEFORM-domain model only from 0.22 to 0.41. That is direct evidence that reverb robustness
is trainable and that it is trainable much better in the spectral domain, which is why this design is
spectral. It is also the ONLY encouraging datum in the entire survey, it was produced with a
proprietary ~1250 h corpus and training code that is still unreleased, so IT IS NOT REPRODUCIBLE. It
justifies an experiment. It does not justify a schedule commitment, and it says nothing about a
BLIND detector, which is a strictly harder problem than the accuracy figure it reports.

The presence tier's own weakest point is stated in the spec and I agree with it: presence is a
threshold on an integrated score with NO CRC BEHIND IT. Its false-positive rate is a purely empirical
quantity, calibrated on simulation and only confirmed on a few hundred physical captures. That is
the number most likely to sink the tier that is otherwise most likely to ship.

A USABLE LOCATOR PAYLOAD AT CONVERSATIONAL DISTANCE: UNLIKELY. Three independent reasons, none of
which the training run can argue away:
  - The rate budget is already a 4.7x discount on DeAR's oracle 8.8 bits/s, and DeAR's number is a
    best-of-6000-offsets maximum in a varechoic chamber on unstated content.
  - The best simulated-replay result in the literature (AWARE, BER 0.024 at 16 bps) still has
    FNR 0.11, one watermarked clip in nine missed outright, rising to FNR 0.58 at 20 bps. It is also
    speech-only at 16 kHz, and its "band-pass 50 Hz - 8 kHz to reflect microphone and loudspeaker
    characteristics" is a NO-OP at 16 kHz because Nyquist is 8 kHz. It models no transducer roll-off
    at all, which is exactly the stage this design calls load-bearing.
  - Even a locator that WORKS does not produce an identity. 25 bits over 2^25 buckets at 2^24
    registered works leaves 1 - e^-0.5 = 39.3% of works sharing a bucket, and NEITHER of GenoMark-Q's
    two disambiguators is available on the path N exists for: the content hash cannot match after a
    room, and Q returned nothing, which is why N ran. So roughly two in five acoustic locator hits at
    full registry scale resolve to two or more candidates with no way to choose. That is not a defect
    to fix later; it is what 25 bits buys.

ANYTHING AT VENUE DISTANCE, IN A POCKET, OR IN A NOISY ROOM: NOT HAPPENING. Beyond 2 m at any RT60,
above ~45 dBA background, a microphone not pointed at the source, laptop speakers at low SPL where
the 211 Hz band floor is barely reproduced, captures under 20 s for the locator or under 10 s for
presence, playback speed or pitch changes beyond +/- 0.5%: all declared failures in advance. And
NEURAL CODECS, where RAW-Bench establishes 0.00 full-message accuracy for every method tested and
that retraining does not fix it, because watermarks and neural codecs compete for the same perceptual
headroom.

THE SHAPE OF THE HONEST ANSWER, if everything works: a phone lying on a desk in front of a monitor.
Not a phone in a pocket, not the back of a room, not a venue.

AND THE DISJOINTNESS ARGUMENT CUTS BOTH WAYS. GenoMark-Q survives the transcode chain and dies in the
room. GenoMark-N is intended to survive the room and will very likely die in a neural codec. That
disjointness is the entire argument for shipping both, and it is simultaneously an admission that
neither covers the union.

MY OWN CONFIDENCE, since you should not have to infer it: I would put presence-at-1 m at somewhere
around even odds conditional on Stage 0's baselines not being at chance, locator-at-1 m at
distinctly worse than even, and I would not bet on either before K0 and the AudioSeal/WavMark blind
baselines exist, because those two numbers are the only ones that would actually move my estimate.
Nobody in the field has them. That is the whole reason Stage 0 comes first.
