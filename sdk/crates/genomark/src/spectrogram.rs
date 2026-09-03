use audio_provenance_audio::fft::RealFft;
use rustfft::num_complex::Complex;

use crate::analysis::sqrt_hann;
use crate::error::GenoMarkError;
use crate::statistic::pair_difference;

/// Band-limited power spectrogram at the detector hop.
///
/// Only the bins any rate hypothesis can reach are kept. The full magnitude array for a four-minute
/// track is hundreds of megabytes; the band is two orders of magnitude smaller and is all the
/// statistic reads.
#[derive(Debug)]
pub struct Spectrogram {
    frames: usize,
    low_bin: usize,
    span: usize,
    power: Vec<f32>,
    hop: usize,
    pad: usize,
}

impl Spectrogram {
    pub fn analyse(
        signal: &[f32],
        frame: usize,
        hop: usize,
        low_bin: usize,
        high_bin: usize,
    ) -> Result<Self, GenoMarkError> {
        let window = sqrt_hann(frame);
        let fft = RealFft::<f32>::new(frame)?;
        // The embed grid pads by one whole frame, so the detector has to pad identically for
        // detector frame `8t` to be embed frame `t`.
        let pad = frame;
        let frames = (pad + signal.len()).div_ceil(hop);
        let padded_len = (frames - 1) * hop + frame;
        let mut padded = vec![0.0f32; padded_len];
        padded[pad..pad + signal.len()].copy_from_slice(signal);

        let span = high_bin
            .checked_sub(low_bin)
            .and_then(|value| value.checked_add(1))
            .ok_or(GenoMarkError::BandOutOfRange {
                top: high_bin,
                bins: frame / 2 + 1,
                frame,
            })?;
        let mut power = vec![0.0f32; frames * span];
        let mut block = fft.scratch_input();
        let mut spectrum = vec![Complex::new(0.0f32, 0.0); fft.bins()];
        for index in 0..frames {
            let base = index * hop;
            for (offset, slot) in block.iter_mut().enumerate() {
                *slot = padded[base + offset] * window[offset];
            }
            fft.forward(&mut block, &mut spectrum)?;
            let out = &mut power[index * span..(index + 1) * span];
            for (slot, bin) in out.iter_mut().zip(spectrum[low_bin..=high_bin].iter()) {
                *slot = bin.re * bin.re + bin.im * bin.im;
            }
        }
        Ok(Self {
            frames,
            low_bin,
            span,
            power,
            hop,
            pad,
        })
    }

    pub const fn frames(&self) -> usize {
        self.frames
    }

    pub const fn hop(&self) -> usize {
        self.hop
    }

    pub const fn pad(&self) -> usize {
        self.pad
    }

    /// Power over a FRACTIONAL bin interval, each bin's power treated as spread evenly across its
    /// own width. This is what lets a rate hypothesis move a cell boundary by a fraction of a bin,
    /// which is the whole point of re-summing rather than re-transforming.
    fn cell(&self, frame: usize, from_bin: f64, to_bin: f64) -> f64 {
        let low = self.low_bin as f64;
        let from = (from_bin - low).max(0.0);
        let to = (to_bin - low).min(self.span as f64);
        if to <= from || !to.is_finite() || !from.is_finite() {
            return 0.0;
        }
        let base = frame * self.span;
        let first = from.floor() as usize;
        let last = (to.ceil() as usize).min(self.span);
        let mut total = 0.0f64;
        for bin in first..last {
            let overlap = (bin as f64 + 1.0).min(to) - (bin as f64).max(from);
            if overlap > 0.0 {
                total += f64::from(self.power[base + bin]) * overlap;
            }
        }
        total
    }
}

/// Per-pair log-energy differences over a contiguous run of detector frames.
#[derive(Debug)]
pub struct PairDiffs {
    from: usize,
    frames: usize,
    pairs: usize,
    values: Vec<f32>,
}

impl PairDiffs {
    pub fn compute(
        spectrogram: &Spectrogram,
        edges: &[f64],
        active: &[usize],
        from: usize,
        to: usize,
    ) -> Self {
        let from = from.min(spectrogram.frames);
        let to = to.min(spectrogram.frames);
        let frames = to.saturating_sub(from);
        let pairs = active.len();
        let mut values = vec![0.0f32; frames * pairs];
        for index in 0..frames {
            let frame = from + index;
            for (slot, &pair) in active.iter().enumerate() {
                let energy_a = spectrogram.cell(frame, edges[2 * pair], edges[2 * pair + 1]);
                let energy_b = spectrogram.cell(frame, edges[2 * pair + 1], edges[2 * pair + 2]);
                values[index * pairs + slot] = pair_difference(energy_a, energy_b) as f32;
            }
        }
        Self {
            from,
            frames,
            pairs,
            values,
        }
    }

    pub const fn pairs(&self) -> usize {
        self.pairs
    }

    /// Linear interpolation along the time axis, which is how a rate hypothesis is applied to `t`
    /// after its bin edges have been applied to `f`.
    pub fn interpolate(&self, position: f64, out: &mut [f64]) -> bool {
        if !position.is_finite() || position < self.from as f64 {
            return false;
        }
        let local = position - self.from as f64;
        let floor = local.floor();
        let index = floor as usize;
        if index + 1 >= self.frames {
            return false;
        }
        let fraction = local - floor;
        for (slot, value) in out.iter_mut().enumerate().take(self.pairs) {
            let low = f64::from(self.values[index * self.pairs + slot]);
            let high = f64::from(self.values[(index + 1) * self.pairs + slot]);
            *value = low * (1.0 - fraction) + high * fraction;
        }
        true
    }
}
