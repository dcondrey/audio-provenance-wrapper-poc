use super::geometry::PairBandGeometry;
use super::statistic::SlotTargets;
use crate::dsp::Rng;
use serde::Serialize;

/// Signed distance from `from` to the nearest point congruent to `to` on a lattice of period
/// `step`, in `[-step/2, step/2)`.
pub fn wrap_to_step(from: f64, to: f64, step: f64) -> f64 {
    let raw = (to - from) / step;
    let wrapped = raw - (raw + 0.5).floor();
    wrapped * step
}

pub fn residue(value: f64, step: f64) -> f64 {
    let scaled = value / step;
    (scaled - scaled.floor()) * step
}

/// Adds a fixed fraction of the lattice period to every slot.
///
/// At fraction 0.5 this maps every lattice point onto the midpoint between the two decision
/// lattices, which is the worst case for a QIM demodulator. It needs no analysis of the file and no
/// key: the shift is the same for every slot, so it is a STATIC equaliser curve of the band's own
/// published shape, cell A up and cell B down by the same amount everywhere.
#[derive(Debug, Clone)]
pub struct ConstantOffset {
    name: String,
    fraction: f64,
}

impl ConstantOffset {
    pub fn new(name: impl Into<String>, fraction: f64) -> Self {
        Self {
            name: name.into(),
            fraction,
        }
    }

    pub const fn fraction(&self) -> f64 {
        self.fraction
    }
}

impl SlotTargets for ConstantOffset {
    fn name(&self) -> &str {
        &self.name
    }

    fn plan(
        &self,
        measured: &[Option<f64>],
        geometry: &PairBandGeometry,
        _seed: u64,
    ) -> Vec<Option<f64>> {
        let offset = self.fraction * geometry.step();
        measured
            .iter()
            .map(|value| value.map(|d| d + offset))
            .collect()
    }
}

/// Adds an independent uniform draw over a span of the lattice period to every slot.
#[derive(Debug, Clone)]
pub struct RandomOffset {
    name: String,
    span_fraction: f64,
}

impl RandomOffset {
    pub fn new(name: impl Into<String>, span_fraction: f64) -> Self {
        Self {
            name: name.into(),
            span_fraction,
        }
    }
}

impl SlotTargets for RandomOffset {
    fn name(&self) -> &str {
        &self.name
    }

    fn plan(
        &self,
        measured: &[Option<f64>],
        geometry: &PairBandGeometry,
        seed: u64,
    ) -> Vec<Option<f64>> {
        let span = self.span_fraction * geometry.step();
        let mut rng = Rng::new(seed);
        measured
            .iter()
            .map(|value| {
                let draw = f64::from(rng.next_symmetric()) * span / 2.0;
                value.map(|d| d + draw)
            })
            .collect()
    }
}

/// Replaces every slot statistic with a local median of its neighbours, which is what a smoothing
/// or "restoration" attack does to a modulation that lives in the slot-to-slot variation.
#[derive(Debug, Clone)]
pub struct Flatten {
    name: String,
    half_window: usize,
}

impl Flatten {
    pub fn new(name: impl Into<String>, half_window: usize) -> Self {
        Self {
            name: name.into(),
            half_window: half_window.max(1),
        }
    }
}

impl SlotTargets for Flatten {
    fn name(&self) -> &str {
        &self.name
    }

    fn plan(
        &self,
        measured: &[Option<f64>],
        _geometry: &PairBandGeometry,
        _seed: u64,
    ) -> Vec<Option<f64>> {
        (0..measured.len())
            .map(|slot| {
                measured[slot]?;
                let low = slot.saturating_sub(self.half_window);
                let high = (slot + self.half_window + 1).min(measured.len());
                let mut window: Vec<f64> = measured[low..high].iter().filter_map(|v| *v).collect();
                super::statistic::percentile(&mut window, 0.5)
            })
            .collect()
    }
}

/// Drives every slot onto a chosen coset of the lattice.
///
/// This is the transplant primitive. The residues can be estimated from a marked donor with no key
/// at all, because the coset a slot sits on is exactly what the detector reads.
#[derive(Debug, Clone)]
pub struct CosetTransplant {
    name: String,
    residues: Vec<Option<f64>>,
}

impl CosetTransplant {
    pub fn new(name: impl Into<String>, residues: Vec<Option<f64>>) -> Self {
        Self {
            name: name.into(),
            residues,
        }
    }
}

impl SlotTargets for CosetTransplant {
    fn name(&self) -> &str {
        &self.name
    }

    fn plan(
        &self,
        measured: &[Option<f64>],
        geometry: &PairBandGeometry,
        _seed: u64,
    ) -> Vec<Option<f64>> {
        let step = geometry.step();
        let period = self.residues.len().max(1);
        measured
            .iter()
            .enumerate()
            .map(|(slot, value)| {
                let d = (*value)?;
                let target = (*self.residues.get(slot % period)?)?;
                Some(d + wrap_to_step(d, target, step))
            })
            .collect()
    }
}

/// The coset each slot index within a block sits on, estimated across the blocks of one file.
///
/// A marked file repeats the same dither and the same coded word in every block of a key epoch, so
/// the statistic's residue modulo the lattice period is the same at a given slot-in-block index in
/// every block. `concentration` is the mean resultant length of those residues read as angles: 1.0
/// is a perfect lattice, and unmarked audio sits near the value a uniform draw would give.
#[derive(Debug, Clone, Serialize)]
pub struct ResidueEstimate {
    pub period: usize,
    pub blocks_observed: usize,
    pub residues: Vec<Option<f64>>,
    pub concentration: Option<f64>,
    pub slots_estimated: usize,
}

pub fn estimate_residues(measured: &[Option<f64>], geometry: &PairBandGeometry) -> ResidueEstimate {
    let period = geometry.slots_per_block();
    let step = geometry.step();
    let mut sines = vec![0.0f64; period];
    let mut cosines = vec![0.0f64; period];
    let mut counts = vec![0usize; period];
    for (slot, value) in measured.iter().enumerate() {
        let Some(d) = value else { continue };
        let angle = core::f64::consts::TAU * residue(*d, step) / step;
        let index = slot % period;
        sines[index] += angle.sin();
        cosines[index] += angle.cos();
        counts[index] += 1;
    }
    let mut resultants: Vec<f64> = Vec::new();
    let residues: Vec<Option<f64>> = (0..period)
        .map(|index| {
            if counts[index] == 0 {
                return None;
            }
            let mean_sin = sines[index] / counts[index] as f64;
            let mean_cos = cosines[index] / counts[index] as f64;
            if counts[index] > 1 {
                resultants.push(mean_sin.hypot(mean_cos));
            }
            let angle = mean_sin.atan2(mean_cos);
            let unit = angle / core::f64::consts::TAU;
            Some((unit - unit.floor()) * step)
        })
        .collect();
    let slots_estimated = residues.iter().filter(|value| value.is_some()).count();
    let blocks_observed = measured.len().div_ceil(period);
    ResidueEstimate {
        period,
        blocks_observed,
        concentration: super::statistic::percentile(&mut resultants, 0.5),
        residues,
        slots_estimated,
    }
}
