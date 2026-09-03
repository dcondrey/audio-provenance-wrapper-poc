//! Robustness and perceptual bench for audio watermarking.
//!
//! The bench exists before the watermark it measures. A mark that round-trips clean audio and dies
//! on every real distribution path is indistinguishable from a working one until something measures
//! it, so this crate is written and validated first.
//!
//! # What it measures
//!
//! A corpus crossed with a channel matrix. For each cell it embeds a known payload, degrades the
//! audio, and detects: exact-payload recovery rate, bit error rate, the rate at which the detector
//! declines to answer, and detection wall time. It also runs the detector over UNMARKED audio
//! through the same channel, which is the false-positive arm and is not optional.
//!
//! # The acoustic rows are simulated
//!
//! [`channel::acoustic::AcousticRerecord`] convolves with a synthesised impulse response
//! (exponentially decaying noise plus discrete early reflections), applies a fixed transducer curve,
//! adds Gaussian room noise, and applies a small clock drift. That is a model of a room, not a room.
//! It is NOT a substitute for playing a file through a loudspeaker and capturing it with a
//! microphone, and no output of this crate may be described as an over-the-air measurement. The
//! disclaimer travels in the channel parameters and in the report so it cannot be separated from the
//! number by copy-and-paste.
//!
//! # The perceptual numbers are not PEAQ
//!
//! [`perceptual`] reports segmental SNR and a masking-threshold-weighted noise-to-mask ratio. Both
//! are model outputs with stated assumptions. Neither is ITU-R BS.1387, neither yields an ODG, and
//! neither substitutes for a blinded listening test. See [`perceptual::PERCEPTUAL_LIMITS`].
//!
//! # Portability
//!
//! Every process launch and filesystem touch goes through [`ports::CommandRunner`] and
//! [`ports::FileStore`]. Nothing in the DSP, measurement or reporting code calls `std::process`,
//! `std::fs`, or a clock. The runner itself is native-only and sits behind the `native` feature,
//! because timing a detector is what it is for.

pub mod attacks;
pub mod audio;
pub mod channel;
pub mod corpus;
pub mod dsp;
pub mod error;
pub mod fixtures;
pub mod null;
pub mod perceptual;
pub mod ports;
pub mod report;
pub mod watermark;

#[cfg(feature = "native")]
pub mod runner;

pub use channel::{Channel, ChannelFamily};
pub use error::{AudioError, BenchError, ChannelError, CodecError, PortError};
pub use report::{BenchReport, ChannelExpectation, RowVerdict, Thresholds};
pub use watermark::{Detection, WatermarkCodec};

pub use corpus::{CorpusFeed, PreloadedCorpus, WavDirectoryCorpus};
pub use null::{NullConfig, NullCorpusProvenance, NullTestSummary, run_null};
#[cfg(feature = "native")]
pub use runner::{BenchConfig, run};
