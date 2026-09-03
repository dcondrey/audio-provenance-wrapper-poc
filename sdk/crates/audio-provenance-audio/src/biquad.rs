use std::f64::consts::PI;

use crate::buffer::AudioBuffer;
use crate::error::AudioError;

/// Second-order section, normalised so `a0 == 1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BiquadCoefficients {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl BiquadCoefficients {
    pub fn low_pass(sample_rate: u32, frequency: f64, q: f64) -> Result<Self, AudioError> {
        let (_, cos_w, alpha) = intermediates(sample_rate, frequency, q)?;
        let b1 = 1.0 - cos_w;
        Ok(normalize(
            b1 / 2.0,
            b1,
            b1 / 2.0,
            1.0 + alpha,
            -2.0 * cos_w,
            1.0 - alpha,
        ))
    }

    pub fn high_pass(sample_rate: u32, frequency: f64, q: f64) -> Result<Self, AudioError> {
        let (_, cos_w, alpha) = intermediates(sample_rate, frequency, q)?;
        let b1 = -(1.0 + cos_w);
        Ok(normalize(
            (1.0 + cos_w) / 2.0,
            b1,
            (1.0 + cos_w) / 2.0,
            1.0 + alpha,
            -2.0 * cos_w,
            1.0 - alpha,
        ))
    }

    pub fn peaking(
        sample_rate: u32,
        frequency: f64,
        q: f64,
        gain_db: f64,
    ) -> Result<Self, AudioError> {
        if !gain_db.is_finite() || gain_db.abs() > 96.0 {
            return Err(AudioError::InvalidFilterParameter(
                "peaking gain must be finite and within +/-96 dB",
            ));
        }
        let (_, cos_w, alpha) = intermediates(sample_rate, frequency, q)?;
        let amplitude = 10.0f64.powf(gain_db / 40.0);
        Ok(normalize(
            1.0 + alpha * amplitude,
            -2.0 * cos_w,
            1.0 - alpha * amplitude,
            1.0 + alpha / amplitude,
            -2.0 * cos_w,
            1.0 - alpha / amplitude,
        ))
    }
}

fn intermediates(sample_rate: u32, frequency: f64, q: f64) -> Result<(f64, f64, f64), AudioError> {
    if sample_rate == 0 {
        return Err(AudioError::InvalidSampleRate { found: 0 });
    }
    if !frequency.is_finite() || frequency <= 0.0 || frequency >= f64::from(sample_rate) / 2.0 {
        return Err(AudioError::InvalidFilterParameter(
            "centre frequency must lie strictly between 0 and Nyquist",
        ));
    }
    if !q.is_finite() || q <= 0.0 {
        return Err(AudioError::InvalidFilterParameter(
            "q must be finite and positive",
        ));
    }
    let omega = 2.0 * PI * frequency / f64::from(sample_rate);
    let (sin_w, cos_w) = omega.sin_cos();
    Ok((sin_w, cos_w, sin_w / (2.0 * q)))
}

fn normalize(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> BiquadCoefficients {
    BiquadCoefficients {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// Transposed direct form II with f64 state.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    coefficients: BiquadCoefficients,
    z1: f64,
    z2: f64,
}

impl Biquad {
    pub const fn new(coefficients: BiquadCoefficients) -> Self {
        Self {
            coefficients,
            z1: 0.0,
            z2: 0.0,
        }
    }

    pub const fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    pub fn process_sample(&mut self, input: f32) -> f32 {
        let c = &self.coefficients;
        let x = f64::from(input);
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y as f32
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        for sample in samples.iter_mut() {
            *sample = self.process_sample(*sample);
        }
    }
}

/// Filters every channel independently, each with its own fresh state.
pub fn apply(buffer: &mut AudioBuffer, coefficients: BiquadCoefficients) {
    let channels = buffer.channels();
    let frames = buffer.frames();
    let planes = buffer.planes_mut();
    for channel in 0..channels {
        let mut filter = Biquad::new(coefficients);
        filter.process(&mut planes[channel * frames..(channel + 1) * frames]);
    }
}

/// Runs `stages` in order over `samples`, each stage starting from rest.
///
/// Filtering a whole slice per stage rather than a whole cascade per sample is
/// what a mastering chain does, and it keeps each stage's state independent.
pub fn cascade(stages: &[BiquadCoefficients], samples: &mut [f32]) {
    for coefficients in stages {
        Biquad::new(*coefficients).process(samples);
    }
}
