use audio_provenance_audio::buffer::AudioBuffer;
use audio_provenance_audio::decode::DecodeLimits;
use audio_provenance_audio::wav::{self, BitDepth};

use crate::error::AudioError;

pub const MIN_SAMPLE_RATE: u32 = 8_000;
pub const MAX_SAMPLE_RATE: u32 = 192_000;
pub const MAX_CHANNELS: usize = 8;
pub const MAX_SAMPLES: usize = 64 << 20;
pub const MAX_WAV_BYTES: usize = 1 << 30;

/// Admission policy for anything the bench will measure.
///
/// `audio_provenance_audio::AudioBuffer` is the substrate type and is deliberately permissive: it accepts
/// 768 kHz, 64 channels, zero frames and non-finite samples, all of which are legitimate somewhere
/// in the stack. None of them is measurable here. A NaN reaching a recovery rate produces a number
/// that looks like a measurement and is not one, so the check is on the way in rather than a note
/// in the report.
pub fn admit(buffer: AudioBuffer) -> Result<AudioBuffer, AudioError> {
    let sample_rate = buffer.sample_rate();
    if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
        return Err(AudioError::SampleRate {
            found: sample_rate,
            min: MIN_SAMPLE_RATE,
            max: MAX_SAMPLE_RATE,
        });
    }
    let channels = buffer.channels();
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(AudioError::ChannelCount {
            found: channels,
            max: MAX_CHANNELS,
        });
    }
    if buffer.is_empty() {
        return Err(AudioError::Empty);
    }
    let frames = buffer.frames();
    if buffer.planes().len() > MAX_SAMPLES {
        return Err(AudioError::TooLarge {
            frames,
            channels,
            limit: MAX_SAMPLES,
        });
    }
    if let Some(index) = buffer.planes().iter().position(|s| !s.is_finite()) {
        return Err(AudioError::NonFinite {
            channel: index / frames,
            frame: index % frames,
        });
    }
    Ok(buffer)
}

pub fn from_planes(
    sample_rate: u32,
    channels: usize,
    planes: Vec<f32>,
) -> Result<AudioBuffer, AudioError> {
    admit(AudioBuffer::from_planes(sample_rate, channels, planes)?)
}

pub fn from_channels(sample_rate: u32, channels: &[Vec<f32>]) -> Result<AudioBuffer, AudioError> {
    admit(AudioBuffer::from_channels(sample_rate, channels)?)
}

pub fn from_interleaved(
    sample_rate: u32,
    channels: usize,
    interleaved: &[f32],
) -> Result<AudioBuffer, AudioError> {
    admit(AudioBuffer::from_interleaved(
        sample_rate,
        channels,
        interleaved,
    )?)
}

/// Rebuilds the buffer with `f` applied to every sample in INTERLEAVED order.
///
/// IMPORTANT: the order is part of the contract. The noise channels draw one value per visited
/// sample from a seeded generator, so a planar walk would give the same distribution and a
/// different realisation, and the `(track, channel, seed)` triple would stop replaying a row.
pub fn map_samples<F>(buffer: &AudioBuffer, mut f: F) -> Result<AudioBuffer, AudioError>
where
    F: FnMut(f32) -> f32,
{
    let mapped: Vec<f32> = buffer
        .to_interleaved()
        .into_iter()
        .map(&mut f)
        .collect::<Vec<f32>>();
    from_interleaved(buffer.sample_rate(), buffer.channels(), &mapped)
}

pub fn peak(buffer: &AudioBuffer) -> f32 {
    buffer
        .planes()
        .iter()
        .fold(0.0f32, |acc, sample| acc.max(sample.abs()))
}

pub fn mean_square(buffer: &AudioBuffer) -> f64 {
    let planes = buffer.planes();
    if planes.is_empty() {
        return 0.0;
    }
    planes
        .iter()
        .map(|&s| f64::from(s) * f64::from(s))
        .sum::<f64>()
        / planes.len() as f64
}

/// Interleaved f32 little-endian, which is what `ffmpeg -f f32le` reads and writes and what the
/// corpus PCM digest is taken over.
pub fn to_f32le_bytes(buffer: &AudioBuffer) -> Vec<u8> {
    let interleaved = buffer.to_interleaved();
    let mut out = Vec::with_capacity(interleaved.len() * 4);
    for sample in interleaved {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

pub fn from_f32le_bytes(
    bytes: &[u8],
    sample_rate: u32,
    channels: usize,
) -> Result<AudioBuffer, AudioError> {
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(AudioError::ChannelCount {
            found: channels,
            max: MAX_CHANNELS,
        });
    }
    if !bytes.len().is_multiple_of(4) {
        return Err(AudioError::Ragged {
            len: bytes.len(),
            channels: channels * 4,
        });
    }
    let samples = bytes.len() / 4;
    if samples > MAX_SAMPLES {
        return Err(AudioError::TooLarge {
            frames: samples / channels,
            channels,
            limit: MAX_SAMPLES,
        });
    }
    let interleaved: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    from_interleaved(sample_rate, channels, &interleaved)
}

pub fn decode_wav(bytes: &[u8]) -> Result<AudioBuffer, AudioError> {
    admit(wav::decode(
        bytes,
        &DecodeLimits::new(MAX_WAV_BYTES, MAX_SAMPLES),
    )?)
}

pub fn encode_wav_f32(buffer: &AudioBuffer) -> Result<Vec<u8>, AudioError> {
    Ok(wav::encode(buffer, BitDepth::Float32)?)
}
