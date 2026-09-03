use crate::error::AudioError;

pub const MAX_CHANNELS: usize = 64;
pub const MAX_SAMPLE_RATE: u32 = 768_000;

/// Planar f32 PCM: `planes[channel * frames + frame]`.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioBuffer {
    sample_rate: u32,
    channels: usize,
    frames: usize,
    planes: Vec<f32>,
}

impl AudioBuffer {
    pub fn silence(sample_rate: u32, channels: usize, frames: usize) -> Result<Self, AudioError> {
        let total = validate_shape(sample_rate, channels, frames)?;
        Ok(Self {
            sample_rate,
            channels,
            frames,
            planes: vec![0.0; total],
        })
    }

    pub fn from_planes(
        sample_rate: u32,
        channels: usize,
        planes: Vec<f32>,
    ) -> Result<Self, AudioError> {
        if channels == 0 || channels > MAX_CHANNELS {
            return Err(AudioError::InvalidChannelCount {
                found: channels as u64,
            });
        }
        if !planes.len().is_multiple_of(channels) {
            return Err(AudioError::LengthMismatch {
                expected: planes.len().next_multiple_of(channels),
                found: planes.len(),
            });
        }
        let frames = planes.len() / channels;
        validate_shape(sample_rate, channels, frames)?;
        Ok(Self {
            sample_rate,
            channels,
            frames,
            planes,
        })
    }

    pub fn from_channels(sample_rate: u32, channels: &[Vec<f32>]) -> Result<Self, AudioError> {
        if channels.is_empty() || channels.len() > MAX_CHANNELS {
            return Err(AudioError::InvalidChannelCount {
                found: channels.len() as u64,
            });
        }
        let frames = channels[0].len();
        if let Some(bad) = channels.iter().find(|plane| plane.len() != frames) {
            return Err(AudioError::LengthMismatch {
                expected: frames,
                found: bad.len(),
            });
        }
        let total = validate_shape(sample_rate, channels.len(), frames)?;
        let mut planes = Vec::with_capacity(total);
        for plane in channels {
            planes.extend_from_slice(plane);
        }
        Ok(Self {
            sample_rate,
            channels: channels.len(),
            frames,
            planes,
        })
    }

    pub fn from_interleaved(
        sample_rate: u32,
        channels: usize,
        interleaved: &[f32],
    ) -> Result<Self, AudioError> {
        if channels == 0 || channels > MAX_CHANNELS {
            return Err(AudioError::InvalidChannelCount {
                found: channels as u64,
            });
        }
        if !interleaved.len().is_multiple_of(channels) {
            return Err(AudioError::LengthMismatch {
                expected: interleaved.len().next_multiple_of(channels),
                found: interleaved.len(),
            });
        }
        let frames = interleaved.len() / channels;
        let total = validate_shape(sample_rate, channels, frames)?;
        let mut planes = vec![0.0f32; total];
        for (frame, chunk) in interleaved.chunks_exact(channels).enumerate() {
            for (channel, sample) in chunk.iter().enumerate() {
                planes[channel * frames + frame] = *sample;
            }
        }
        Ok(Self {
            sample_rate,
            channels,
            frames,
            planes,
        })
    }

    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub const fn channels(&self) -> usize {
        self.channels
    }

    pub const fn frames(&self) -> usize {
        self.frames
    }

    pub const fn is_empty(&self) -> bool {
        self.frames == 0
    }

    pub fn duration_seconds(&self) -> f64 {
        self.frames as f64 / f64::from(self.sample_rate)
    }

    pub fn channel(&self, channel: usize) -> Option<&[f32]> {
        let start = channel.checked_mul(self.frames)?;
        self.planes.get(start..start.checked_add(self.frames)?)
    }

    pub fn channel_mut(&mut self, channel: usize) -> Option<&mut [f32]> {
        let start = channel.checked_mul(self.frames)?;
        let end = start.checked_add(self.frames)?;
        self.planes.get_mut(start..end)
    }

    pub fn planes(&self) -> &[f32] {
        &self.planes
    }

    pub fn planes_mut(&mut self) -> &mut [f32] {
        &mut self.planes
    }

    pub fn to_interleaved(&self) -> Vec<f32> {
        let mut out = vec![0.0f32; self.planes.len()];
        for channel in 0..self.channels {
            let base = channel * self.frames;
            for frame in 0..self.frames {
                out[frame * self.channels + channel] = self.planes[base + frame];
            }
        }
        out
    }

    /// Channel average, reproducing `AudioObserver::pushAudioBlock`: f32
    /// accumulation in ascending channel order, then one multiply by a
    /// precomputed reciprocal.
    ///
    /// IMPORTANT: the window hash chain is taken over these exact bytes, so the
    /// accumulator width and the multiply-not-divide both have to stay as they
    /// are for a Audio Provenance hash to match a POC hash.
    pub fn mono_sum(&self) -> Vec<f32> {
        let gain = 1.0f32 / self.channels as f32;
        let mut out = vec![0.0f32; self.frames];
        for (frame, slot) in out.iter_mut().enumerate() {
            let mut sum = 0.0f32;
            for channel in 0..self.channels {
                sum += self.planes[channel * self.frames + frame];
            }
            *slot = sum * gain;
        }
        out
    }
}

fn validate_shape(sample_rate: u32, channels: usize, frames: usize) -> Result<usize, AudioError> {
    if sample_rate == 0 || sample_rate > MAX_SAMPLE_RATE {
        return Err(AudioError::InvalidSampleRate {
            found: u64::from(sample_rate),
        });
    }
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(AudioError::InvalidChannelCount {
            found: channels as u64,
        });
    }
    channels
        .checked_mul(frames)
        .ok_or(AudioError::SampleLimitExceeded {
            limit: usize::MAX,
            requested: u64::MAX,
        })
}
