//! GenoMark-N (`genomark-neural-v1`) inference: the Rust host for the learned acoustic mark.
//!
//! # NOTHING HERE IS A MEASURED CAPABILITY
//!
//! GenoMark-N is a GATED FEASIBILITY PROGRAM, not a shipped feature. No trained model exists, no
//! physical capture campaign has run, and `capabilities().acoustic_rerecording` is the literal
//! `"unsupported"` until GENOMARK_N_SPEC.md section 12's Stage 2 gate passes on measured physical
//! captures. This crate is the inference path a future trained model will be dropped into; loading
//! the fixture graphs in `fixtures/` proves the plumbing runs and proves nothing else.
//!
//! It is ADDITIVE. GenoMark-Q (`genomark-lepqim-v1`) is the shipped, measured mark, and N is a
//! second mark alongside it, never a replacement. Q survives the transcode chain and dies in a
//! room; N is intended to survive a room and will very likely die in a neural codec. Neither covers
//! the union, and "N is orthogonal to Q by construction" is false (spec 8.3).
//!
//! # RUNTIME DIVERGENCE FROM THE SPEC, STATED SO IT IS NOT MISTAKEN FOR AGREEMENT
//!
//! GENOMARK_N_SPEC.md section 7.1 names `candle` with `.safetensors` weights as the SHIPPING
//! runtime and keeps ONNX Runtime as an offline cross-check, on the ground that `ort` either
//! downloads a prebuilt library at build time or vendors a large C++ dependency. This crate
//! implements the ONNX/`ort` path. The `download-binaries` feature is the development default and
//! is exactly what section 7.1 rules out for a shippable SDK; `load-dynamic` shifts the runtime to
//! a host-supplied library. A `candle` port and the section 7.4 parity gate (row N-B12) remain
//! Stage 3 work, and this crate does not close them.
//!
//! # THE STFT CONTRACT
//!
//! THE TRANSFORM IS OWNED BY RUST, NOT BY THE GRAPH. `torch.stft` exports to ONNX badly and complex
//! tensors are a standing ONNX pain point, so the exported graph is pure convolution. The PyTorch
//! side MUST reproduce the framing below EXACTLY; a mismatch does not fail, it silently
//! misbehaves. Every clause is re-declared in the model card's `transform` block and checked
//! against this build's constants at load time.
//!
//!  1. RATE. The model runs at exactly 48 kHz. The host is never resampled: a COPY is resampled to
//!     48 kHz for analysis, and on the embed path only the residual is resampled back.
//!  2. CHANNELS. The transform input is the mono sum, `mean over channels`, in ascending channel
//!     order (`AudioBuffer::mono_sum`).
//!  3. WINDOW. `w[n] = sqrt(hann(2048, periodic))`, i.e. the elementwise square root of
//!     `0.5 - 0.5*cos(2*pi*n/2048)` for `n` in `0..2048`. PERIODIC, denominator 2048, NOT 2047.
//!  4. HOP. 512 samples. 93.75 frames per second, 23.4375 Hz per bin.
//!  5. PADDING. 2048 zeros are prepended to the signal and the tail is zero-padded to
//!     `(frames - 1) * 512 + 2048`. Frame `t` reads `padded[t*512 .. t*512 + 2048]`. The lead pad
//!     equals the window length, NOT half of it; this is the detail a Python implementer gets
//!     wrong, and it shifts every frame index by four hops.
//!  6. FRAME COUNT. `T = ceil((2048 + signal_len) / 512)`.
//!  7. TRANSFORM. Real FFT of the windowed frame, 1025 bins, unnormalised forward (no `1/N`).
//!  8. BAND. Bins 9..=328 INCLUSIVE, 320 rows, 210.9375 Hz to 7687.5 Hz. Everything outside is
//!     untouched, exactly.
//!  9. VALUE. `ln(max(|X[t,f]|, 1e-7))`. Natural log of the MAGNITUDE, not of power, not base 10.
//! 10. LAYOUT. `[1, 1, 320, T]` float32, C order: frequency outer, time inner. Element `(f, t)` is
//!     at flat index `f * T + t`.
//! 11. SYNTHESIS. Weighted overlap-add: the same window on synthesis, divided by the overlap-added
//!     squared window. Phase is never touched; the mark is a real positive multiplier `exp(g)` on
//!     the complex bin.
//!
//! The encoder's second input is `[1, 56]` float32 carrying the message bits as 0.0 or 1.0, MSB
//! first, in the order [`payload::Payload::to_message_bits`] emits them.
//!
//! # PAYLOADS MUST CARRY THIS BUILD'S VERSION
//!
//! Spec 3.4 counts a syntactic version check into the false-accept budget, so the decoder refuses
//! any version but its own. A payload carrying a different one could be embedded and never
//! returned, which would read as a model failure in every recovery row, so
//! [`payload::Payload::new`] refuses it at construction and the embed path fails loudly. A bench
//! run against GenoMark-N must supply a payload whose leading 3 bits are the current version;
//! `audio-provenance-bench`'s default payload is not one.
//!
//! # THE DETECTOR HAS NO ALIGNMENT SEARCH, AND NO API THAT COULD ACCEPT ONE
//!
//! Every published speaker-to-microphone number in this field is an ORACLE best-of-search maximum:
//! offsets are swept and the one scoring best AGAINST THE KNOWN PAYLOAD is kept. A deployed
//! detector has no ground truth. So the decoder here is fully convolutional in time and its message
//! read is a global average over the presence-weighted span, which is invariant to translation;
//! bulk delay is a pure translation and clock drift a slow dilation, and pooling absorbs both.
//! NO FUNCTION IN THIS CRATE TAKES AN EXPECTED PAYLOAD, A TIME OFFSET, OR A THRESHOLD AS A
//! DETECTION INPUT. Adding one would make every number this detector produces meaningless.
//!
//! # THRESHOLDS ARE FROZEN IN THE MODEL CARD
//!
//! The presence tier is a threshold on an integrated score with no CRC behind it, so its
//! false-positive rate is a purely empirical quantity. It is calibrated on at least 5000 unmarked
//! simulated trials to an operating point of 1e-3 and then FROZEN; it is never re-tuned to recover
//! recall. The false-positive rate is the constraint and the recall is the result (spec 3.5, K5a).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

// IMPORTANT: these are two mutually exclusive ways to obtain the ONNX Runtime library, and enabling
// both silently selects dynamic loading and then fails at the first session build with a dlopen
// error. `--all-features` is therefore not a valid configuration for this crate; say so at compile
// time rather than at test time.
#[cfg(all(feature = "download-binaries", feature = "load-dynamic"))]
compile_error!(
    "`download-binaries` and `load-dynamic` are mutually exclusive ONNX Runtime linkage \
     strategies; enable exactly one"
);

pub mod budget;
pub mod capabilities;
pub mod card;
pub mod crc24;
pub mod detect;
pub mod embed;
pub mod envelope;
pub mod error;
pub mod model;
pub mod params;
pub mod payload;
pub mod session;
pub mod spectral;
pub mod thresholds;

#[cfg(feature = "bench")]
pub mod bench;

pub use capabilities::{AcousticRerecording, Capabilities};
pub use card::ModelCard;
pub use detect::{DetectionClass, NeuralDetection};
pub use envelope::{AcousticEnvelope, EnvelopeClaim};
pub use error::GenoMarkNError;
pub use model::GenoMarkN;
pub use params::{ALGORITHM_ID, PAYLOAD_BITS, PAYLOAD_BYTES, UNSUPPORTED};
pub use payload::Payload;
pub use thresholds::Thresholds;

#[cfg(feature = "bench")]
pub use bench::GenoMarkNCodec;
