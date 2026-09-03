use audio_provenance_audio::window::{self, Symmetry};
use rustfft::num_complex::Complex;

use crate::params::{
    FRAMES_PER_SLOT, PUNCTURE_BAND_POWER, PUNCTURE_CELL_POWER, PUNCTURE_MIN_PAIRS,
};
use crate::statistic::{SLOT_SAMPLES, pair_difference, trimmed_mean};

/// sqrt-Hann: squared it is Hann, which is COLA at 50% overlap, so weighted overlap-add
/// reconstruction through [`audio_provenance_audio::Stft`] is exact.
pub fn sqrt_hann(len: usize) -> Vec<f32> {
    window::hann(len, Symmetry::Periodic)
        .into_iter()
        .map(|value| value.max(0.0).sqrt())
        .collect()
}

pub fn window_power(window: &[f32]) -> f64 {
    window
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum()
}

/// Bins the active pairs occupy, used to put the puncture thresholds in signal units rather than
/// in the transform's own unnormalised scale.
pub fn active_bin_count(edges: &[usize], active: &[usize]) -> usize {
    active
        .iter()
        .map(|&pair| edges[2 * pair + 2].saturating_sub(edges[2 * pair]))
        .sum()
}

pub fn cell_energy(spectrum: &[Complex<f32>], from: usize, to: usize) -> f64 {
    spectrum
        .get(from..to)
        .map(|slice| {
            slice
                .iter()
                .map(|bin| {
                    f64::from(bin.re) * f64::from(bin.re) + f64::from(bin.im) * f64::from(bin.im)
                })
                .sum()
        })
        .unwrap_or(0.0)
}

#[derive(Debug, Clone, Copy)]
pub struct SlotMeasurement {
    pub statistic: f64,
    pub punctured: bool,
}

/// The slot statistic and the puncture decision, sharing one pass over the two frames.
///
/// `normaliser` divides raw transform energy into per-bin signal power so the spec's absolute
/// -70 dBFS and 1e-12 thresholds mean what they say at any transform length.
pub fn slot_measurement<'a, F>(
    frame_spectrum: F,
    first_frame: usize,
    edges: &[usize],
    active: &[usize],
    window_power: f64,
    band_bins: usize,
) -> Option<SlotMeasurement>
where
    F: Fn(usize) -> Option<&'a [Complex<f32>]>,
{
    let mut values = Vec::with_capacity(SLOT_SAMPLES);
    let mut band_energy = 0.0f64;
    let mut usable = vec![true; active.len()];

    for offset in 0..FRAMES_PER_SLOT {
        let spectrum = frame_spectrum(first_frame + offset)?;
        for (index, &pair) in active.iter().enumerate() {
            let low = edges[2 * pair];
            let middle = edges[2 * pair + 1];
            let high = edges[2 * pair + 2];
            let energy_a = cell_energy(spectrum, low, middle);
            let energy_b = cell_energy(spectrum, middle, high);
            values.push(pair_difference(energy_a, energy_b));
            band_energy += energy_a + energy_b;
            let scale_a = ((middle - low) as f64 * window_power).max(f64::MIN_POSITIVE);
            let scale_b = ((high - middle) as f64 * window_power).max(f64::MIN_POSITIVE);
            if energy_a / scale_a <= PUNCTURE_CELL_POWER
                || energy_b / scale_b <= PUNCTURE_CELL_POWER
            {
                usable[index] = false;
            }
        }
    }

    let denominator =
        (band_bins as f64 * FRAMES_PER_SLOT as f64 * window_power).max(f64::MIN_POSITIVE);
    let band_power = band_energy / denominator;
    let punctured = band_power < PUNCTURE_BAND_POWER
        || usable.iter().filter(|&&good| good).count() < PUNCTURE_MIN_PAIRS;

    let statistic = trimmed_mean(&mut values)?;
    Some(SlotMeasurement {
        statistic,
        punctured,
    })
}
