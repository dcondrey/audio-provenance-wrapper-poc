use std::marker::PhantomData;
use std::sync::Arc;

use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use rustfft::FftNum;
use rustfft::num_complex::Complex;
use rustfft::num_traits::Float;

use crate::error::AudioError;

pub const MAX_TRANSFORM_LEN: usize = 1 << 22;

/// Real-input FFT sized once and reused.
///
/// `realfft` panics on a length or buffer-size mismatch, so every entry point
/// here checks the slice lengths before handing them over.
///
/// Generic over the sample scalar so an analysis that needs more headroom than
/// f32 offers can ask for `RealFft<f64>` instead of growing a second kernel.
pub struct RealFft<T: FftNum + Float> {
    len: usize,
    forward: Arc<dyn RealToComplex<T>>,
    inverse: Arc<dyn ComplexToReal<T>>,
    scalar: PhantomData<T>,
}

impl<T: FftNum + Float> std::fmt::Debug for RealFft<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealFft").field("len", &self.len).finish()
    }
}

impl<T: FftNum + Float> RealFft<T> {
    pub fn new(len: usize) -> Result<Self, AudioError> {
        if len < 2 {
            return Err(AudioError::InvalidTransformLength {
                found: len,
                reason: "must be at least 2",
            });
        }
        if !len.is_multiple_of(2) {
            return Err(AudioError::InvalidTransformLength {
                found: len,
                reason: "real fft length must be even",
            });
        }
        if len > MAX_TRANSFORM_LEN {
            return Err(AudioError::InvalidTransformLength {
                found: len,
                reason: "exceeds the maximum transform length",
            });
        }
        let mut planner = RealFftPlanner::<T>::new();
        Ok(Self {
            len,
            forward: planner.plan_fft_forward(len),
            inverse: planner.plan_fft_inverse(len),
            scalar: PhantomData,
        })
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    /// Never true: [`RealFft::new`] rejects a length below 2. Present because
    /// `len` alone is a clippy error on a public type.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn bins(&self) -> usize {
        self.len / 2 + 1
    }

    pub fn scratch_input(&self) -> Vec<T> {
        vec![T::zero(); self.len]
    }

    pub fn scratch_spectrum(&self) -> Vec<Complex<T>> {
        vec![Complex::new(T::zero(), T::zero()); self.bins()]
    }

    pub fn forward(&self, input: &mut [T], spectrum: &mut [Complex<T>]) -> Result<(), AudioError> {
        self.check(input.len(), self.len)?;
        self.check(spectrum.len(), self.bins())?;
        self.forward
            .process(input, spectrum)
            .map_err(|_| AudioError::Fft("forward transform rejected its buffers"))
    }

    pub fn inverse(&self, spectrum: &mut [Complex<T>], output: &mut [T]) -> Result<(), AudioError> {
        self.check(spectrum.len(), self.bins())?;
        self.check(output.len(), self.len)?;
        self.inverse
            .process(spectrum, output)
            .map_err(|_| AudioError::Fft("inverse transform rejected its buffers"))?;
        // realfft's inverse is unnormalised; scaling here keeps forward-then-
        // inverse an identity, which is what every caller in this crate wants.
        let scale = T::one() / self.scalar_len()?;
        for sample in output.iter_mut() {
            *sample = *sample * scale;
        }
        Ok(())
    }

    pub fn magnitudes(&self, windowed: &[T]) -> Result<Vec<T>, AudioError> {
        let mut input = windowed.to_vec();
        let mut spectrum = self.scratch_spectrum();
        self.forward(&mut input, &mut spectrum)?;
        Ok(spectrum.iter().map(|bin| bin.norm()).collect())
    }

    /// Squared magnitude per bin of an already-windowed frame.
    ///
    /// Unnormalised, matching [`RealFft::magnitudes`]: a caller that needs
    /// physical units divides by its own window power.
    pub fn power_spectrum(&self, windowed: &[T]) -> Result<Vec<T>, AudioError> {
        let mut input = windowed.to_vec();
        let mut spectrum = self.scratch_spectrum();
        self.forward(&mut input, &mut spectrum)?;
        Ok(spectrum
            .iter()
            .map(|bin| bin.re * bin.re + bin.im * bin.im)
            .collect())
    }

    fn scalar_len(&self) -> Result<T, AudioError> {
        T::from_usize(self.len).ok_or(AudioError::Fft("transform length is not representable"))
    }

    fn check(&self, found: usize, expected: usize) -> Result<(), AudioError> {
        if found == expected {
            Ok(())
        } else {
            Err(AudioError::LengthMismatch { expected, found })
        }
    }
}
