use std::path::Path;

use apw_assoc::python_round;
use apw_daemon::{AudioFingerprint, AudioMetadata, AudioProbe};
use apw_provenance::{PcmFormat, PcmReader};
use serde_json::Number;

/// `compute_audio_fingerprint` in `daemon/sample_watcher/watcher.py` reads at
/// most one second at 44.1 kHz, and the bound is part of the value: a longer
/// read would produce a different RMS for the same file.
const FINGERPRINT_MAX_FRAMES: usize = 44_100;

/// The container-parsing seam, backed by the bounded RIFF/AIFF chunk walk.
///
/// IMPORTANT: every method takes an attacker-supplied path. A truncated or
/// hostile container yields all-`None`, matching the Python probe's degradation,
/// so a manifest never records a half-believed value.
pub struct PcmAudioProbe;

impl AudioProbe for PcmAudioProbe {
    fn metadata(&self, path: &Path) -> AudioMetadata {
        let Ok(reader) = PcmReader::open(path) else {
            return AudioMetadata::default();
        };
        let format = reader.format();
        if format.sample_rate == 0 {
            return AudioMetadata::default();
        }
        AudioMetadata {
            duration_seconds: Some(reader.remaining_frames() as f64 / f64::from(format.sample_rate)),
            sample_rate: Some(Number::from(format.sample_rate)),
            channels: Some(format.channels),
        }
    }

    fn fingerprint(&self, path: &Path) -> AudioFingerprint {
        fingerprint(path).unwrap_or_default()
    }
}

fn fingerprint(path: &Path) -> Option<AudioFingerprint> {
    let extension = path.extension().and_then(|value| value.to_str())?;
    if !extension.eq_ignore_ascii_case("wav") {
        return None;
    }
    let mut reader = PcmReader::open(path).ok()?;
    let PcmFormat { sample_width, .. } = reader.format();
    if sample_width != 2 {
        return None;
    }
    let mono = reader.read_mono_frames(FINGERPRINT_MAX_FRAMES).ok()?;
    if mono.is_empty() {
        return None;
    }

    let energy: f64 = mono.iter().map(|sample| sample * sample).sum();
    let rms = (energy / mono.len() as f64).sqrt();
    let crossings = mono
        .windows(2)
        .filter(|pair| match pair {
            [previous, current] => (*current >= 0.0) != (*previous >= 0.0),
            _ => false,
        })
        .count();
    let zcr = if mono.len() > 1 {
        crossings as f64 / (mono.len() - 1) as f64
    } else {
        0.0
    };

    Some(AudioFingerprint {
        rms: Some(python_round(rms, 6)),
        zero_crossing_rate: Some(python_round(zcr, 6)),
    })
}
