//! Watermark-Q, algorithm id `apw-watermark-lepqim-v1`.
//!
//! Dither-modulated QIM over the trimmed mean of adjacent-cell log-spectral-energy differences.
//! The statistic is a ratio of two neighbouring cells' energies, so a scalar gain cancels in it
//! exactly rather than statistically, and there is no normaliser for a channel to corrupt.
//!
//! # Detection is blind
//!
//! [`Watermark::detect`] takes audio and the profile key. It does not take, and there is no other
//! entry point that takes, the payload it is looking for. A detector handed the true bits measures
//! its own search, which is the defect that invalidates the published acoustic-watermarking
//! numbers this design deliberately does not chase.
//!
//! # Cross-block soft combining
//!
//! Every block of a key epoch carries the same coded word under the same dither, so the per-coded-
//! bit soft metric of every block in a file adds. When no single block passes the CRC on its own,
//! [`Watermark::detect`] sums that metric over the whole block grid and runs the Viterbi once over
//! the sum. Acceptance does not move: the CRC over the one decoded message is still the only thing
//! that accepts, and the grid a block is aligned to is chosen from the keyed preamble and pilots
//! alone, never from a decode.
//!
//! # Playback rate
//!
//! A free-running clock is handled by a rate search. The two axes a rate moves are searched
//! SEPARATELY, and the reason is measured: the cell edges are integer bins at rate 1, so re-summing
//! them at any other rate splits bins fractionally, and inside the +-0.3% range that model error
//! costs far more than the ~1% cell leak the frequency correction buys back. Coupling the two axes
//! meant every hypothesis that could have fixed the time misalignment wrecked the statistic in the
//! same step; the whole search was inert at +-0.1%. Pitch-preserving time stretch is unsupported.
//!
//! # Acoustic re-recording is unsupported
//!
//! See [`capabilities::ACOUSTIC_RERECORDING`]. Audio played through a loudspeaker and captured by a
//! microphone returns no payload, by design and not by defect.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod analysis;
pub mod block;
pub mod capabilities;
pub mod convolutional;
pub mod crc32c;
pub mod detect;
pub mod embed;
pub mod error;
pub mod geometry;
pub mod interleaver;
pub mod keystream;
pub mod params;
pub mod payload;
pub mod spectrogram;
pub mod statistic;
pub mod validate;

#[cfg(feature = "bench")]
pub mod bench;

pub use capabilities::{ACOUSTIC_RERECORDING, Capabilities};
pub use detect::{ConfidenceClass, DetectionOutcome};
pub use embed::{BinGain, EmbedReport};
pub use error::WatermarkError;
pub use params::{ALGORITHM_ID, DELTA, PAYLOAD_BITS, PAYLOAD_BYTES};
pub use payload::Payload;

use audio_provenance_audio::AudioBuffer;

/// One profile: the key the dither lattice, preamble and pilots are derived from, and the namespace
/// selector that key is bound to.
///
/// Namespace 0 uses a PUBLISHED key. The public mark is provenance recovery for cooperative and
/// accidental cases; an informed adversary can estimate and subtract it. It is not tamper
/// resistance and must not be described as such.
#[derive(Debug, Clone)]
pub struct Watermark {
    profile_key: Vec<u8>,
    namespace: u8,
}

impl Watermark {
    pub fn public() -> Self {
        Self {
            profile_key: params::PUBLIC_PROFILE_KEY.to_vec(),
            namespace: 0,
        }
    }

    pub fn keyed(profile_key: Vec<u8>, namespace: u8) -> Result<Self, WatermarkError> {
        if profile_key.is_empty() {
            return Err(WatermarkError::EmptyProfileKey);
        }
        if namespace > 0xF {
            return Err(WatermarkError::PayloadFieldRange {
                field: "namespace",
                found: u64::from(namespace),
                max: 0xF,
                bits: 4,
            });
        }
        Ok(Self {
            profile_key,
            namespace,
        })
    }

    pub const fn namespace(&self) -> u8 {
        self.namespace
    }

    pub fn capabilities(&self, sample_rate: u32) -> Capabilities {
        Capabilities::at(sample_rate)
    }

    /// Writes a marked copy. The hard binding in a manifest is computed over THIS output, never
    /// over the input: embed, then hash, then sign.
    pub fn embed(
        &self,
        audio: &AudioBuffer,
        payload: Payload,
    ) -> Result<AudioBuffer, WatermarkError> {
        embed::embed_into(&self.profile_key, self.namespace, audio, payload, false)
            .map(|(marked, _)| marked)
    }

    /// Marked copy plus what the embedder had to do to get there, including the two-pass closure
    /// residual. Degradation is recorded, never silent.
    pub fn embed_measured(
        &self,
        audio: &AudioBuffer,
        payload: Payload,
    ) -> Result<(AudioBuffer, EmbedReport), WatermarkError> {
        embed::embed_into(&self.profile_key, self.namespace, audio, payload, true)
    }

    /// Blind detection. Returns what the CRC accepts, or nothing.
    pub fn detect(&self, audio: &AudioBuffer) -> Result<DetectionOutcome, WatermarkError> {
        detect::detect_in(&self.profile_key, self.namespace, audio)
    }
}
