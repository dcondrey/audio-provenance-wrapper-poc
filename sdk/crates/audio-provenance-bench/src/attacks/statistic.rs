use super::geometry::{PairBandGeometry, PunctureRule};
use crate::error::{AudioError, ChannelError};
use audio_provenance_audio::window::{self, Symmetry};
use audio_provenance_audio::{AudioBuffer, Stft, StftFrames};
use rustfft::num_complex::Complex;
use serde::Serialize;
use std::fmt::Debug;

/// Floor added inside the logarithm, the same guard the statistic's own definition carries so a
/// silent cell produces a finite difference rather than an infinity.
const ENERGY_FLOOR: f64 = 1e-20;
const CLOSURE_GAIN_FLOOR: f64 = 0.15;
const CLOSURE_GAIN_CEILING: f64 = 1.2;
const CLOSURE_STEP_FLOOR: f64 = 0.02;

/// Passes of measure-apply-remeasure an attack is allowed.
///
/// IMPORTANT: a single-shot spectral multiply returns only part of the intended change to the next
/// analysis of the same slot, because the analysis window squares over the modification and the
/// neighbouring frames carry different shifts. An attack that applied one pass and booked the
/// INTENDED shift would land at roughly half strength and would report the mark as far more robust
/// than it is. Every attack here iterates to a measured target and reports what it achieved.
pub const DEFAULT_MAX_PASSES: usize = 6;

/// What the attacker wants each slot statistic to become. `None` leaves a slot alone.
pub trait SlotTargets: Debug + Send + Sync {
    fn name(&self) -> &str;
    fn plan(
        &self,
        measured: &[Option<f64>],
        geometry: &PairBandGeometry,
        seed: u64,
    ) -> Vec<Option<f64>>;
}

/// How far an attack actually moved the statistic, measured on the output rather than assumed from
/// the shift that was asked for. This is the self-check on every removal rate in the report.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct DriveReport {
    pub slots: usize,
    pub slots_targeted: usize,
    pub passes: usize,
    pub measured_closure_gain: f64,
    pub achieved_shift_median_nepers: Option<f64>,
    pub achieved_shift_p95_nepers: Option<f64>,
    pub residual_to_target_p95_nepers: Option<f64>,
}

fn sqrt_hann(len: usize) -> Vec<f32> {
    window::hann(len, Symmetry::Periodic)
        .into_iter()
        .map(|value| value.max(0.0).sqrt())
        .collect()
}

fn window_power(win: &[f32]) -> f64 {
    win.iter().map(|v| f64::from(*v) * f64::from(*v)).sum()
}

fn cell_energy(spectrum: &[Complex<f32>], from: usize, to: usize) -> f64 {
    spectrum
        .get(from..to)
        .map(|slice| slice.iter().map(|bin| f64::from(bin.norm_sqr())).sum())
        .unwrap_or(0.0)
}

fn trimmed_mean(values: &mut [f64], trim_each_tail: usize) -> Option<f64> {
    let kept = values.len().checked_sub(2 * trim_each_tail)?;
    if kept == 0 {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    Some(
        values[trim_each_tail..trim_each_tail + kept]
            .iter()
            .sum::<f64>()
            / kept as f64,
    )
}

pub fn percentile(values: &mut [f64], fraction: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let index = ((values.len() - 1) as f64 * fraction).round() as usize;
    values.get(index).copied()
}

fn stft_for(geometry: &PairBandGeometry) -> Result<(Stft, f64), ChannelError> {
    let win = sqrt_hann(geometry.frame());
    let power = window_power(&win);
    let stft = Stft::new(win, geometry.hop()).map_err(AudioError::Substrate)?;
    Ok((stft, power))
}

fn slot_statistics(
    spectra: &StftFrames,
    geometry: &PairBandGeometry,
    puncture: PunctureRule,
    window_power: f64,
) -> Vec<Option<f64>> {
    let slots = geometry.slot_of_frame_count(spectra.frames());
    let edges = geometry.edge_bins();
    let active = geometry.active_pairs();
    let band_bins = geometry.band_bins();
    let mut out = Vec::with_capacity(slots);
    for slot in 0..slots {
        let mut values = Vec::with_capacity(active.len() * geometry.frames_per_slot());
        let mut band_energy = 0.0f64;
        let mut usable = vec![true; active.len()];
        let mut complete = true;
        for offset in 0..geometry.frames_per_slot() {
            let Some(spectrum) = spectra.frame(slot * geometry.frames_per_slot() + offset) else {
                complete = false;
                break;
            };
            for (index, &pair) in active.iter().enumerate() {
                let low = edges[2 * pair];
                let middle = edges[2 * pair + 1];
                let high = edges[2 * pair + 2];
                let energy_a = cell_energy(spectrum, low, middle);
                let energy_b = cell_energy(spectrum, middle, high);
                values.push((energy_a + ENERGY_FLOOR).ln() - (energy_b + ENERGY_FLOOR).ln());
                band_energy += energy_a + energy_b;
                let scale_a = ((middle - low) as f64 * window_power).max(f64::MIN_POSITIVE);
                let scale_b = ((high - middle) as f64 * window_power).max(f64::MIN_POSITIVE);
                if energy_a / scale_a <= puncture.cell_power
                    || energy_b / scale_b <= puncture.cell_power
                {
                    usable[index] = false;
                }
            }
        }
        if !complete {
            out.push(None);
            continue;
        }
        let denominator = (band_bins as f64 * geometry.frames_per_slot() as f64 * window_power)
            .max(f64::MIN_POSITIVE);
        let punctured = band_energy / denominator < puncture.band_power
            || usable.iter().filter(|&&good| good).count() < puncture.min_pairs;
        let statistic = trimmed_mean(&mut values, geometry.trim_each_tail());
        out.push(if punctured { None } else { statistic });
    }
    out
}

/// The slot statistic of every slot in `audio`, computed from published geometry alone.
///
/// The mid signal is the channel average, and the transform is linear, so analysing the averaged
/// signal gives exactly the averaged spectra an embedder writing to the mid channel works in.
pub fn analyse(
    audio: &AudioBuffer,
    geometry: &PairBandGeometry,
    puncture: PunctureRule,
) -> Result<Vec<Option<f64>>, ChannelError> {
    let (stft, power) = stft_for(geometry)?;
    let mid = audio.mono_sum();
    let spectra = stft.forward(&mid).map_err(AudioError::Substrate)?;
    Ok(slot_statistics(&spectra, geometry, puncture, power))
}

fn apply_shifts(spectra: &mut [StftFrames], geometry: &PairBandGeometry, shifts: &[f64]) {
    let edges = geometry.edge_bins();
    for (slot, &shift) in shifts.iter().enumerate() {
        if shift == 0.0 || !shift.is_finite() {
            continue;
        }
        let up = (shift / 4.0).exp() as f32;
        let down = (-shift / 4.0).exp() as f32;
        for offset in 0..geometry.frames_per_slot() {
            let frame = slot * geometry.frames_per_slot() + offset;
            for channel in spectra.iter_mut() {
                let Some(spectrum) = channel.frame_mut(frame) else {
                    continue;
                };
                for &pair in geometry.active_pairs() {
                    for bin in edges[2 * pair]..edges[2 * pair + 1] {
                        if let Some(value) = spectrum.get_mut(bin) {
                            *value *= up;
                        }
                    }
                    for bin in edges[2 * pair + 1]..edges[2 * pair + 2] {
                        if let Some(value) = spectrum.get_mut(bin) {
                            *value *= down;
                        }
                    }
                }
            }
        }
    }
}

/// One fixed spectral tilt over the whole file, applied open loop.
///
/// Nothing is measured and nothing is iterated: the same cell-A-up, cell-B-down curve is applied to
/// every slot. That is a STATIC equaliser of the band's published shape, which is the cheapest form
/// an attack on this statistic can take, and the reason the guard mask and the 86 Hz pair
/// differencing do not stop it: they defeat a SMOOTH response, not one shaped like the spec.
pub fn apply_fixed_tilt(
    audio: &AudioBuffer,
    geometry: &PairBandGeometry,
    shift_nepers: f64,
) -> Result<AudioBuffer, ChannelError> {
    let (stft, _) = stft_for(geometry)?;
    let frames = audio.frames();
    let mut spectra = Vec::with_capacity(audio.channels());
    for channel in 0..audio.channels() {
        let plane = audio.channel(channel).ok_or(AudioError::Empty)?;
        spectra.push(stft.forward(plane).map_err(AudioError::Substrate)?);
    }
    let slots = geometry.slot_of_frame_count(spectra.first().map_or(0, StftFrames::frames));
    apply_shifts(&mut spectra, geometry, &vec![shift_nepers; slots]);
    let mut planes = Vec::with_capacity(spectra.len());
    for channel in &spectra {
        planes.push(
            stft.inverse(channel, frames)
                .map_err(AudioError::Substrate)?,
        );
    }
    crate::audio::from_channels(audio.sample_rate(), &planes).map_err(ChannelError::from)
}

/// The shift a one-shot operation actually achieved, measured between two analyses rather than
/// taken from what was asked for.
pub fn measured_shift(before: &[Option<f64>], after: &[Option<f64>], passes: usize) -> DriveReport {
    let mut achieved: Vec<f64> = before
        .iter()
        .zip(after.iter())
        .filter_map(|(first, second)| Some(((*second)? - (*first)?).abs()))
        .collect();
    DriveReport {
        slots: before.len(),
        slots_targeted: achieved.len(),
        passes,
        measured_closure_gain: f64::NAN,
        achieved_shift_median_nepers: percentile(&mut achieved, 0.5),
        achieved_shift_p95_nepers: percentile(&mut achieved, 0.95),
        residual_to_target_p95_nepers: None,
    }
}

fn closure_gain(previous: &[Option<f64>], current: &[Option<f64>], step: &[f64]) -> Option<f64> {
    let mut ratios: Vec<f64> = previous
        .iter()
        .zip(current.iter())
        .zip(step.iter())
        .filter_map(|((before, after), applied)| {
            let (before, after) = ((*before)?, (*after)?);
            (applied.abs() > CLOSURE_STEP_FLOOR).then(|| (after - before) / applied)
        })
        .filter(|ratio| ratio.is_finite())
        .collect();
    percentile(&mut ratios, 0.5).map(|value| value.clamp(CLOSURE_GAIN_FLOOR, CLOSURE_GAIN_CEILING))
}

/// Moves every slot statistic of `audio` to what `targets` asks for, iterating until the measured
/// statistic reaches the target rather than until a nominal shift has been applied.
pub fn drive(
    audio: &AudioBuffer,
    geometry: &PairBandGeometry,
    puncture: PunctureRule,
    targets: &dyn SlotTargets,
    seed: u64,
    max_passes: usize,
    convergence_nepers: f64,
) -> Result<(AudioBuffer, DriveReport), ChannelError> {
    let (stft, power) = stft_for(geometry)?;
    let frames = audio.frames();
    let sample_rate = audio.sample_rate();
    let mut signal = audio.clone();
    let mut plan: Vec<Option<f64>> = Vec::new();
    let mut first: Vec<Option<f64>> = Vec::new();
    let mut previous: Vec<Option<f64>> = Vec::new();
    let mut previous_step: Vec<f64> = Vec::new();
    let mut gain = 0.45f64;
    let mut passes = 0usize;
    let mut measured: Vec<Option<f64>> = Vec::new();

    for pass in 0..max_passes.max(1) {
        passes = pass + 1;
        let mut spectra = Vec::with_capacity(signal.channels());
        for channel in 0..signal.channels() {
            let plane = signal.channel(channel).ok_or(AudioError::Empty)?;
            spectra.push(stft.forward(plane).map_err(AudioError::Substrate)?);
        }
        let mid = stft
            .forward(&signal.mono_sum())
            .map_err(AudioError::Substrate)?;
        measured = slot_statistics(&mid, geometry, puncture, power);

        if pass == 0 {
            first = measured.clone();
            plan = targets.plan(&measured, geometry, seed);
        } else if let Some(estimate) = closure_gain(&previous, &measured, &previous_step) {
            gain = estimate;
        }

        let residual: Vec<f64> = plan
            .iter()
            .zip(measured.iter())
            .map(|(target, value)| match (target, value) {
                (Some(target), Some(value)) => target - value,
                _ => 0.0,
            })
            .collect();
        let mut magnitudes: Vec<f64> = residual
            .iter()
            .zip(plan.iter())
            .filter(|(_, target)| target.is_some())
            .map(|(value, _)| value.abs())
            .collect();
        let residual_p95 = percentile(&mut magnitudes, 0.95);

        let converged = pass > 0 && residual_p95.is_some_and(|value| value <= convergence_nepers);
        if converged || pass + 1 == max_passes.max(1) {
            break;
        }

        let step: Vec<f64> = residual.iter().map(|value| value / gain).collect();
        apply_shifts(&mut spectra, geometry, &step);
        let mut planes = Vec::with_capacity(spectra.len());
        for channel in &spectra {
            planes.push(
                stft.inverse(channel, frames)
                    .map_err(AudioError::Substrate)?,
            );
        }
        signal = crate::audio::from_channels(sample_rate, &planes)?;
        previous = measured.clone();
        previous_step = step;
    }

    let mut achieved: Vec<f64> = first
        .iter()
        .zip(measured.iter())
        .filter_map(|(before, after)| Some(((*after)? - (*before)?).abs()))
        .collect();
    let mut residual: Vec<f64> = plan
        .iter()
        .zip(measured.iter())
        .filter_map(|(target, value)| Some(((*target)? - (*value)?).abs()))
        .collect();
    let report = DriveReport {
        slots: first.len(),
        slots_targeted: plan.iter().filter(|target| target.is_some()).count(),
        passes,
        measured_closure_gain: gain,
        achieved_shift_median_nepers: percentile(&mut achieved, 0.5),
        achieved_shift_p95_nepers: percentile(&mut achieved, 0.95),
        residual_to_target_p95_nepers: percentile(&mut residual, 0.95),
    };
    Ok((signal, report))
}
