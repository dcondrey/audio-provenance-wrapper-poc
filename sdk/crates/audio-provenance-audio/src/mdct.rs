use std::f64::consts::PI;
use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::error::AudioError;
use crate::fft::MAX_TRANSFORM_LEN;
use crate::window::princen_bradley;

/// Modified discrete cosine transform over `2 * half_len` samples, computed
/// with a complex FFT of the same length plus pre- and post-twiddles.
///
/// IMPORTANT: the twiddles and the butterflies run in f64. At `half_len` 1024 an
/// f32 twiddle table leaves roughly 1e-5 of reconstruction error, which is above
/// the level a watermark residual has to survive.
pub struct Mdct {
    half_len: usize,
    fft_forward: Arc<dyn Fft<f64>>,
    fft_inverse: Arc<dyn Fft<f64>>,
    pre_forward: Vec<Complex<f64>>,
    post_forward: Vec<Complex<f64>>,
    pre_inverse: Vec<Complex<f64>>,
    post_inverse: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
}

impl std::fmt::Debug for Mdct {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mdct")
            .field("half_len", &self.half_len)
            .finish()
    }
}

impl Mdct {
    pub fn new(half_len: usize) -> Result<Self, AudioError> {
        if half_len < 2 {
            return Err(AudioError::InvalidTransformLength {
                found: half_len,
                reason: "mdct half length must be at least 2",
            });
        }
        if !half_len.is_multiple_of(2) {
            return Err(AudioError::InvalidTransformLength {
                found: half_len,
                reason: "mdct half length must be even",
            });
        }
        if half_len > MAX_TRANSFORM_LEN / 2 {
            return Err(AudioError::InvalidTransformLength {
                found: half_len,
                reason: "exceeds the maximum transform length",
            });
        }

        let block = half_len * 2;
        let m = half_len as f64;
        let offset = 0.5 + m / 2.0;

        let mut planner = FftPlanner::<f64>::new();
        Ok(Self {
            half_len,
            fft_forward: planner.plan_fft_forward(block),
            fft_inverse: planner.plan_fft_inverse(block),
            pre_forward: (0..block)
                .map(|n| Complex::from_polar(1.0, -PI * n as f64 / (2.0 * m)))
                .collect(),
            post_forward: (0..half_len)
                .map(|k| Complex::from_polar(1.0, -PI / m * offset * (k as f64 + 0.5)))
                .collect(),
            pre_inverse: (0..half_len)
                .map(|k| Complex::from_polar(1.0, PI / m * offset * k as f64))
                .collect(),
            post_inverse: (0..block)
                .map(|n| Complex::from_polar(1.0, PI * (n as f64 + offset) / (2.0 * m)))
                .collect(),
            scratch: vec![Complex::new(0.0, 0.0); block],
        })
    }

    pub const fn half_len(&self) -> usize {
        self.half_len
    }

    pub const fn block_len(&self) -> usize {
        self.half_len * 2
    }

    pub fn forward(&mut self, block: &[f32], coefficients: &mut [f32]) -> Result<(), AudioError> {
        check_len(block.len(), self.block_len())?;
        check_len(coefficients.len(), self.half_len)?;
        for (slot, (sample, twiddle)) in self
            .scratch
            .iter_mut()
            .zip(block.iter().zip(self.pre_forward.iter()))
        {
            *slot = twiddle * f64::from(*sample);
        }
        self.fft_forward.process(&mut self.scratch);
        for (out, (value, twiddle)) in coefficients
            .iter_mut()
            .zip(self.scratch.iter().zip(self.post_forward.iter()))
        {
            *out = (value * twiddle).re as f32;
        }
        Ok(())
    }

    pub fn inverse(&mut self, coefficients: &[f32], block: &mut [f32]) -> Result<(), AudioError> {
        check_len(coefficients.len(), self.half_len)?;
        check_len(block.len(), self.block_len())?;
        for slot in self.scratch.iter_mut() {
            *slot = Complex::new(0.0, 0.0);
        }
        for (slot, (value, twiddle)) in self
            .scratch
            .iter_mut()
            .zip(coefficients.iter().zip(self.pre_inverse.iter()))
        {
            *slot = twiddle * f64::from(*value);
        }
        self.fft_inverse.process(&mut self.scratch);
        let scale = 2.0 / self.half_len as f64;
        for (out, (value, twiddle)) in block
            .iter_mut()
            .zip(self.scratch.iter().zip(self.post_inverse.iter()))
        {
            *out = (scale * (value * twiddle).re) as f32;
        }
        Ok(())
    }
}

/// Whole-signal MDCT with time-domain alias cancellation.
///
/// Blocks of `2 * half_len` samples hop by `half_len`, windowed on both
/// analysis and synthesis with a Princen-Bradley window, so overlap-adding
/// consecutive synthesis blocks cancels the aliasing each one carries.
#[derive(Debug)]
pub struct MdctTransform {
    mdct: Mdct,
    window: Vec<f32>,
}

impl MdctTransform {
    pub fn new(half_len: usize) -> Result<Self, AudioError> {
        let mdct = Mdct::new(half_len)?;
        let window = princen_bradley(half_len);
        Ok(Self { mdct, window })
    }

    pub fn with_window(half_len: usize, window: Vec<f32>) -> Result<Self, AudioError> {
        let mdct = Mdct::new(half_len)?;
        check_len(window.len(), mdct.block_len())?;
        Ok(Self { mdct, window })
    }

    pub const fn half_len(&self) -> usize {
        self.mdct.half_len
    }

    pub fn window(&self) -> &[f32] {
        &self.window
    }

    pub fn block_count(&self, signal_len: usize) -> usize {
        signal_len.div_ceil(self.half_len()) + 1
    }

    /// Coefficients for every block, laid out block-major.
    pub fn analyze(&mut self, signal: &[f32]) -> Result<Vec<f32>, AudioError> {
        let half = self.half_len();
        let blocks = self.block_count(signal.len());
        let mut padded = vec![0.0f32; (blocks + 1) * half];
        padded[half..half + signal.len()].copy_from_slice(signal);

        let mut coefficients = vec![0.0f32; blocks * half];
        let mut block = vec![0.0f32; half * 2];
        for index in 0..blocks {
            let base = index * half;
            for (offset, slot) in block.iter_mut().enumerate() {
                *slot = padded[base + offset] * self.window[offset];
            }
            let out = &mut coefficients[index * half..(index + 1) * half];
            self.mdct.forward(&block, out)?;
        }
        Ok(coefficients)
    }

    pub fn synthesize(
        &mut self,
        coefficients: &[f32],
        signal_len: usize,
    ) -> Result<Vec<f32>, AudioError> {
        let half = self.half_len();
        let blocks = self.block_count(signal_len);
        check_len(coefficients.len(), blocks * half)?;

        let mut padded = vec![0.0f32; (blocks + 1) * half];
        let mut block = vec![0.0f32; half * 2];
        for index in 0..blocks {
            self.mdct
                .inverse(&coefficients[index * half..(index + 1) * half], &mut block)?;
            let base = index * half;
            for (offset, value) in block.iter().enumerate() {
                padded[base + offset] += value * self.window[offset];
            }
        }
        Ok(padded[half..half + signal_len].to_vec())
    }
}

fn check_len(found: usize, expected: usize) -> Result<(), AudioError> {
    if found == expected {
        Ok(())
    } else {
        Err(AudioError::LengthMismatch { expected, found })
    }
}
