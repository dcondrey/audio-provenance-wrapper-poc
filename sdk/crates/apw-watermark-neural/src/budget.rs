//! The per-bin, per-frame log-gain ceiling the encoder's output is multiplied by (spec 2.7).
//!
//! IMPORTANT: this is deliberately the same psychoacoustic model `audio-provenance-bench/src/perceptual.rs`
//! implements -- Schroeder spreading over a half-Bark partition, tonality alpha = 0.5, absolute
//! threshold anchored at 96 dB SPL for digital full scale -- so that the training objective, the
//! embed-time budget and the bench measurement are one function and a training win cannot be a
//! measurement artifact. The bench keeps its model private and on its own 2048/1024 native-rate
//! grid, and this crate does not own that crate.
//! TODO: factor the masking model into a shared crate once one owner holds both; two copies of a
//! psychoacoustic model is exactly the divergence spec 9.4 warns about.

use crate::params::{BAND_BIN_HIGH, BAND_BIN_LOW, BAND_BINS};

const FULL_SCALE_SPL_DB: f64 = 96.0;
const TONALITY_ALPHA: f64 = 0.5;

fn bark(frequency_hz: f64) -> f64 {
    13.0 * (0.00076 * frequency_hz).atan() + 3.5 * (frequency_hz / 7500.0).powi(2).atan()
}

fn absolute_threshold_db(frequency_hz: f64) -> f64 {
    let khz = (frequency_hz / 1000.0).max(0.02);
    3.64 * khz.powf(-0.8) - 6.5 * (-0.6 * (khz - 3.3).powi(2)).exp() + 1e-3 * khz.powi(4)
}

fn spreading_db(delta_bark: f64) -> f64 {
    let x = delta_bark + 0.474;
    15.81 + 7.5 * x - 17.5 * (1.0 + x * x).sqrt()
}

#[derive(Debug)]
pub struct MaskingModel {
    band_of_bin: Vec<usize>,
    bins_in_band: Vec<f64>,
    centres: Vec<f64>,
    thresholds_in_quiet: Vec<f64>,
    window_power: f64,
    kappa: f64,
    max_nepers: f64,
}

impl MaskingModel {
    pub fn new(
        sample_rate: u32,
        bins: usize,
        window_power: f64,
        kappa: f64,
        max_nepers: f64,
    ) -> Self {
        let nyquist = f64::from(sample_rate) / 2.0;
        let mut band_of_bin = Vec::with_capacity(bins);
        let mut band_count = 0usize;
        for k in 0..bins {
            let frequency = nyquist * k as f64 / (bins - 1).max(1) as f64;
            let band = (bark(frequency) / 0.5).floor().max(0.0) as usize;
            band_count = band_count.max(band + 1);
            band_of_bin.push(band);
        }
        let centres: Vec<f64> = (0..band_count).map(|b| b as f64 * 0.5 + 0.25).collect();
        let mut bins_in_band = vec![0.0f64; band_count];
        let mut thresholds_in_quiet = vec![f64::INFINITY; band_count];
        for (k, &band) in band_of_bin.iter().enumerate() {
            if let Some(slot) = bins_in_band.get_mut(band) {
                *slot += 1.0;
            }
            let frequency = nyquist * k as f64 / (bins - 1).max(1) as f64;
            let spl = absolute_threshold_db(frequency).min(90.0);
            let power = 10f64.powf((spl - FULL_SCALE_SPL_DB) / 10.0);
            if let Some(slot) = thresholds_in_quiet.get_mut(band)
                && power < *slot
            {
                *slot = power;
            }
        }
        for threshold in &mut thresholds_in_quiet {
            if !threshold.is_finite() {
                *threshold = 1e-12;
            }
        }
        Self {
            band_of_bin,
            bins_in_band,
            centres,
            thresholds_in_quiet,
            window_power,
            kappa,
            max_nepers,
        }
    }

    fn band_thresholds(&self, power: &[f64]) -> Vec<f64> {
        let mut bands = vec![0.0f64; self.centres.len()];
        for (k, &value) in power.iter().enumerate() {
            if let Some(&band) = self.band_of_bin.get(k)
                && let Some(slot) = bands.get_mut(band)
            {
                *slot += value;
            }
        }
        let mut threshold = vec![0.0f64; self.centres.len()];
        for (b, slot) in threshold.iter_mut().enumerate() {
            let zb = self.centres[b];
            let mut spread = 0.0;
            for (j, &energy) in bands.iter().enumerate() {
                if energy <= 0.0 {
                    continue;
                }
                spread += energy * 10f64.powf(spreading_db(zb - self.centres[j]) / 10.0);
            }
            let offset_db = TONALITY_ALPHA * (14.5 + zb) + (1.0 - TONALITY_ALPHA) * 5.5;
            *slot = (spread / 10f64.powf(offset_db / 10.0)).max(self.thresholds_in_quiet[b]);
        }
        threshold
    }

    /// `B[t,f] = clamp(kappa * 10^((M - L) / 20), 0, max_nepers)`, laid out to match the tensor the
    /// encoder emits: `[320, T]` frequency-major.
    pub fn budget(&self, frames: &audio_provenance_audio::StftFrames) -> Vec<f32> {
        let time = frames.frames();
        let bins = frames.bins();
        let mut out = vec![0.0f32; BAND_BINS * time];
        let mut power = vec![0.0f64; bins];
        for t in 0..time {
            let Some(frame) = frames.frame(t) else {
                continue;
            };
            for (slot, value) in power.iter_mut().zip(frame.iter()) {
                *slot = f64::from(value.norm_sqr()) / self.window_power;
            }
            let thresholds = self.band_thresholds(&power);
            for (row, bin) in (BAND_BIN_LOW..=BAND_BIN_HIGH).enumerate() {
                let band = self.band_of_bin.get(bin).copied().unwrap_or(0);
                let per_bin_threshold = thresholds.get(band).copied().unwrap_or(0.0).max(0.0)
                    / self.bins_in_band.get(band).copied().unwrap_or(1.0).max(1.0);
                let signal = power.get(bin).copied().unwrap_or(0.0).max(1e-30);
                let ratio = (per_bin_threshold / signal).sqrt();
                out[row * time + t] = (self.kappa * ratio).clamp(0.0, self.max_nepers) as f32;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{ANALYSIS_SAMPLE_RATE, N_FFT};
    use crate::spectral::Analysis;

    fn tone(frequency: f64, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| {
                (0.25
                    * (core::f64::consts::TAU * frequency * n as f64
                        / f64::from(ANALYSIS_SAMPLE_RATE))
                    .sin()) as f32
            })
            .collect()
    }

    /// The ceiling is what makes fidelity a property of the budget rather than a hoped-for outcome
    /// of a loss weight. It binds on loud tonal content and it is never exceeded anywhere.
    #[test]
    fn never_exceeds_the_hard_ceiling_and_binds_on_a_loud_tone() {
        let analysis = Analysis::new().unwrap();
        let frames = analysis.forward(&tone(1000.0, 48_000)).unwrap();
        let model = MaskingModel::new(
            ANALYSIS_SAMPLE_RATE,
            N_FFT / 2 + 1,
            analysis.window_power(),
            0.5,
            crate::params::MAX_BUDGET_NEPERS,
        );
        let budget = model.budget(&frames);
        assert!(budget.iter().all(|value| value.is_finite()));
        assert!(
            budget
                .iter()
                .all(|value| (0.0..=crate::params::MAX_BUDGET_NEPERS as f32).contains(value))
        );
        let bin_1k = (1000.0 / (f64::from(ANALYSIS_SAMPLE_RATE) / N_FFT as f64)).round() as usize;
        let row = bin_1k - crate::params::BAND_BIN_LOW;
        let time = frames.frames();
        let centre = time / 2;
        assert!(
            budget[row * time + centre] < crate::params::MAX_BUDGET_NEPERS as f32,
            "the masker's own bin must be budget-limited, not ceiling-limited"
        );
    }
}
