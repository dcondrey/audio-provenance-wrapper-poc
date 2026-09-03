# GenoMark-N ONNX contract (`genomark-n-onnx-contract/1`)

**Read this before writing `crates/genomark-n`.** At the time this was written that crate did not
exist, so the contract below is defined here and is the one the Rust side must match. The machine
-readable form is emitted by `export_onnx.py` next to the weights as `onnx_contract.json`; this
document is the prose. If the two ever disagree, the JSON produced by the export that shipped the
weights wins, because it carries the digests.

Everything here follows `docs/GENOMARK_N_SPEC.md` section 7.2.

## The split

The exported graphs are **pure convolution over log-magnitudes**. The Rust side owns:

- windowing, FFT and inverse FFT (`realfft` 3.5.0, the square-root Hann WOLA path GenoMark-Q uses),
- the residual-only resample (`rubato` 5.0.0) of spec 2.2,
- band slicing and the log,
- the **perceptual budget** `B[t,f]`,
- applying `exp(g)` to the complex bins and WOLA resynthesis,
- the sliding-window schedule, the CRC and the flip search.

Consequence: the model file contains no control flow, no dynamic shape beyond the frame count, and
nothing that can differ between runtimes except arithmetic.

## Front end (Rust)

| quantity | value |
|---|---|
| sample rate | 48 000 Hz (model only; the host is never resampled, spec 2.2) |
| `n_fft` | 2048 |
| `hop` | 512 |
| window | square-root Hann, periodic; WOLA synthesis |
| lead pad | **2048 zeros, the WINDOW LENGTH, not half of it** |
| frame count | `ceil((2048 + samples) / 512)` |
| frames/s | 93.75 |
| bin width | 23.4375 Hz |
| band | bins **9..328 inclusive** = **320 bins** = **210.9375 Hz .. 7687.5 Hz** |
| graph input | `log(max(abs(X[band, t]), 1e-7))` |

Frame `t` reads `padded[t*512 .. t*512 + 2048]`, i.e. host samples `[t*512 - 2048, t*512)`, so its
centre sits at `t*512 - 1024` and the first four frames lie mostly in the lead pad. This is
`crates/genomark-n`'s framing, and it is NOT `torch.stft(center=True)`, which pads 1024 and would
put every frame index four hops out of step while the model card claimed otherwise. The Rust
`ModelCard::validate` refuses a card whose `lead_pad_samples` is not 2048.

Bins outside the band are returned bit-identical: spec 2.4 means that literally.

## Encoder graph — `encoder.onnx`

| tensor | direction | shape | dtype |
|---|---|---|---|
| `log_magnitude` | in | `[1, 1, 320, frames]` | float32 |
| `message_bits` | in | `[1, 56]`, values 0.0 or 1.0 | float32 |
| `log_gain_raw` | out | `[1, 1, 320, frames]`, range `[-1, 1]` | float32 |

Apply:

```
g[t,f]  = log_gain_raw[t,f] * B[t,f]        # B in nepers, from the COVER alone
|X'|    = |X| * exp(g)                       # phase unchanged
```

`frames` is the only dynamic axis. The encoder must be run on the **cover**, and the same `B` used
to scale its output.

## Decoder graph — `decoder.onnx`

| tensor | direction | shape | dtype |
|---|---|---|---|
| `log_magnitude` | in | `[1, 1, 320, frames]` | float32 |
| `presence_logit` | out | `[1, frames]` | float32 |
| `bit_logit` | out | `[1, 56]` | float32 |
| `bit_trace` | out | `[1, 56, frames]` | float32 |

`bit_logit` is the pooled read over the whole input. `bit_trace` is the per-frame difference
`a_i^+[t] - a_i^-[t]`, exposed so the Rust side can re-pool over a presence-gated span (spec 4.5)
without a second forward pass: pool `bit_trace` over frames where the smoothed presence probability
exceeds 0.5 and divide by `tau`.

**`tau` is a model constant, not a graph output.** It is in the safetensors as `decoder.log_tau`;
the shipped value is `exp(log_tau)`, floored at 1e-3.

### Deliberate deviation from spec 2.6(b)

The spec writes the bit output as `tanh((mean a+ - mean a-) / tau)` and spec 9.4 then applies BCE
"on the 56 pooled bit logits". A tanh-bounded value used as a BCE logit caps the attainable
probability at `sigmoid(1) = 0.731` and flattens the gradient exactly where the model is confident.
The graph therefore emits the **pre-tanh** quantity as `bit_logit`. `tanh` is strictly increasing,
so neither the hard decision (its sign) nor the confidence ordering the flip search uses (`|.|`)
changes. A consumer wanting the spec's soft bit in `(-1, 1)` applies `tanh` itself.

## Perceptual budget — `budget_tables.json`

`B[t,f] = clamp(kappa * 10^((M[t,f] - L[t,f]) / 20), 0, 0.35)` nepers, computed on the cover, in
Rust, **not** in the graph. Two reasons: spec 2.9's op set does not include the log and power terms,
and a fidelity bound buried in opaque weights cannot be audited.

`budget_tables.json` ships everything needed to reproduce it bit for bit on the model's grid:
`band_of_bin` (320 entries), `band_centres_bark`, `spreading_matrix_linear`
(`10^(spreading_db(z_b - z_j)/10)`), `masking_offset_linear`, `kappa`, `b_max_nepers`. The model is
the same Schroeder / half-Bark / `alpha = 0.5` model `crates/audio-provenance-bench/src/perceptual.rs`
implements, ported in `src/genomark_n/perceptual.py`. Spec 9.4 requires the training objective and
the bench measurement to be the same function.

## Detector schedule

30 s windows at 15 s hop, at most 64 windows per file. Within a window the message pooling is
restricted to frames where the smoothed presence probability exceeds 0.5 after a 0.5 s median
smooth; a window whose presence-positive span is under 8 s does not attempt a message read.

The flip search is **fixed**: ordered-statistics, `k = 2` over the 4 least-confident bits, giving
`1 + 4 + 6 = 11` CRC trials per window, independent of the logit values. `64 * 11 * 2^-24 = 4.2e-5`
false accepts per file. Widening the search without widening the CRC is how a false identity ships.

## Payload

| bits | field |
|---|---|
| 0..2 | version (currently 1) |
| 3..6 | namespace (identical semantics to GenoMark-Q) |
| 7..31 | locator prefix: the leading 25 bits of the same SHA-256 GenoMark-Q's 48-bit locator prefixes |
| 32..55 | CRC-24/OPENPGP, poly `0x864CFB`, init `0xB704CE`, over bits 0..31 |

Check value: `crc24(b"123456789") == 0x21CF02`.

## Opset

`export_onnx.py` requests **17** (spec 7.1). On torch 2.13 / onnxscript 0.7.1 the exporter emits
**18** and the down-conversion of the axes-as-input `ReduceMean` form fails, so the file is left at
18 rather than shipping an unverified conversion. This is recorded in `onnx_contract.json` under
`opset.deviation`. It is low-stakes because `ort` is an offline cross-check only (spec 7.1); the
shipped runtime is `candle` reading the safetensors file, which carries no opset.

## What the Rust must verify

1. **Digest before deserialise.** `shipped_weights.sha256` in the contract is what the crate pins in
   source (spec 7.5). A swapped model is a forgery vector; an unpinned path is accepted only behind
   an explicit `allow_unpinned_model` escape, off by default and recorded in `findings`.
2. **Port parity (bench row N-B12).** For 200 corpus items through 6 channels, the Rust decoder's 56
   bit logits and per-frame presence logits must agree with the PyTorch reference to max absolute
   error < 1e-3 in fp32, and the accept/reject decision must agree on 100% of trials. The reference
   side of that comparison is `src/genomark_n/pipeline.py::Detector`; the harness does not yet ship
   a dump command for it, which is the open item named in this directory's README.
3. **Band arithmetic.** 320 bins, 23.4375 Hz, 210.9375..7687.5 Hz. `tests/test_contract.py` pins it
   on this side.
