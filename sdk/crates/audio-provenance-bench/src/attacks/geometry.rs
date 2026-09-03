use crate::error::ChannelError;

/// Where a QIM-over-adjacent-cell-energy-differences watermark keeps its statistic: cell boundaries
/// in Hz, which pairs of cells carry it, and how many transform frames make one slot.
///
/// IMPORTANT: nothing in this struct is secret. Every field is published in the algorithm's own
/// specification, which is what makes the attacks built on it available to anyone who reads it. The
/// key controls the dither and the polarity schedule, not the geometry.
#[derive(Debug, Clone)]
pub struct PairBandGeometry {
    sample_rate: u32,
    frame: usize,
    frames_per_slot: usize,
    trim_each_tail: usize,
    slots_per_block: usize,
    step: f64,
    cell_edges_hz: Vec<f64>,
    active_pairs: Vec<usize>,
    edge_bins: Vec<usize>,
}

/// Slot statistic below which a slot carries too little energy for an attack to move it, matching
/// the puncture rule a spec-reading attacker copies out of the detector's own admission test.
#[derive(Debug, Clone, Copy)]
pub struct PunctureRule {
    pub band_power: f64,
    pub cell_power: f64,
    pub min_pairs: usize,
}

impl PairBandGeometry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sample_rate: u32,
        frame: usize,
        frames_per_slot: usize,
        trim_each_tail: usize,
        slots_per_block: usize,
        step: f64,
        cell_edges_hz: Vec<f64>,
        active_pairs: Vec<usize>,
    ) -> Result<Self, ChannelError> {
        let bad = |reason: &str| ChannelError::Parameter {
            parameter: "pair_band_geometry",
            reason: reason.to_owned(),
        };
        if sample_rate == 0 || frame < 4 || !frame.is_multiple_of(2) {
            return Err(bad(
                "sample rate must be positive and the frame even and at least 4",
            ));
        }
        if frames_per_slot == 0 || slots_per_block == 0 || !(step.is_finite() && step > 0.0) {
            return Err(bad(
                "frames per slot, slots per block and the lattice step must be positive",
            ));
        }
        if cell_edges_hz.len() < 3 || cell_edges_hz.len().is_multiple_of(2) {
            return Err(bad(
                "cell edges must be an odd count of at least three boundaries",
            ));
        }
        if cell_edges_hz
            .windows(2)
            .any(|pair| !pair[0].is_finite() || pair[1] <= pair[0])
        {
            return Err(bad("cell edges must be finite and strictly increasing"));
        }
        if active_pairs.len() * frames_per_slot <= 2 * trim_each_tail {
            return Err(bad("too few active pairs to survive the trimmed mean"));
        }
        let pairs = (cell_edges_hz.len() - 1) / 2;
        if active_pairs.iter().any(|&pair| pair >= pairs) {
            return Err(bad("an active pair index is outside the cell layout"));
        }
        let scale = frame as f64 / f64::from(sample_rate);
        let edge_bins: Vec<usize> = cell_edges_hz
            .iter()
            .map(|hz| (hz * scale).round().max(0.0) as usize)
            .collect();
        if edge_bins.windows(2).any(|pair| pair[1] <= pair[0]) {
            return Err(bad(
                "cell boundaries collapse onto the same bin at this rate and frame",
            ));
        }
        if edge_bins.last().copied().unwrap_or(usize::MAX) > frame / 2 + 1 {
            return Err(bad("the band runs past nyquist at this rate and frame"));
        }
        Ok(Self {
            sample_rate,
            frame,
            frames_per_slot,
            trim_each_tail,
            slots_per_block,
            step,
            cell_edges_hz,
            active_pairs,
            edge_bins,
        })
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

    pub const fn frames_per_slot(&self) -> usize {
        self.frames_per_slot
    }

    pub const fn trim_each_tail(&self) -> usize {
        self.trim_each_tail
    }

    pub const fn slots_per_block(&self) -> usize {
        self.slots_per_block
    }

    /// The lattice period the statistic is quantised on. Every removal target in [`super::erase`]
    /// is expressed as a fraction of it.
    pub const fn step(&self) -> f64 {
        self.step
    }

    pub fn edge_bins(&self) -> &[usize] {
        &self.edge_bins
    }

    pub fn active_pairs(&self) -> &[usize] {
        &self.active_pairs
    }

    pub fn band_bins(&self) -> usize {
        self.active_pairs
            .iter()
            .map(|&pair| self.edge_bins[2 * pair + 2] - self.edge_bins[2 * pair])
            .sum()
    }

    pub fn slot_of_frame_count(&self, frames: usize) -> usize {
        frames / self.frames_per_slot
    }

    pub fn band_hz(&self) -> (f64, f64) {
        (
            self.cell_edges_hz.first().copied().unwrap_or(0.0),
            self.cell_edges_hz.last().copied().unwrap_or(0.0),
        )
    }
}

impl PunctureRule {
    pub const fn new(band_power: f64, cell_power: f64, min_pairs: usize) -> Self {
        Self {
            band_power,
            cell_power,
            min_pairs,
        }
    }
}
