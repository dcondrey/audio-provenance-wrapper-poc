use crate::geometry::frame_length;
use crate::params::{
    ALGORITHM_ID, BLOCK_SLOTS, FRAMES_PER_SLOT, MAX_SAMPLE_RATE, MEASURED_RATE_RANGE,
    MIN_SAMPLE_RATE, PAYLOAD_BITS, SEARCHED_RATE_RANGE,
};

/// Acoustic re-recording is the literal string `"unsupported"`, never `None`.
///
/// `None` reads as "not yet measured" and invites hope. No published method demonstrates blind,
/// CRC-gated, ground-truth-free recovery over a speaker-to-microphone path at any distance; every
/// published physical number is an oracle best-of search computed with knowledge of the true bits.
pub const ACOUSTIC_RERECORDING: &str = UNSUPPORTED;

pub const UNSUPPORTED: &str = "unsupported";

pub const BAND_LOW_HZ: f64 = 861.3;
pub const BAND_HIGH_HZ: f64 = 4306.6;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capabilities {
    pub algorithm: &'static str,
    pub payload_bits: usize,
    pub sample_rate_range: (u32, u32),
    pub band_hz: (f64, f64),
    pub acoustic_rerecording: &'static str,
    /// Filled only by a completed PEAQ campaign and a blinded ABX panel; neither has run.
    pub measured_transparency: Option<f64>,
    /// Filled only by a completed robustness campaign against the product corpus.
    pub measured_survival: Option<f64>,
    pub block_seconds: f64,
    pub guaranteed_seconds: f64,
    pub strong_class_seconds: f64,
    /// Playback-rate deviation the detector searches, as a fraction.
    pub searched_rate_range: f64,
    /// Playback-rate deviation recovery was measured to survive, over the whole bench corpus and
    /// every channel. Smaller than the searched range on purpose: the grid is allowed a margin, the
    /// claim is not, and this claim is bounded by the largest drift the matrix applies rather than
    /// by the widest rate anything has ever decoded at.
    pub measured_rate_range: f64,
    /// Pitch-preserving time stretch and independent pitch shift move the time and frequency axes
    /// by different factors and break the single-rate model. Deliberately unimplemented.
    pub time_stretch: &'static str,
}

impl Capabilities {
    pub fn at(sample_rate: u32) -> Self {
        let hop = frame_length(sample_rate) / 2;
        let block = (BLOCK_SLOTS * FRAMES_PER_SLOT * hop) as f64 / f64::from(sample_rate);
        Self {
            algorithm: ALGORITHM_ID,
            payload_bits: PAYLOAD_BITS,
            sample_rate_range: (MIN_SAMPLE_RATE, MAX_SAMPLE_RATE),
            band_hz: (BAND_LOW_HZ, BAND_HIGH_HZ),
            acoustic_rerecording: ACOUSTIC_RERECORDING,
            measured_transparency: None,
            measured_survival: None,
            block_seconds: block,
            guaranteed_seconds: block * 2.0,
            strong_class_seconds: block * 3.0,
            searched_rate_range: SEARCHED_RATE_RANGE,
            measured_rate_range: MEASURED_RATE_RANGE,
            time_stretch: UNSUPPORTED,
        }
    }
}
