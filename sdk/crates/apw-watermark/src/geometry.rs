use crate::error::WatermarkError;
use crate::params::{
    ACTIVE_PAIRS, CELLS, GUARD_HZ, LONG_FRAME_ABOVE, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE, PAIRS,
    REFERENCE_BAND_FIRST_BIN, REFERENCE_FRAME, REFERENCE_SAMPLE_RATE,
};

pub const CELL_EDGES: usize = CELLS + 1;

/// Cell edge frequencies, fixed in Hz. At 44.1 kHz with a 1024-sample frame these round back to
/// bins 20, 22, ... 100, which is the band the spec states as bins 20..99.
pub fn cell_edge_hz(index: usize) -> f64 {
    (REFERENCE_BAND_FIRST_BIN + 2 * index) as f64 * REFERENCE_SAMPLE_RATE / REFERENCE_FRAME as f64
}

pub const fn frame_length(sample_rate: u32) -> usize {
    if sample_rate > LONG_FRAME_ABOVE {
        2048
    } else {
        1024
    }
}

fn guarded(pair: usize) -> bool {
    let low = cell_edge_hz(2 * pair);
    let high = cell_edge_hz(2 * pair + 2);
    GUARD_HZ.iter().any(|&hz| hz >= low && hz < high)
}

/// Which pairs carry the statistic, the transform length, and the bin edges each rate hypothesis
/// re-derives.
#[derive(Debug, Clone)]
pub struct Band {
    sample_rate: u32,
    frame: usize,
    active: Vec<usize>,
}

impl Band {
    pub fn new(sample_rate: u32) -> Result<Self, WatermarkError> {
        if sample_rate < MIN_SAMPLE_RATE {
            return Err(WatermarkError::SampleRateTooLow {
                found: sample_rate,
                min: MIN_SAMPLE_RATE,
            });
        }
        if sample_rate > MAX_SAMPLE_RATE {
            return Err(WatermarkError::SampleRateTooHigh {
                found: sample_rate,
                max: MAX_SAMPLE_RATE,
            });
        }
        let frame = frame_length(sample_rate);
        let active: Vec<usize> = (0..PAIRS).filter(|&pair| !guarded(pair)).collect();
        if active.len() != ACTIVE_PAIRS {
            return Err(WatermarkError::GuardMaskShape {
                found: active.len(),
                expected: ACTIVE_PAIRS,
            });
        }
        let band = Self {
            sample_rate,
            frame,
            active,
        };
        band.edges(1.0)?;
        Ok(band)
    }

    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub const fn frame(&self) -> usize {
        self.frame
    }

    pub const fn hop(&self) -> usize {
        self.frame / 2
    }

    pub const fn bins(&self) -> usize {
        self.frame / 2 + 1
    }

    pub fn active_pairs(&self) -> &[usize] {
        &self.active
    }

    /// The bin edges the embedder writes against: one integer per cell boundary, derived from the
    /// fixed cell frequencies and this file's rate.
    pub fn edges(&self, rho: f64) -> Result<Vec<usize>, WatermarkError> {
        let scale = rho * self.frame as f64 / f64::from(self.sample_rate);
        let mut edges = Vec::with_capacity(CELL_EDGES);
        for index in 0..CELL_EDGES {
            let bin = (cell_edge_hz(index) * scale).round();
            if !bin.is_finite() || bin < 0.0 {
                return Err(WatermarkError::BandOutOfRange {
                    top: 0,
                    bins: self.bins(),
                    frame: self.frame,
                });
            }
            edges.push(bin as usize);
        }
        for cell in 0..CELLS {
            if edges[cell + 1] <= edges[cell] {
                return Err(WatermarkError::DegenerateCell {
                    cell,
                    sample_rate: self.sample_rate,
                    frame: self.frame,
                    rho,
                });
            }
        }
        let top = edges[CELLS];
        if top > self.bins() {
            return Err(WatermarkError::BandOutOfRange {
                top,
                bins: self.bins(),
                frame: self.frame,
            });
        }
        Ok(edges)
    }

    /// Where the embedder's integer cell boundaries land after the file was played at `rho` times
    /// its authored rate.
    ///
    /// IMPORTANT: these are FRACTIONAL bin positions, and they have to be. A 1% rate change moves
    /// the top of the band by 0.9 of a bin and the bottom by 0.2; re-summing whole bins cannot
    /// follow that, so rounding each edge to an integer leaves the rate search inert. Measured
    /// before the fix, recovery died outside about +-0.1% no matter how many hypotheses ran.
    pub fn scaled_edges(&self, rho: f64) -> Result<Vec<f64>, WatermarkError> {
        Ok(self
            .edges(1.0)?
            .into_iter()
            .map(|edge| edge as f64 * rho)
            .collect())
    }

    /// Inclusive bin span the detector has to keep for every rate hypothesis in `rhos`.
    pub fn spectrogram_span(&self, rhos: &[f64]) -> Result<(usize, usize), WatermarkError> {
        let mut low = f64::INFINITY;
        let mut high = 0.0f64;
        for &rho in rhos {
            let edges = self.scaled_edges(rho)?;
            low = low.min(edges[0]);
            high = high.max(edges[CELLS]);
        }
        let low = low.floor().max(0.0) as usize;
        let high = (high.ceil() as usize).min(self.bins().saturating_sub(1));
        if low >= high {
            return Err(WatermarkError::BandOutOfRange {
                top: high,
                bins: self.bins(),
                frame: self.frame,
            });
        }
        Ok((low, high))
    }
}
