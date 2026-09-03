use std::path::Path;

use serde_json::{json, Number, Value};

/// Container-level facts about an audio file.
///
/// REQUIRED: `sample_rate` is a `Number`, not an `f64`. Python's
/// `extract_audio_metadata` returns an `int` from the wave/aifc path and an
/// `int`-or-`float` from the `afinfo` path, and `44100` and `44100.0` are
/// different canonical bytes inside a signed manifest.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioMetadata {
    pub duration_seconds: Option<f64>,
    pub sample_rate: Option<Number>,
    pub channels: Option<u16>,
}

impl AudioMetadata {
    pub fn to_json(&self) -> Value {
        json!({
            "duration_seconds": self.duration_seconds,
            "sample_rate": self.sample_rate,
            "channels": self.channels,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AudioFingerprint {
    pub rms: Option<f64>,
    pub zero_crossing_rate: Option<f64>,
}

impl AudioFingerprint {
    pub fn to_json(&self) -> Value {
        json!({
            "rms": self.rms,
            "zero_crossing_rate": self.zero_crossing_rate,
        })
    }
}

/// The untrusted-audio parsing seam.
///
/// IMPORTANT: implementations parse attacker-supplied containers. An
/// unparseable, truncated or hostile file MUST yield empty fields, never a panic
/// and never a partially-believed value: `extract_audio_metadata` in the Python
/// daemon degrades to all-`None` on every error path, and the manifest records
/// those as JSON `null`.
pub trait AudioProbe: Send + Sync {
    fn metadata(&self, path: &Path) -> AudioMetadata;
    fn fingerprint(&self, path: &Path) -> AudioFingerprint;
}

/// The honest default: no container parser is wired in, so nothing is claimed.
/// It is exactly what the Python daemon emits when `afinfo` is absent and the
/// `wave`/`aifc` readers reject the file.
pub struct UnavailableAudioProbe;

impl AudioProbe for UnavailableAudioProbe {
    fn metadata(&self, _path: &Path) -> AudioMetadata {
        AudioMetadata::default()
    }

    fn fingerprint(&self, _path: &Path) -> AudioFingerprint {
        AudioFingerprint::default()
    }
}
