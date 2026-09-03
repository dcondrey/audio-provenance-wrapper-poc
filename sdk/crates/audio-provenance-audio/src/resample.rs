use rubato::audioadapter_buffers::direct::SequentialSlice;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use crate::buffer::{AudioBuffer, MAX_SAMPLE_RATE};
use crate::error::AudioError;

const CHUNK_FRAMES: usize = 1024;

/// Band-limited sample rate conversion.
///
/// IMPORTANT: rubato trims the integer part of its startup delay, but roughly a
/// sample of group delay survives (measured at 1.13 samples for 48k -> 44.1k ->
/// 48k, and constant with frequency). Callers that compare the result against
/// the input sample by sample must align first; a 1 kHz tone reads 16 dB SNR
/// unaligned and 105 dB aligned.
pub fn resample(buffer: &AudioBuffer, target_rate: u32) -> Result<AudioBuffer, AudioError> {
    if target_rate == 0 || target_rate > MAX_SAMPLE_RATE {
        return Err(AudioError::InvalidSampleRate {
            found: u64::from(target_rate),
        });
    }
    if target_rate == buffer.sample_rate() {
        return Ok(buffer.clone());
    }
    if buffer.is_empty() {
        return AudioBuffer::silence(target_rate, buffer.channels(), 0);
    }
    let ratio = f64::from(target_rate) / f64::from(buffer.sample_rate());
    run(buffer, ratio, target_rate)
}

/// Band-limited resampling by an arbitrary ratio, keeping the declared sample
/// rate. That is a playback-speed change: the content is stretched or
/// compressed and the header still says what it said.
///
/// `ratio` is output length over input length, so 0.5 halves the sample count.
pub fn resample_ratio(buffer: &AudioBuffer, ratio: f64) -> Result<AudioBuffer, AudioError> {
    if !ratio.is_finite() || ratio <= 0.0 {
        return Err(AudioError::InvalidResampleRatio(
            "ratio must be finite and positive",
        ));
    }
    if (ratio - 1.0).abs() < f64::EPSILON {
        return Ok(buffer.clone());
    }
    if buffer.is_empty() {
        return Ok(buffer.clone());
    }
    run(buffer, ratio, buffer.sample_rate())
}

fn run(buffer: &AudioBuffer, ratio: f64, out_rate: u32) -> Result<AudioBuffer, AudioError> {
    let parameters = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: None,
        oversampling_factor: 128,
        interpolation: SincInterpolationType::Cubic,
        window: WindowFunction::BlackmanHarris2,
    };
    let mut resampler = Async::<f32>::new_sinc(
        ratio,
        1.0,
        &parameters,
        CHUNK_FRAMES,
        buffer.channels(),
        FixedAsync::Input,
    )?;

    let frames = buffer.frames();
    let capacity = resampler.process_all_needed_output_len(frames);
    let mut output = vec![0.0f32; capacity * buffer.channels()];

    let input = SequentialSlice::new(buffer.planes(), buffer.channels(), frames).map_err(|_| {
        AudioError::LengthMismatch {
            expected: buffer.channels() * frames,
            found: buffer.planes().len(),
        }
    })?;
    let produced = {
        let mut sink =
            SequentialSlice::new_mut(&mut output, buffer.channels(), capacity).map_err(|_| {
                AudioError::LengthMismatch {
                    expected: buffer.channels() * capacity,
                    found: capacity,
                }
            })?;
        let (_, produced) = resampler.process_all_into_buffer(&input, &mut sink, frames, None)?;
        produced
    };

    let mut planes = vec![0.0f32; produced * buffer.channels()];
    for channel in 0..buffer.channels() {
        let source = &output[channel * capacity..channel * capacity + produced];
        planes[channel * produced..(channel + 1) * produced].copy_from_slice(source);
    }
    AudioBuffer::from_planes(out_rate, buffer.channels(), planes)
}

/// Catmull-Rom interpolation at a fractional sample position, with the four-tap
/// neighbourhood clamped to the ends of the slice.
pub fn fractional_sample(samples: &[f32], position: f64) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let last = samples.len() - 1;
    let base = position.floor();
    let fraction = (position - base) as f32;
    let index = base as i64;

    let tap = |offset: i64| -> f32 {
        let at = index.saturating_add(offset).clamp(0, last as i64);
        samples[at as usize]
    };
    let (p0, p1, p2, p3) = (tap(-1), tap(0), tap(1), tap(2));

    let a0 = p1;
    let a1 = 0.5 * (p2 - p0);
    let a2 = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let a3 = 0.5 * (p3 - p0) + 1.5 * (p1 - p2);
    ((a3 * fraction + a2) * fraction + a1) * fraction + a0
}

/// Simulates a capture clock running fast or slow by `ppm` parts per million.
///
/// The sample rate in the header does not change; the content is stretched, the
/// way a drifting converter would render it.
#[derive(Clone, Copy, Debug)]
pub struct ClockDrift {
    ppm: f64,
}

impl ClockDrift {
    pub const MAX_PPM: f64 = 100_000.0;

    pub fn new(ppm: f64) -> Result<Self, AudioError> {
        if !ppm.is_finite() || ppm.abs() > Self::MAX_PPM {
            return Err(AudioError::InvalidResampleRatio(
                "drift must be finite and within +/-100000 ppm",
            ));
        }
        Ok(Self { ppm })
    }

    pub const fn ppm(&self) -> f64 {
        self.ppm
    }

    fn rate(&self) -> f64 {
        1.0 + self.ppm * 1.0e-6
    }

    pub fn apply(&self, buffer: &AudioBuffer) -> Result<AudioBuffer, AudioError> {
        let rate = self.rate();
        let frames = (buffer.frames() as f64 * rate).round() as usize;
        let mut planes = vec![0.0f32; frames * buffer.channels()];
        for channel in 0..buffer.channels() {
            let source = buffer
                .channel(channel)
                .ok_or(AudioError::InvalidChannelCount {
                    found: channel as u64,
                })?;
            for frame in 0..frames {
                planes[channel * frames + frame] = fractional_sample(source, frame as f64 / rate);
            }
        }
        AudioBuffer::from_planes(buffer.sample_rate(), buffer.channels(), planes)
    }
}
