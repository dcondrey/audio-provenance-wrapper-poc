use audio_provenance_audio::AudioBuffer;

use crate::error::GenoMarkError;

/// Longest input the whole-file detector spectrogram is allowed to size an allocation from.
pub const MAX_ANALYSIS_SECONDS: f64 = 600.0;

/// The boundary every public entry point crosses. `AudioBuffer` is a permissive substrate type and
/// will hold a NaN or a zero-frame buffer; nothing past this point may.
pub fn admit(audio: &AudioBuffer) -> Result<AudioBuffer, GenoMarkError> {
    if audio.is_empty() {
        return Err(GenoMarkError::Empty);
    }
    let limit = (MAX_ANALYSIS_SECONDS * f64::from(audio.sample_rate())) as usize;
    if audio.frames() > limit {
        return Err(GenoMarkError::TooLarge {
            frames: audio.frames(),
            channels: audio.channels(),
            limit,
        });
    }
    for channel in 0..audio.channels() {
        let plane = audio.channel(channel).ok_or(GenoMarkError::Empty)?;
        if let Some(frame) = plane.iter().position(|sample| !sample.is_finite()) {
            return Err(GenoMarkError::NonFinite { channel, frame });
        }
    }
    Ok(audio.clone())
}
