# GenoMark-N training harness

PyTorch harness for `genomark-neural-v1`, the **learned acoustic mark** of
`docs/GENOMARK_N_SPEC.md`. It is additive: a second mark alongside GenoMark-Q, never a replacement.

**Nothing here is a result.** The spec is a gated feasibility program, not a committed feature.
`capabilities().acousticRerecording` stays the literal `"unsupported"` until spec section 12's Stage
2 gate passes on measured physical captures, and the model card emits every physical envelope field
as an explicit `null` under `status: "unmeasured"` so it cannot be quoted otherwise.

## Quick start

```bash
uv sync
uv run pytest -q
./scripts/overfit_check.sh   # the discriminator: does the data path work at all?
./scripts/smoke.sh           # train, calibrate, evaluate, export ONNX, parity fixtures, model card

# the 300-step spec 9.6 smoke, then PyTorch -> ONNX -> Rust:
uv run python -m genomark_n.train --config configs/smoke_300.yaml
uv run python scripts/curve.py runs/smoke300/train_log.jsonl
./scripts/loop_closure.sh configs/smoke_300.yaml runs/smoke300 300
```

`scripts/smoke.sh` completes in about three minutes on CPU. It proves every stage **runs**; it
proves nothing about whether the mark works.

`scripts/overfit_check.sh` is the one that proves something. One clip, one payload, the full marked
span, **the distortion chain entirely bypassed**, so the only thing under test is

```
encode -> budget -> encoder -> gain -> ISTFT -> STFT -> decoder -> pooled logits -> CRC decode
```

Spec 9.3 says Phase A's clean-channel target is bit accuracy > 0.999 and that "a Phase A failure is
a bug, not a result". This is that test, in three minutes instead of 60k steps. Measured on this
machine, 60 steps, 1.5 s clip, CPU:

```
step   20 loss   0.0120 bit_accuracy 1.0000
step   59 loss   0.0014 bit_accuracy 1.0000
final bit accuracy   1.0000
CRC decoded          True
CRC bits == truth    True   bits_corrected 0
RESULT               PASS
```

So the path is wired correctly. It says nothing about whether the mark survives anything.

## The one invariant

Spec 4.1 names the reason no published speaker-to-microphone number means what it appears to mean:
every one of them is an **oracle best-of-search maximum computed with knowledge of the true bits**.
DeAR shifts over a 113 ms range and keeps the best. DeepAWR sweeps 6000 offsets, breaks early on a
perfect hit, and discards the CRC it computes.

This harness makes that bug **unrepresentable**, not merely avoided:

```python
Detector.detect(audio, thresholds) -> Detection      # no payload parameter exists
```

Truth reaches `evaluate.py` only *after* a detection has been accepted or refused, and only to
score. `tests/test_blindness.py` pins the signature. Blindness has three surfaces and all three are
closed:

1. **No offset search.** The readout pools each bit's activation trace over the presence-weighted
   window; pooling is translation-invariant, so there is no frame grid to align to (spec 4.2).
2. **No threshold fitted on the eval set.** `calibrate.py` never sees a marked clip. It produces a
   `FrozenThresholds` record over unmarked audio, carrying its trial count and the 3/N bound that
   count implies, and refuses to hide when the count is too small to support the stated rate.
3. **No channel-layer realignment.** Channels truncate to the input length and never compensate the
   room's propagation delay or a codec's encoder delay, mirroring the Rust bench's own rule that
   "sync is the detector's job".

## Layout

```
src/genomark_n/
  config.py          every spec hyperparameter, typed, with the hard bounds enforced
  seeding.py         seeding + an honest statement of what determinism MPS cannot give
  stft.py            the STFT that lives OUTSIDE the exported graph (spec 7.2)
  perceptual.py      torch port of crates/audio-provenance-bench/src/perceptual.rs + the budget B[t,f]
  payload.py         56-bit message, CRC-24/OPENPGP, the fixed 11-trial flip search
  model.py           encoder U-Net (frequency-only downsampling) and the two-head decoder
  qmark.py           GenoMark-Q's statistic: distortion stage D0 and the L_q hinge
  pipeline.py        Marker, the loss set, FrozenThresholds, and the blind Detector
  channels.py        eval channels named and parameterised to mirror audio-provenance-bench
  rir/               image-source synthesizer (Eyring, windowed-sinc) + licence-gated corpora
  data/              licence gating, manifests, synthetic corpus, the training dataset
  distortion/        spec 6's D0-D11, differentiable, with a real ffmpeg codec round trip
                     (`noise.py` synthesises D7's tilted room floor when MUSAN is not configured)
  rust_card.py       the `genomark-n-model-card/1` record crates/genomark-n loads
  train.py calibrate.py evaluate.py export_onnx.py parity.py model_card.py
docs/ONNX_CONTRACT.md   >>> the contract crates/genomark-n must match <<<
NOTICE                  what was and was not vendored, and the licence read for each
```

## For the author of `crates/genomark-n`

`docs/ONNX_CONTRACT.md` is the contract, and `export_onnx.py` emits its machine-readable twin as
`onnx_contract.json` beside the weights. Short form:

- encoder in `[1,1,320,frames]` log-magnitude + `[1,56]` bits, out `[1,1,320,frames]` in `[-1,1]`;
- decoder in `[1,1,320,frames]`, out `presence_logit [1,frames]`, `bit_logit [1,56]`,
  `bit_trace [1,56,frames]`;
- STFT, band slice, log, budget, gain application and WOLA all belong to Rust;
- band is bins **9..328 inclusive**, 320 bins, 210.9375..7687.5 Hz at 23.4375 Hz per bin, 93.75
  frames/s. `tests/test_contract.py` pins this side of it.

## Deliberate deviations from the spec text, each with its reason

| where | spec says | here | why |
|---|---|---|---|
| decoder output | `tanh((mean a+ - mean a-)/tau)` (2.6b) used as a BCE logit (9.4) | emits the **pre-tanh** value as `bit_logit` | a tanh-bounded logit caps BCE probability at 0.731 and flattens the gradient where the model is confident; tanh is monotone so the sign and the `abs` ordering the flip search uses are unchanged |
| ONNX opset | 17 (7.1) | **18**, recorded in the contract | onnxscript's down-conversion of the axes-as-input `ReduceMean` fails; shipping an unverified conversion is worse than recording the version. `ort` is an offline cross-check only; candle reads safetensors, which carries no opset |
| parameter count | ~5.5 M, ~22 MB fp32 (2.8) | **8.23 M, 32.9 MB fp32** | the channel widths of 2.5/2.6 taken literally give this. Under K7's 40 MB ceiling, above the spec's estimate. Either accept it or narrow `encoder.stage_channels`; the number is measured and printed by the model card, not asserted |
| RIR decay rate | image-source with Eyring absorption | image-source **plus a measured-decay correction** | a rectangular image-source field does not decay at the Eyring rate: reflection count per metre varies with direction, so by Jensen the direction-averaged decay is slower. Uncorrected, a room asked for 0.40 s measures 0.55 s. The geometry sets the reflection pattern; the correction sets the rate; the achieved T30 is measured back and carried on the result |
| analysis length | sampled per example, `U(3 s, 5 s)` (4.4c) | sampled **per batch** | a per-item length cannot be stacked without padding, and padding hands the decoder a silent boundary that is a synchronisation cue no real capture provides |
| STFT framing | `center=True` is never named; spec 7.2 leaves the transform to Rust | **lead pad = 2048, the window length**, frame count `ceil((2048+samples)/512)` | `crates/genomark-n` pads by the window length and `ModelCard::validate` refuses a card claiming otherwise. `torch.stft(center=True)` pads 1024 and would put every frame four hops out of step. The model is shift-equivariant so the choice costs training nothing and costs the contract everything |
| EMA scope | "EMA of the **encoder** weights" (9.5) | EMA over the whole model, decoder included | the decoder is what the frozen threshold is calibrated against, so an EMA encoder paired with a raw decoder would calibrate one model and ship another. Narrow it in `runlog.Ema` if the spec's scope is meant literally |

## Compute, honestly

Spec 9.6 is right and the smoke test confirms the shape of it: MPS is the development and
smoke-test path, not the training path. `train_log.jsonl` records `seconds_per_step` and
`projected_320k_days` on every finished run; kill criterion K8 fires at 14 days. Read that number
from your own smoke run before believing any schedule.

## Two measurements worth carrying forward

**The perceptual budget alone does not enforce K2.** The N-C0 kappa sweep on the eval corpus
(`calibrate --kappa-sweep`) reports every kappa from 0.05 to 0.50 passing K2, because an untrained
encoder does not use its budget:

```
kappa 0.050 nmr_max -18.49 dB  above_mask 0.000  segSNR 41.69 dB  K2 pass
kappa 0.200 nmr_max  -7.26 dB  above_mask 0.000  segSNR 31.64 dB  K2 pass
kappa 0.500 nmr_max  -6.85 dB  above_mask 0.000  segSNR 24.74 dB  K2 pass
```

But the overfit check, where the encoder is trained to maximise recoverability and therefore
saturates its budget, lands at **segmental SNR 14.87 dB at kappa = 0.5** — below K2's 22 dB floor.
Read together: kappa must be calibrated against a *converged* encoder, not an initial one, and
`L_perc` has to be doing real work rather than sitting at zero. Expect the shipped kappa to be
nearer 0.2 than 0.5.

**Wall clock says the same thing spec 9.6 does.** The smoke run's own `train_log.jsonl` records
`seconds_per_step` and `projected_320k_days`; at batch 2 on CPU it projects ~27 days, and batch 16
with the full chain is worse. Kill criterion K8 fires at 14 days. Rent the GPU.

## Known gaps

- **The parity fixtures exist but nothing consumes them yet.** `genomark_n.parity` writes bench row
  N-B12's reference side as raw little-endian f32 blobs plus `parity_manifest.json`. `crates/genomark-n`
  now exists and loads an exported card, but nothing reads the fixtures back, so N-B12 is prepared
  and not run.
- **GroupNorm is not strictly shift-equivariant.** Its statistics are global over `(C, H, W)` per
  sample, so a crop taken at a different offset sees slightly different global statistics.
  `tests/test_equivariance.py` measures it rather than asserting it away: on an untrained model the
  pooled bit logits move ~2e-3 relative under a 137-sample (sub-frame) shift and the dense presence
  trace translates to within ~2e-2 relative under a 5-hop shift, against control deviations 17x and
  20x larger. The spec 4.2 claim is "shift-equivariant to a measured tolerance", not exactly.
- **`resample_linear` is linear interpolation**, not a polyphase resampler. It imposes a mild
  low-pass the real converter does not. Fine for a differentiable training stage, wrong for a
  reported channel; the Rust bench's `rubato` path is the reference for the latter.
- **D7's noise is synthesised, not measured.** `distortion/noise.py` mixes a tilted room floor
  (`U(-6, -3)` dB per octave about 250 Hz, flat below 50 Hz) with a Gaussian floor 20 dB beneath it,
  because a white floor puts far too much power at the top of the 211-7688 Hz band and too little at
  the bottom, which flatters the result exactly where the mark is hardest to place. MUSAN
  (OpenSLR-17, CC BY 4.0) is not on this machine; wire it through `data.noise_manifests` and it is
  added on top of this floor rather than replacing it.
- **Nothing here is a trained model.** The 300-step run in `configs/smoke_300.yaml` is spec 9.6's
  mandatory wall-clock smoke, three orders of magnitude short of spec 9.5's schedule and on
  synthesized audio. Its checkpoint detects nothing; the loader card it produces says so in
  `training_provenance.status`.
