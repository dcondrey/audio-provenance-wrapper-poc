use crate::error::AudioError;
use crate::fft::RealFft;
use crate::window::{Symmetry, hann};

/// Band split used by `AudioObserver::computeSpectralBands`.
pub const LOW_BAND_HZ: f64 = 300.0;
pub const MID_BAND_HZ: f64 = 4000.0;

/// RMS below which the POC treats a window as silent.
pub const SILENCE_THRESHOLD: f64 = 0.001;

const ENVELOPE_SEGMENTS: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BandEnergies {
    pub low: f64,
    pub mid: f64,
    pub high: f64,
}

pub fn peak(samples: &[f32]) -> f64 {
    samples
        .iter()
        .fold(0.0f64, |acc, value| acc.max(f64::from(*value).abs()))
}

pub fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum();
    (sum / samples.len() as f64).sqrt()
}

/// Sign flips per adjacent pair, divided by `len - 1`.
///
/// IMPORTANT: the predicate is `>= 0.0`, so an exact zero counts as
/// non-negative. Writing this with `signum` gives a different answer at zero and
/// silently diverges from the POC on any gated or faded signal.
pub fn zero_crossing_rate(samples: &[f32]) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    let crossings = samples
        .windows(2)
        .filter(|pair| (pair[0] >= 0.0) != (pair[1] >= 0.0))
        .count();
    crossings as f64 / (samples.len() - 1) as f64
}

pub fn crest_factor(samples: &[f32]) -> f64 {
    let level = rms(samples);
    if level > 1.0e-9 {
        peak(samples) / level
    } else {
        0.0
    }
}

/// Per-segment RMS normalised by the whole-window RMS.
///
/// A segment can be empty below four samples, where the POC's `computeRMS`
/// would divide by zero and return NaN. Returning 0.0 for an empty segment is a
/// deliberate divergence; the POC's window is always 4096 so it cannot reach it.
pub fn energy_envelope(samples: &[f32]) -> [f64; ENVELOPE_SEGMENTS] {
    let level = rms(samples);
    let mut envelope = [0.0f64; ENVELOPE_SEGMENTS];
    if level <= 1.0e-9 {
        return envelope;
    }
    for (segment, slot) in envelope.iter_mut().enumerate() {
        let start = segment * samples.len() / ENVELOPE_SEGMENTS;
        let end = (segment + 1) * samples.len() / ENVELOPE_SEGMENTS;
        *slot = rms(&samples[start..end]) / level;
    }
    envelope
}

/// Magnitude spectrum of a Hann-windowed frame.
///
/// The window is the symmetric form and the usable bin range is `1..len / 2`,
/// both matching the POC. Magnitudes are unnormalised, which is harmless because
/// every metric derived from them is a ratio.
///
/// IMPORTANT: an odd `samples.len()` is rejected rather than padded. Padding
/// would change `bin_hz` and silently move every centroid reading, and the POC
/// never analyses a partial window either: its FIFO only ever emits full
/// 4096-sample blocks. Callers with a ragged tail must drop it or pad to the
/// analysis size themselves.
#[derive(Clone, Debug, PartialEq)]
pub struct Spectrum {
    magnitudes: Vec<f32>,
    bin_hz: f64,
    bins: usize,
}

impl Spectrum {
    pub fn analyze(samples: &[f32], sample_rate: u32) -> Result<Self, AudioError> {
        if sample_rate == 0 {
            return Err(AudioError::InvalidSampleRate { found: 0 });
        }
        let fft = RealFft::new(samples.len())?;
        let window = hann(samples.len(), Symmetry::Symmetric);
        let windowed: Vec<f32> = samples
            .iter()
            .zip(window.iter())
            .map(|(sample, weight)| sample * weight)
            .collect();
        Ok(Self {
            magnitudes: fft.magnitudes(&windowed)?,
            bin_hz: f64::from(sample_rate) / samples.len() as f64,
            bins: samples.len() / 2,
        })
    }

    pub fn magnitudes(&self) -> &[f32] {
        &self.magnitudes
    }

    pub const fn bin_hz(&self) -> f64 {
        self.bin_hz
    }

    pub fn centroid_hz(&self) -> f64 {
        let mut weighted = 0.0f64;
        let mut total = 0.0f64;
        for index in 1..self.bins {
            let magnitude = f64::from(self.magnitudes[index]);
            weighted += index as f64 * self.bin_hz * magnitude;
            total += magnitude;
        }
        if total > 0.0 { weighted / total } else { 0.0 }
    }

    pub fn band_energies(&self) -> BandEnergies {
        let low_cut = ((LOW_BAND_HZ / self.bin_hz) as usize).min(self.bins);
        let mid_cut = ((MID_BAND_HZ / self.bin_hz) as usize).min(self.bins);

        let mut low = 0.0f64;
        let mut mid = 0.0f64;
        let mut high = 0.0f64;
        for index in 1..self.bins {
            let magnitude = f64::from(self.magnitudes[index]);
            let energy = magnitude * magnitude;
            if index < low_cut {
                low += energy;
            } else if index < mid_cut {
                mid += energy;
            } else {
                high += energy;
            }
        }

        let total = low + mid + high;
        if total <= 0.0 {
            return BandEnergies::default();
        }
        BandEnergies {
            low: low / total,
            mid: mid / total,
            high: high / total,
        }
    }
}

/// Every per-window figure the POC's `buffer_hash` event carries, from a single
/// forward transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowMetrics {
    pub peak: f64,
    pub rms: f64,
    pub zero_crossing_rate: f64,
    pub crest_factor: f64,
    pub spectral_centroid_hz: f64,
    pub bands: BandEnergies,
    pub energy_envelope: [f64; ENVELOPE_SEGMENTS],
    pub has_audio: bool,
}

pub fn window_metrics(samples: &[f32], sample_rate: u32) -> Result<WindowMetrics, AudioError> {
    let spectrum = Spectrum::analyze(samples, sample_rate)?;
    let level = rms(samples);
    Ok(WindowMetrics {
        peak: peak(samples),
        rms: level,
        zero_crossing_rate: zero_crossing_rate(samples),
        crest_factor: crest_factor(samples),
        spectral_centroid_hz: spectrum.centroid_hz(),
        bands: spectrum.band_energies(),
        energy_envelope: energy_envelope(samples),
        has_audio: level > SILENCE_THRESHOLD,
    })
}
