# genomark-n

Rust inference for GenoMark-N (`genomark-neural-v1`), the LEARNED acoustic mark specified in
`docs/GENOMARK_N_SPEC.md`. It is ADDITIVE: a second mark alongside GenoMark-Q, never a replacement.

**Nothing here is a measured capability.** No trained model exists, no physical capture campaign has
run, and `capabilities().acoustic_rerecording` is the literal `"unsupported"`. What is finished is
the path a trained model drops into: model-card load with a pinned weights digest, a shape-checked
ONNX session, the Rust-side STFT, and a pooled shift-equivariant detector with no alignment search.

## The contract the Python side must match

`src/lib.rs`'s crate documentation carries the numbered STFT contract, and it is the authority. It
is deliberately not restated here, because two copies diverge. Read it with:

```
cargo doc -p genomark-n --open
```

Every clause is also declared in the model card's `transform` block and checked against this build's
constants at load time, so an export that disagrees is refused rather than silently misbehaving.

Summary of the graph interface only:

| graph   | input                            | output                                          |
|---------|----------------------------------|-------------------------------------------------|
| decoder | `log_mag` f32 `[1, 1, 320, T]`   | `presence_logit` `[1, T]`, `message_logit` `[1, 56]` |
| encoder | `log_mag` `[1, 1, 320, T]`, `message` `[1, 56]` | `gain_raw` `[1, 1, 320, T]`, values in `[-1, 1]` |

`T` MUST be a symbolic dimension in the export. The encoder emits DIRECTION only; the Rust side
multiplies by the perceptual budget, so fidelity is a property of the budget rather than a hoped-for
outcome of a loss weight.

## Runtime divergence from the spec

Spec section 7.1 names `candle` with `.safetensors` as the shipping runtime and keeps ONNX Runtime
as an offline cross-check. This crate implements the ONNX/`ort` path. `download-binaries` is the
development default and is exactly what 7.1 rules out for a shippable SDK; `load-dynamic` shifts to
a host-supplied library named by `ORT_DYLIB_PATH`. The two are MUTUALLY EXCLUSIVE and enabling both
is a compile error, so `--all-features` is not a valid configuration for this crate. Gate with:

```
cargo test  -p genomark-n
cargo clippy -p genomark-n --all-targets -- -D warnings
cargo clippy -p genomark-n --all-targets --no-default-features --features bench,load-dynamic -- -D warnings
```

The `candle` port and the section 7.4 parity gate (row N-B12) are Stage 3 work that this crate does
not close.

## Fixtures

`fixtures/*.onnx` are hand-written arithmetic graphs with no learned parameters. They exist so the
inference path is executed and asserted before any trained model exists. Regenerate with:

```
uv run --with onnx --with numpy python tools/make_fixture_models.py
```

The script rewrites the digests in `fixtures/genomark-n-fixture-v1.card.json`, which the loader then
verifies. A card marked `fixture` may not carry an operating envelope, and `is_bench_fixture()`
returns true so every bench row it produces is labelled.
