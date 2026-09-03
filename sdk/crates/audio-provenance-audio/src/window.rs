use core::f64::consts::PI;

/// `Periodic` (denominator `n`) is the DFT-even form that satisfies COLA for
/// overlap-add. `Symmetric` (denominator `n - 1`) is what
/// `AudioObserver::computeSpectralCentroid` applies, so analysis parity with the
/// POC needs that one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symmetry {
    Periodic,
    Symmetric,
}

impl Symmetry {
    fn denominator(self, len: usize) -> f64 {
        match self {
            Self::Periodic => len as f64,
            Self::Symmetric => (len - 1) as f64,
        }
    }
}

fn cosine_series(len: usize, symmetry: Symmetry, coefficients: &[f64]) -> Vec<f32> {
    if len == 0 {
        return Vec::new();
    }
    if len == 1 {
        return vec![1.0];
    }
    let denominator = symmetry.denominator(len);
    (0..len)
        .map(|index| {
            let phase = 2.0 * PI * index as f64 / denominator;
            let value: f64 = coefficients
                .iter()
                .enumerate()
                .map(|(term, coefficient)| {
                    let sign = if term % 2 == 0 { 1.0 } else { -1.0 };
                    sign * coefficient * (phase * term as f64).cos()
                })
                .sum();
            value as f32
        })
        .collect()
}

pub fn hann(len: usize, symmetry: Symmetry) -> Vec<f32> {
    cosine_series(len, symmetry, &[0.5, 0.5])
}

pub fn blackman_harris(len: usize, symmetry: Symmetry) -> Vec<f32> {
    cosine_series(len, symmetry, &[0.35875, 0.48829, 0.14128, 0.01168])
}

/// Princen-Bradley sine window for MDCT/IMDCT: `w[n]^2 + w[n + m]^2 == 1`, the
/// condition that makes time-domain alias cancellation exact.
pub fn princen_bradley(half_len: usize) -> Vec<f32> {
    let block = half_len * 2;
    (0..block)
        .map(|n| (PI / block as f64 * (n as f64 + 0.5)).sin() as f32)
        .collect()
}
