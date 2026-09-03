use audio_provenance_audio::window::{Symmetry, hann};
use audio_provenance_audio::{AudioBuffer, AudioError, Stft, StftFrames};

use crate::params::{
    ANALYSIS_SAMPLE_RATE, BAND_BIN_HIGH, BAND_BIN_LOW, BAND_BINS, HOP, LOG_FLOOR, N_FFT,
};

/// The analysis front end. THE TRANSFORM IS OWNED BY RUST, NOT BY THE GRAPH.
///
/// `torch.stft` exports to ONNX badly and complex tensors are a standing ONNX pain point, so the
/// exported graph is pure convolution and everything below is the Rust side's responsibility. The
/// PyTorch side MUST reproduce this framing exactly; see the crate-level docs for the numbered
/// contract, which is the authority.
#[derive(Debug)]
pub struct Analysis {
    stft: Stft,
    window_power: f64,
}

impl Analysis {
    pub fn new() -> Result<Self, AudioError> {
        let window: Vec<f32> = hann(N_FFT, Symmetry::Periodic)
            .into_iter()
            .map(|value| value.max(0.0).sqrt())
            .collect();
        let window_power: f64 = window
            .iter()
            .map(|value| f64::from(*value) * f64::from(*value))
            .sum::<f64>()
            * N_FFT as f64;
        Ok(Self {
            stft: Stft::new(window, HOP)?,
            window_power,
        })
    }

    pub fn frame_count(&self, signal_len: usize) -> usize {
        self.stft.frame_count(signal_len)
    }

    pub const fn window_power(&self) -> f64 {
        self.window_power
    }

    pub fn forward(&self, signal: &[f32]) -> Result<StftFrames, AudioError> {
        self.stft.forward(signal)
    }

    pub fn inverse(&self, frames: &StftFrames, signal_len: usize) -> Result<Vec<f32>, AudioError> {
        self.stft.inverse(frames, signal_len)
    }

    /// `[1, 1, 320, T]` row-major: frequency outer, time inner, natural log of the magnitude
    /// floored at `LOG_FLOOR`.
    pub fn band_log_magnitude(&self, frames: &StftFrames) -> Vec<f32> {
        let time = frames.frames();
        let mut tensor = vec![0.0f32; BAND_BINS * time];
        for t in 0..time {
            let Some(frame) = frames.frame(t) else {
                continue;
            };
            for (row, bin) in (BAND_BIN_LOW..=BAND_BIN_HIGH).enumerate() {
                let magnitude = frame.get(bin).map_or(0.0, |value| value.norm());
                tensor[row * time + t] = magnitude.max(LOG_FLOOR).ln();
            }
        }
        tensor
    }

    /// Applies a per-bin log gain in nepers to the band, leaving phase and every bin outside
    /// `BAND_BIN_LOW..=BAND_BIN_HIGH` bit-identical.
    pub fn apply_log_gain(&self, frames: &mut StftFrames, gain: &[f32]) {
        let time = frames.frames();
        for t in 0..time {
            let Some(frame) = frames.frame_mut(t) else {
                continue;
            };
            for (row, bin) in (BAND_BIN_LOW..=BAND_BIN_HIGH).enumerate() {
                let Some(&g) = gain.get(row * time + t) else {
                    continue;
                };
                if let Some(value) = frame.get_mut(bin) {
                    *value *= g.exp();
                }
            }
        }
    }
}

/// A resampled COPY at the model rate. The host is never resampled (spec 2.2); only the analysis
/// copy and, on the embed path, the residual.
pub fn at_analysis_rate(buffer: &AudioBuffer) -> Result<AudioBuffer, AudioError> {
    if buffer.sample_rate() == ANALYSIS_SAMPLE_RATE {
        return Ok(buffer.clone());
    }
    audio_provenance_audio::resample(buffer, ANALYSIS_SAMPLE_RATE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::MAX_BUDGET_NEPERS;

    fn signal(len: usize) -> Vec<f32> {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state >> 11) as f32 / (1u64 << 53) as f32) * 0.6 - 0.3
            })
            .collect()
    }

    /// The WOLA path this crate inherits reconstructs exactly, so an unmodified round trip must
    /// return the input. If it does not, every residual the embed path forms is contaminated by
    /// transform error rather than by the mark.
    #[test]
    fn reconstructs_an_untouched_signal() {
        let analysis = Analysis::new().unwrap();
        let input = signal(20_000);
        let frames = analysis.forward(&input).unwrap();
        let output = analysis.inverse(&frames, input.len()).unwrap();
        let worst = input
            .iter()
            .zip(output.iter())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "worst reconstruction error {worst}");
    }

    /// Spec 2.4: everything outside bins 9..=328 is untouched, EXACTLY. A mask that leaked outside
    /// the band would put energy where consumer transducers roll off and where the ear is most
    /// sensitive to level change, and would buy nothing back.
    #[test]
    fn leaves_every_bin_outside_the_band_bit_identical() {
        let analysis = Analysis::new().unwrap();
        let frames = analysis.forward(&signal(8_000)).unwrap();
        let mut marked = frames.clone();
        let gain = vec![MAX_BUDGET_NEPERS as f32; BAND_BINS * frames.frames()];
        analysis.apply_log_gain(&mut marked, &gain);
        for t in 0..frames.frames() {
            let before = frames.frame(t).unwrap();
            let after = marked.frame(t).unwrap();
            for bin in 0..before.len() {
                if (BAND_BIN_LOW..=BAND_BIN_HIGH).contains(&bin) {
                    continue;
                }
                assert_eq!(before[bin], after[bin], "frame {t} bin {bin}");
            }
        }
    }
}
