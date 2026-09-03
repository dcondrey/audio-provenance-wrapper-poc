use crate::error::CodecError;
use audio_provenance_audio::AudioBuffer;
use serde::Serialize;
use std::fmt::Debug;

#[derive(Debug, Clone, Serialize)]
pub struct Detection {
    /// `None` means the detector declined to report a payload. Anything else is an accept, and on
    /// unmarked audio an accept is a false positive whatever its confidence says.
    pub payload: Option<Vec<u8>>,
    pub confidence: f64,
    pub bits_corrected: u32,
    pub candidates_examined: u64,
    pub notes: Option<String>,
}

impl Detection {
    pub const fn none() -> Self {
        Self {
            payload: None,
            confidence: 0.0,
            bits_corrected: 0,
            candidates_examined: 0,
            notes: None,
        }
    }

    pub const fn is_accept(&self) -> bool {
        self.payload.is_some()
    }
}

/// What the bench measures. The bench never reaches inside an implementation, so a codec that
/// cheats by remembering what it embedded is measured exactly like one that does not.
pub trait WatermarkCodec: Debug + Send + Sync {
    fn name(&self) -> &str;

    fn payload_len(&self) -> usize;

    /// True for a deliberately simple stand-in used to validate the bench itself. A fixture is
    /// never a product watermark and the report labels it as a fixture on every row.
    fn is_bench_fixture(&self) -> bool;

    fn describe(&self) -> String;

    fn embed(&self, audio: &AudioBuffer, payload: &[u8]) -> Result<AudioBuffer, CodecError>;

    fn detect(&self, audio: &AudioBuffer) -> Result<Detection, CodecError>;
}

pub fn bits_of(bytes: &[u8]) -> Vec<u8> {
    let mut bits = Vec::with_capacity(bytes.len() * 8);
    for byte in bytes {
        for shift in (0..8).rev() {
            bits.push((byte >> shift) & 1);
        }
    }
    bits
}

pub fn bytes_of(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| chunk.iter().fold(0u8, |acc, &bit| (acc << 1) | (bit & 1)))
        .collect()
}

/// Fraction of differing bits between two equal-length payloads. Returns `None` when the lengths
/// differ, because a bit error rate across different payload sizes is not a defined quantity.
pub fn bit_error_rate(expected: &[u8], observed: &[u8]) -> Option<f64> {
    if expected.len() != observed.len() || expected.is_empty() {
        return None;
    }
    let differing: u32 = expected
        .iter()
        .zip(observed.iter())
        .map(|(a, b)| u32::from(a ^ b).count_ones())
        .sum();
    Some(f64::from(differing) / (expected.len() * 8) as f64)
}
