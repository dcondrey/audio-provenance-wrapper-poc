use serde_json::{json, Map, Value};

use crate::error::{AssocError, Result};
use crate::pyround::python_round;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    Little,
    Big,
}

/// One bounded analysis window: the four axes the association compares.
///
/// `crest` and `envelope` are optional because a routed plug-in window may omit
/// them; an axis missing on either side is dropped from the weighted score
/// rather than scored as a mismatch.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Feature {
    pub rms: Option<f64>,
    pub zcr: Option<f64>,
    pub crest: Option<f64>,
    pub envelope: Option<Vec<f64>>,
}

impl Feature {
    /// Mirrors `_number`: a non-finite value is treated as absent.
    pub(crate) fn rms(&self) -> Option<f64> {
        self.rms.filter(|value| value.is_finite())
    }

    pub(crate) fn zcr(&self) -> Option<f64> {
        self.zcr.filter(|value| value.is_finite())
    }

    pub(crate) fn crest(&self) -> Option<f64> {
        self.crest.filter(|value| value.is_finite())
    }

    /// The four-axis feature of one mono window.
    pub fn from_samples(samples: &[f64]) -> Result<Feature> {
        if samples.is_empty() {
            return Ok(Feature {
                rms: Some(0.0),
                zcr: Some(0.0),
                crest: Some(0.0),
                envelope: Some(vec![0.0; 4]),
            });
        }
        let count = samples.len();
        let energy = samples.iter().fold(0.0f64, |total, value| total + value * value);
        let rms = (energy / count as f64).sqrt();

        let crossings = samples
            .windows(2)
            .filter(|pair| match (pair.first(), pair.last()) {
                (Some(previous), Some(current)) => (*current >= 0.0) != (*previous >= 0.0),
                _ => false,
            })
            .count();
        let zcr = if count > 1 {
            crossings as f64 / (count - 1) as f64
        } else {
            0.0
        };

        let peak = samples
            .iter()
            .fold(f64::NEG_INFINITY, |best, value| {
                let magnitude = value.abs();
                if magnitude > best {
                    magnitude
                } else {
                    best
                }
            });
        let crest = if rms > 1e-9 { peak / rms } else { 0.0 };

        let mut envelope = Vec::with_capacity(4);
        for segment in 0..4usize {
            let start = segment * count / 4;
            let end = (segment + 1) * count / 4;
            let values = samples.get(start..end).unwrap_or_default();
            if values.is_empty() {
                return Err(AssocError::EmptyEnvelopeSegment(count));
            }
            let segment_energy = values
                .iter()
                .fold(0.0f64, |total, value| total + value * value);
            let segment_rms = (segment_energy / values.len() as f64).sqrt();
            envelope.push(if rms > 1e-9 { segment_rms / rms } else { 0.0 });
        }

        Ok(Feature {
            rms: Some(rms),
            zcr: Some(zcr),
            crest: Some(crest),
            envelope: Some(envelope),
        })
    }
}

/// Interleaved PCM to normalised f64 samples, one entry per channel sample.
pub fn decode_pcm(raw: &[u8], sample_width: u16, byte_order: ByteOrder) -> Result<Vec<f64>> {
    match sample_width {
        1 => Ok(raw
            .iter()
            .map(|value| (f64::from(*value) - 128.0) / 128.0)
            .collect()),
        2 => Ok(raw
            .chunks_exact(2)
            .map(|bytes| {
                let mut pair = [0u8; 2];
                pair.copy_from_slice(bytes);
                let value = match byte_order {
                    ByteOrder::Little => i16::from_le_bytes(pair),
                    ByteOrder::Big => i16::from_be_bytes(pair),
                };
                f64::from(value) / 32768.0
            })
            .collect()),
        3 => Ok(raw
            .chunks_exact(3)
            .map(|bytes| {
                let first = bytes.first().copied().unwrap_or(0);
                let second = bytes.get(1).copied().unwrap_or(0);
                let third = bytes.get(2).copied().unwrap_or(0);
                let value = match byte_order {
                    ByteOrder::Little => i32::from_le_bytes([0, first, second, third]) >> 8,
                    ByteOrder::Big => i32::from_be_bytes([first, second, third, 0]) >> 8,
                };
                f64::from(value) / 8_388_608.0
            })
            .collect()),
        4 => Ok(raw
            .chunks_exact(4)
            .map(|bytes| {
                let mut quad = [0u8; 4];
                quad.copy_from_slice(bytes);
                let value = match byte_order {
                    ByteOrder::Little => i32::from_le_bytes(quad),
                    ByteOrder::Big => i32::from_be_bytes(quad),
                };
                f64::from(value) / 2_147_483_648.0
            })
            .collect()),
        other => Err(AssocError::UnsupportedSampleWidth(other)),
    }
}

/// The `export_feature_extraction` block of the association record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionDetails {
    pub sample_rate_hz: u32,
    pub channel_count: u16,
    pub sample_width_bytes: u16,
    pub window_size_frames: u64,
    pub truncated_at_window_limit: bool,
}

impl ExtractionDetails {
    pub fn to_value(&self) -> Value {
        let duration = python_round(
            self.window_size_frames as f64 / f64::from(self.sample_rate_hz),
            8,
        );
        let mut details = Map::new();
        details.insert("sample_rate_hz".into(), json!(self.sample_rate_hz));
        details.insert("channel_count".into(), json!(self.channel_count));
        details.insert("sample_width_bytes".into(), json!(self.sample_width_bytes));
        details.insert("window_size_frames".into(), json!(self.window_size_frames));
        details.insert("window_duration_seconds".into(), json!(duration));
        details.insert(
            "truncated_at_window_limit".into(),
            json!(self.truncated_at_window_limit),
        );
        details.insert("streaming_extraction".into(), json!(true));
        Value::Object(details)
    }
}
