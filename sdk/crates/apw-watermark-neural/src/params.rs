//! Every constant WATERMARK_N_SPEC.md fixes for the inference side, in one place.

pub const ALGORITHM_ID: &str = "apw-watermark-neural-v1";
pub const CARD_FORMAT: &str = "apw-watermark-neural-model-card/1";

/// The model runs at exactly this rate. Host audio is never resampled; a copy is (spec 2.2).
pub const ANALYSIS_SAMPLE_RATE: u32 = 48_000;
pub const MIN_SAMPLE_RATE: u32 = 32_000;
pub const MAX_SAMPLE_RATE: u32 = 192_000;

pub const N_FFT: usize = 2048;
pub const HOP: usize = 512;
pub const FRAME_RATE_HZ: f64 = ANALYSIS_SAMPLE_RATE as f64 / HOP as f64;

/// Bins 9..=328 at 48 kHz with a 2048-point transform: 210.9375 Hz to 7687.5 Hz (spec 2.4).
pub const BAND_BIN_LOW: usize = 9;
pub const BAND_BIN_HIGH: usize = 328;
pub const BAND_BINS: usize = BAND_BIN_HIGH - BAND_BIN_LOW + 1;
pub const LOG_FLOOR: f32 = 1e-7;

pub const MESSAGE_BITS: usize = 56;
pub const VERSION_BITS: u32 = 3;
pub const NAMESPACE_BITS: u32 = 4;
pub const LOCATOR_PREFIX_BITS: u32 = 25;
pub const CRC_BITS: usize = 24;
pub const PAYLOAD_BITS: usize = MESSAGE_BITS - CRC_BITS;
pub const PAYLOAD_BYTES: usize = PAYLOAD_BITS / 8;
pub const PAYLOAD_VERSION: u8 = audio_provenance_core::WATERMARK_PAYLOAD_VERSION;

/// Ordered-statistics flip search, k = 2 over the 4 least confident bits: 1 + 4 + 6 patterns
/// (spec 3.2). Widening either number invalidates the false-accept count in spec 3.4 and forces
/// the CRC width to be redone with it.
pub const FLIP_SEARCH_BITS: usize = 4;
pub const FLIP_SEARCH_WEIGHT: usize = 2;
pub const FLIP_PATTERNS: usize = 11;

/// Hard ceiling on the per-bin log-gain, in nepers (spec 2.7). No calibration may raise it.
pub const MAX_BUDGET_NEPERS: f64 = 0.35;

/// Confidence caps from spec 5.4. Not tuning parameters: they are raised only when a blind
/// physical false-positive rate exists to justify it.
pub const CONFIDENCE_LOCATOR_MULTI: f64 = 0.85;
pub const CONFIDENCE_LOCATOR_SINGLE: f64 = 0.70;

/// The literal string, never `None` and never a boolean. `None` reads as "not yet measured" and
/// invites hope; a boolean is what lets marketing outrun engineering (spec 8.6).
pub const UNSUPPORTED: &str = "unsupported";
pub const MEASURED_LIMITED: &str = "measured_limited";

/// Gate values the envelope validator enforces, straight from the kill criteria.
pub const MIN_CALIBRATION_TRIALS: u64 = 5_000;
pub const MAX_PRESENCE_FALSE_POSITIVE_RATE: f64 = 1e-3;
pub const MIN_PRESENCE_RECALL: f64 = 0.80;
pub const MIN_BLIND_DETECTION_RATE: f64 = 0.50;
pub const MIN_DISTANCE_M: f64 = 0.5;
pub const MAX_DURATION_S: f64 = 45.0;
pub const MIN_ROOMS: u32 = 3;
pub const MIN_SPEAKERS: u32 = 2;
pub const MIN_MICROPHONES: u32 = 2;

/// The dense presence pass is run in bounded chunks with a discarded margin on each side rather
/// than in one call over the whole file. The decoder is fully convolutional, so a frame further
/// than the receptive field from a chunk edge has the same value it would have in a single dense
/// pass; the margin is set far wider than the spec 2.6 trunk's reach (four stride-2 stages with
/// time dilations up to 8, tens of frames). Without this a ten-minute file asks a 256-channel
/// U-Net for one activation tensor of several hundred gigabytes.
pub const DENSE_CHUNK_SECONDS: f64 = 60.0;
pub const DENSE_CHUNK_MARGIN_SECONDS: f64 = 2.0;
