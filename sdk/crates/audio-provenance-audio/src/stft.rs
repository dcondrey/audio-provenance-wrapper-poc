use rustfft::num_complex::Complex;

use crate::error::AudioError;
use crate::fft::RealFft;

/// Frames of a short-time Fourier transform, stored bin-major within a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct StftFrames {
    frames: usize,
    bins: usize,
    data: Vec<Complex<f32>>,
}

impl StftFrames {
    pub const fn frames(&self) -> usize {
        self.frames
    }

    pub const fn bins(&self) -> usize {
        self.bins
    }

    pub fn frame(&self, index: usize) -> Option<&[Complex<f32>]> {
        let start = index.checked_mul(self.bins)?;
        self.data.get(start..start.checked_add(self.bins)?)
    }

    pub fn frame_mut(&mut self, index: usize) -> Option<&mut [Complex<f32>]> {
        let start = index.checked_mul(self.bins)?;
        let end = start.checked_add(self.bins)?;
        self.data.get_mut(start..end)
    }

    pub fn data(&self) -> &[Complex<f32>] {
        &self.data
    }
}

/// Weighted overlap-add STFT.
///
/// The window is applied on analysis and again on synthesis, and the sum of
/// squared windows divides the result out. That reconstructs exactly for any
/// window whose squared overlap-add sum is non-zero, which is a strictly wider
/// condition than COLA; use [`Stft::cola_sum`] to check that the window and hop
/// you picked actually give the flat denominator you expect.
#[derive(Debug)]
pub struct Stft {
    fft: RealFft<f32>,
    window: Vec<f32>,
    hop: usize,
}

impl Stft {
    pub fn new(window: Vec<f32>, hop: usize) -> Result<Self, AudioError> {
        let fft = RealFft::new(window.len())?;
        if hop == 0 || hop > window.len() {
            return Err(AudioError::InvalidTransformLength {
                found: hop,
                reason: "hop must be in 1..=window length",
            });
        }
        Ok(Self { fft, window, hop })
    }

    pub const fn window_len(&self) -> usize {
        self.window.len()
    }

    pub const fn hop(&self) -> usize {
        self.hop
    }

    pub fn bins(&self) -> usize {
        self.fft.bins()
    }

    /// Zero padding placed before and after the signal so that every input
    /// sample falls inside the region where all overlapping windows are present.
    pub const fn pad(&self) -> usize {
        self.window.len()
    }

    pub fn frame_count(&self, signal_len: usize) -> usize {
        (self.pad() + signal_len).div_ceil(self.hop)
    }

    fn padded_len(&self, signal_len: usize) -> usize {
        (self.frame_count(signal_len) - 1) * self.hop + self.window.len()
    }

    /// Overlap-added squared window over a padded signal of `signal_len`
    /// samples. Flat across `pad()..pad() + signal_len` exactly when the chosen
    /// window and hop satisfy the squared-COLA condition.
    pub fn cola_sum(&self, signal_len: usize) -> Vec<f32> {
        let mut sum = vec![0.0f32; self.padded_len(signal_len)];
        for frame in 0..self.frame_count(signal_len) {
            let base = frame * self.hop;
            for (offset, value) in self.window.iter().enumerate() {
                sum[base + offset] += value * value;
            }
        }
        sum
    }

    pub fn forward(&self, signal: &[f32]) -> Result<StftFrames, AudioError> {
        let frames = self.frame_count(signal.len());
        let mut padded = vec![0.0f32; self.padded_len(signal.len())];
        padded[self.pad()..self.pad() + signal.len()].copy_from_slice(signal);

        let bins = self.fft.bins();
        let mut data = vec![Complex::new(0.0f32, 0.0); frames * bins];
        let mut block = self.fft.scratch_input();
        for frame in 0..frames {
            let base = frame * self.hop;
            for (offset, slot) in block.iter_mut().enumerate() {
                *slot = padded[base + offset] * self.window[offset];
            }
            let spectrum = &mut data[frame * bins..(frame + 1) * bins];
            self.fft.forward(&mut block, spectrum)?;
        }
        Ok(StftFrames { frames, bins, data })
    }

    pub fn inverse(&self, frames: &StftFrames, signal_len: usize) -> Result<Vec<f32>, AudioError> {
        if frames.bins != self.fft.bins() {
            return Err(AudioError::LengthMismatch {
                expected: self.fft.bins(),
                found: frames.bins,
            });
        }
        let expected = self.frame_count(signal_len);
        if frames.frames != expected {
            return Err(AudioError::LengthMismatch {
                expected,
                found: frames.frames,
            });
        }

        let padded_len = self.padded_len(signal_len);
        let mut accumulator = vec![0.0f32; padded_len];
        let mut denominator = vec![0.0f32; padded_len];
        let mut block = self.fft.scratch_input();
        let mut spectrum = self.fft.scratch_spectrum();
        for frame in 0..frames.frames {
            spectrum.copy_from_slice(&frames.data[frame * frames.bins..(frame + 1) * frames.bins]);
            self.fft.inverse(&mut spectrum, &mut block)?;
            let base = frame * self.hop;
            for (offset, value) in self.window.iter().enumerate() {
                accumulator[base + offset] += block[offset] * value;
                denominator[base + offset] += value * value;
            }
        }

        let mut out = vec![0.0f32; signal_len];
        for (index, slot) in out.iter_mut().enumerate() {
            let at = self.pad() + index;
            let weight = denominator[at];
            if weight > f32::EPSILON {
                *slot = accumulator[at] / weight;
            }
        }
        Ok(out)
    }
}
