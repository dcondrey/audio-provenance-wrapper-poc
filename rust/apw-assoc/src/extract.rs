use std::path::Path;

use crate::error::{AssocError, Result};
use crate::feature::{decode_pcm, ExtractionDetails, Feature};
use crate::pyround::python_round_to_i64;
use crate::wav::{PcmSource, WavSource};

/// The floor on window length; a target shorter than this is widened so a
/// window always carries enough samples to score.
pub const MIN_WINDOW_FRAMES: u64 = 128;

/// Route an export path to a reader by suffix.
///
/// AIFF is deliberately absent: `apw-audio` owns the AIFF chunk walk, and this
/// crate takes frames through `PcmSource` rather than growing a second parser.
pub fn open_pcm(path: &Path) -> Result<Box<dyn PcmSource>> {
    let suffix = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match suffix.as_str() {
        "wav" => Ok(Box::new(WavSource::open(path)?)),
        "aif" | "aiff" => Err(AssocError::AiffReaderUnavailable),
        _ => Err(AssocError::UnsupportedSuffix),
    }
}

pub fn extract_feature_sequence(
    path: &Path,
    target_window_seconds: f64,
    max_windows: usize,
) -> Result<(Vec<Feature>, ExtractionDetails)> {
    let mut source = open_pcm(path)?;
    extract_from_source(source.as_mut(), target_window_seconds, max_windows)
}

/// Stream bounded, mono-mixed features equivalent to routed plug-in features.
pub fn extract_from_source(
    source: &mut dyn PcmSource,
    target_window_seconds: f64,
    max_windows: usize,
) -> Result<(Vec<Feature>, ExtractionDetails)> {
    let sample_rate_hz = source.sample_rate_hz();
    let channel_count = source.channel_count();
    let sample_width_bytes = source.sample_width_bytes();
    if sample_rate_hz == 0 || channel_count == 0 {
        return Err(AssocError::InvalidFormatMetadata);
    }
    let scaled = f64::from(sample_rate_hz) * target_window_seconds;
    let Some(rounded) = python_round_to_i64(scaled) else {
        return Err(AssocError::InvalidWindowLength);
    };
    let frames_per_window = (rounded.max(0) as u64).max(MIN_WINDOW_FRAMES);
    let frame_size = u64::from(channel_count) * u64::from(sample_width_bytes);
    let channels = usize::from(channel_count);

    let mut sequence: Vec<Feature> = Vec::new();
    let mut exhausted = false;
    for _ in 0..max_windows {
        let raw = source.read_frames(frames_per_window)?;
        let frame_count = raw.len() as u64 / frame_size.max(1);
        if frame_count < frames_per_window {
            exhausted = true;
            break;
        }
        let decoded = decode_pcm(&raw, sample_width_bytes, source.byte_order())?;
        let mono: Vec<f64> = decoded
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f64>() / channels as f64)
            .collect();
        sequence.push(Feature::from_samples(&mono)?);
    }
    let truncated_at_window_limit = if exhausted {
        false
    } else {
        !source.read_frames(1)?.is_empty()
    };

    Ok((
        sequence,
        ExtractionDetails {
            sample_rate_hz,
            channel_count,
            sample_width_bytes,
            window_size_frames: frames_per_window,
            truncated_at_window_limit,
        },
    ))
}
