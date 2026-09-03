//! Audio substrate for Audio Provenance: decoding, WAV encoding, transforms, metrics,
//! resampling and filtering.
//!
//! Nothing below `fs` touches the filesystem, and `codecs` is the only path that
//! pulls in a demuxer, so the DSP surface compiles for `wasm32-unknown-unknown`
//! with `--no-default-features`.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod biquad;
pub mod buffer;
pub mod convolve;
pub mod decode;
pub mod error;
pub mod fft;
pub mod iff;
pub mod mdct;
pub mod metrics;
pub mod resample;
pub mod stft;
pub mod wav;
pub mod window;

#[cfg(feature = "fs")]
pub mod files;

pub use biquad::{Biquad, BiquadCoefficients};
pub use buffer::{AudioBuffer, MAX_CHANNELS, MAX_SAMPLE_RATE};
pub use convolve::convolve;
pub use decode::{DecodeLimits, decode_bytes};
pub use error::{AudioError, ChunkId};
pub use fft::RealFft;
pub use mdct::{Mdct, MdctTransform};
pub use metrics::{BandEnergies, Spectrum, WindowMetrics, window_metrics};
pub use resample::{ClockDrift, resample, resample_ratio};
pub use stft::{Stft, StftFrames};
pub use wav::BitDepth;
pub use window::Symmetry;
