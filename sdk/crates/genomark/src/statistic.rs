use crate::params::{ACTIVE_PAIRS, FRAMES_PER_SLOT, TRIM_EACH_TAIL};

pub const ENERGY_FLOOR: f64 = 1e-20;
pub const SLOT_SAMPLES: usize = ACTIVE_PAIRS * FRAMES_PER_SLOT;

/// Log-energy difference of one pair's two cells.
///
/// A scalar gain multiplies both cell energies by the same factor and cancels here with no memory,
/// which is the whole reason this design was chosen over a normalised statistic.
pub fn pair_difference(energy_a: f64, energy_b: f64) -> f64 {
    (energy_a + ENERGY_FLOOR).ln() - (energy_b + ENERGY_FLOOR).ln()
}

/// Mean of the middle values after dropping `TRIM_EACH_TAIL` from each tail.
///
/// A uniform shift of every input shifts the output by exactly that amount, because it preserves
/// the ordering and therefore which values are dropped. That is what keeps the embedder's closed
/// form closed.
pub fn trimmed_mean(values: &mut [f64]) -> Option<f64> {
    let kept = values.len().checked_sub(2 * TRIM_EACH_TAIL)?;
    if kept == 0 {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let sum: f64 = values[TRIM_EACH_TAIL..TRIM_EACH_TAIL + kept].iter().sum();
    Some(sum / kept as f64)
}

pub fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let middle = values.len() / 2;
    values.select_nth_unstable_by(middle, |a, b| {
        a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal)
    });
    Some(values[middle])
}

/// Signed QIM soft metric for one slot: negative for the bit-0 lattice, positive for bit-1, zero
/// exactly between them. Range [-0.5, 0.5].
///
/// IMPORTANT: spec 4.7 writes this as `(0.5 - |q - 0.5|) * 2 * sign(q - 0.5)`, which evaluates to
/// zero at BOTH lattice points and peaks a quarter-step off either one. That expression cannot
/// demodulate its own embedder. This is the distance-difference metric it was describing.
pub fn soft_bit(quotient: f64) -> f64 {
    if quotient <= 0.5 {
        2.0 * quotient - 0.5
    } else {
        1.5 - 2.0 * quotient
    }
}

/// Position of `value` within the dithered lattice period, in [0, 1).
pub fn lattice_quotient(value: f64, dither: f64, step: f64) -> f64 {
    let scaled = (value - dither) / step;
    let fractional = scaled - scaled.floor();
    if fractional.is_finite() {
        fractional.clamp(0.0, 1.0 - f64::EPSILON)
    } else {
        0.0
    }
}
