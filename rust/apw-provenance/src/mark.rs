use std::path::Path;

pub use crate::pcm::{DescriptorError, PcmFormat, PcmReader};

pub const MARK_WINDOW_SECONDS: f64 = 0.25;
pub const MARK_WINDOW_COUNT: usize = 24;
pub const MARK_MATCH_THRESHOLD: f64 = 0.90;
pub const MARK_BUCKET_TOLERANCE: i64 = 1;

const RMS_BUCKETS: i64 = 64;
const RMS_FLOOR_DB: f64 = -80.0;
const ZCR_BUCKETS: i64 = 48;
const ZCR_FLOOR_LOG10: f64 = -5.0;
// REQUIRED: -0.30103, not -LOG10_2. The Python descriptor uses the truncated
// literal, and a more accurate constant moves bucket boundaries, so a mark
// recorded by one implementation stops matching the other.
#[allow(clippy::approx_constant)]
const ZCR_CEIL_LOG10: f64 = -0.30103;
const CREST_BUCKETS: i64 = 24;
const CREST_CEIL: f64 = 12.0;

const MIN_FRAMES_PER_WINDOW: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowFeature {
    pub rms: f64,
    pub zcr: f64,
    pub crest: f64,
}

/// Where the soft binding gets its per-window features.
///
/// TODO: apw-audio owns the routed/export feature extractor. When that crate
/// lands, an adapter over it should replace `PcmFeatureSource` here; the trait
/// exists so that swap does not change the binding's quantisation or its
/// recorded descriptors.
pub trait FeatureSource: Send + Sync {
    fn feature_sequence(
        &self,
        path: &Path,
        window_seconds: f64,
        max_windows: usize,
    ) -> Result<Vec<WindowFeature>, DescriptorError>;
}

/// Streaming feature extraction over uncompressed PCM WAV and AIFF.
#[derive(Debug, Clone, Copy, Default)]
pub struct PcmFeatureSource;

impl FeatureSource for PcmFeatureSource {
    fn feature_sequence(
        &self,
        path: &Path,
        window_seconds: f64,
        max_windows: usize,
    ) -> Result<Vec<WindowFeature>, DescriptorError> {
        let mut reader = PcmReader::open(path)?;
        let format = reader.format();
        let frames_per_window = frames_per_window(format, window_seconds);
        let mut sequence = Vec::with_capacity(max_windows.min(MARK_WINDOW_COUNT));
        for _ in 0..max_windows {
            let mono = reader.read_mono_frames(frames_per_window)?;
            if mono.len() < frames_per_window {
                break;
            }
            sequence.push(feature_of(&mono));
        }
        Ok(sequence)
    }
}

fn frames_per_window(format: PcmFormat, window_seconds: f64) -> usize {
    let target = (f64::from(format.sample_rate) * window_seconds).round_ties_even();
    let frames = if target.is_finite() && target > 0.0 {
        target as usize
    } else {
        0
    };
    frames.max(MIN_FRAMES_PER_WINDOW)
}

fn feature_of(samples: &[f64]) -> WindowFeature {
    if samples.is_empty() {
        return WindowFeature {
            rms: 0.0,
            zcr: 0.0,
            crest: 0.0,
        };
    }
    let sum_squares: f64 = samples.iter().map(|value| value * value).sum();
    let rms = (sum_squares / samples.len() as f64).sqrt();
    let crossings = samples
        .windows(2)
        .filter(|pair| {
            let previous = pair.first().copied().unwrap_or_default() >= 0.0;
            let current = pair.get(1).copied().unwrap_or_default() >= 0.0;
            previous != current
        })
        .count();
    let zcr = if samples.len() > 1 {
        crossings as f64 / (samples.len() - 1) as f64
    } else {
        0.0
    };
    let peak = samples
        .iter()
        .fold(0.0f64, |peak, value| peak.max(value.abs()));
    let crest = if rms > 1e-9 { peak / rms } else { 0.0 };
    WindowFeature { rms, zcr, crest }
}

/// Quantise loudness, zero-crossing rate and crest per window.
///
/// IMPORTANT: RMS and ZCR are bucketed in the log domain. Bucketed linearly on
/// [0,1] they crowd into a handful of buckets and two unrelated tones quantise
/// identically, which made the soft binding match anything.
pub fn quantise_descriptor(sequence: &[WindowFeature]) -> Vec<i64> {
    let mut descriptor = Vec::with_capacity(sequence.len() * 3);
    for feature in sequence {
        let rms_db = 20.0 * feature.rms.max(1e-9).log10();
        let zcr_log = feature.zcr.max(1e-9).log10();
        descriptor.push(bucket((rms_db - RMS_FLOOR_DB) / -RMS_FLOOR_DB, RMS_BUCKETS));
        descriptor.push(bucket(
            (zcr_log - ZCR_FLOOR_LOG10) / (ZCR_CEIL_LOG10 - ZCR_FLOOR_LOG10),
            ZCR_BUCKETS,
        ));
        descriptor.push(bucket(
            (feature.crest - 1.0) / (CREST_CEIL - 1.0),
            CREST_BUCKETS,
        ));
    }
    descriptor
}

fn bucket(unit_value: f64, buckets: i64) -> i64 {
    let clamped = if unit_value.is_nan() {
        0.0
    } else {
        unit_value.clamp(0.0, 1.0)
    };
    (clamped * (buckets - 1) as f64) as i64
}

/// Fraction of descriptor dimensions agreeing within [`MARK_BUCKET_TOLERANCE`].
///
/// An averaged distance lets a large disagreement in one feature hide behind
/// agreement in the others; per-dimension agreement does not.
pub fn descriptor_similarity(left: &[i64], right: &[i64]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let agreeing = left
        .iter()
        .zip(right.iter())
        .filter(|(a, b)| (**a - **b).abs() <= MARK_BUCKET_TOLERANCE)
        .count();
    agreeing as f64 / left.len().max(right.len()) as f64
}

pub fn mark_mechanism() -> String {
    format!(
        "keyed HMAC over a quantised RMS/ZCR/crest descriptor of the first \
         {MARK_WINDOW_COUNT} windows of {MARK_WINDOW_SECONDS}s, recorded in a local \
         append-only side index"
    )
}

/// What this soft binding does NOT claim.
///
/// REQUIRED: nothing here asserts inaudibility or transcode survival, because
/// neither has been measured. Callers publish this list verbatim.
pub fn mark_limits() -> Vec<String> {
    vec![
        "No audio samples are altered, so inaudibility is not claimed and not applicable."
            .to_string(),
        "Survival through lossy transcoding, resampling, or re-recording is unmeasured."
            .to_string(),
        "Recovery requires this machine's side index; the mark is not carried by the asset."
            .to_string(),
        "Only uncompressed PCM WAV and AIFF assets can be described.".to_string(),
        format!(
            "Recovery is a nearest-descriptor match: at least {:.0}% of descriptor dimensions \
             within {MARK_BUCKET_TOLERANCE} bucket. It is evidence of resemblance, not of \
             identity.",
            MARK_MATCH_THRESHOLD * 100.0
        ),
        "Two takes that are near-identical in loudness, zero-crossing rate and crest quantise \
         alike and will collide; a verdict reached through the mark alone is reported with proof \
         level 'inferred'."
            .to_string(),
    ]
}
