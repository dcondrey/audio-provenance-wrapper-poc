use audio_provenance_audio::{AudioBuffer, Stft, StftFrames};
use rustfft::num_complex::Complex;

use crate::analysis::{active_bin_count, slot_measurement, sqrt_hann, window_power};
use crate::block::slot_bit;
use crate::convolutional::encode;
use crate::error::WatermarkError;
use crate::geometry::Band;
use crate::interleaver::interleave;
use crate::keystream::Schedule;
use crate::params::{BLOCK_SLOTS, BLOCKS_PER_EPOCH, DELTA, FRAMES_PER_SLOT, OLA_RESIDUAL_LIMIT};
use crate::payload::Payload;
use crate::statistic::median;
use crate::validate::admit;

/// Weighted overlap-add returns only part of a per-frame spectral change to the next analysis of
/// the same frame, because the analysis window squares over the modification and the neighbouring
/// frames carry different shifts. Measured over the corpus the fraction sits near 0.45, so a
/// correction pass that assumed 1.0 would converge at 0.55 per pass and never reach the residual
/// the spec asserts. The fraction is measured per file from the first correction and reused.
const CLOSURE_GAIN_FLOOR: f64 = 0.15;
const CLOSURE_GAIN_CEILING: f64 = 1.2;
const CLOSURE_STEP_FLOOR: f64 = 0.02;
const MAX_PASSES: usize = 5;

/// What the embedder changed, so a degraded embed is recorded rather than hidden.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmbedReport {
    pub slots: usize,
    pub blocks: usize,
    pub punctured_slots: usize,
    pub passes: usize,
    /// Largest cumulative per-slot shift applied to the analysis spectra.
    pub max_applied_shift_nepers: f64,
    /// Largest lattice shift the QIM asked for. Bounded by DELTA/2 by construction.
    pub max_intended_shift_nepers: f64,
    /// Per-bin amplitude change measured between the input and the output spectra inside the band.
    /// `None` unless closure was measured.
    pub bin_gain: Option<BinGain>,
    /// 95th percentile of `|target - d|` measured on the final signal.
    pub closure_residual_nepers: Option<f64>,
    pub closure_residual_max_nepers: Option<f64>,
    pub measured_closure_gain: f64,
}

impl EmbedReport {
    pub fn meets_closure_budget(&self) -> bool {
        self.closure_residual_nepers
            .is_some_and(|residual| residual <= OLA_RESIDUAL_LIMIT)
    }
}

/// Distribution of |20 log10| of the per-bin amplitude ratio between input and output, over every
/// in-band bin of every frame whose original power clears the analysis floor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinGain {
    pub median_db: f64,
    pub p95_db: f64,
    pub max_db: f64,
    /// The same deviations weighted by the original bin power, which is the figure that tracks what
    /// a listener could hear rather than what a near-null bin did in ratio terms.
    pub energy_weighted_rms_db: f64,
}

struct Plan {
    stft: Stft,
    edges: Vec<usize>,
    active: Vec<usize>,
    window_power: f64,
    band_bins: usize,
    frames: usize,
    slots: usize,
    bins: usize,
}

impl Plan {
    fn new(band: &Band, signal_frames: usize) -> Result<Self, WatermarkError> {
        let window = sqrt_hann(band.frame());
        let power = window_power(&window);
        let stft = Stft::new(window, band.hop())?;
        let frames = stft.frame_count(signal_frames);
        let edges = band.edges(1.0)?;
        let active = band.active_pairs().to_vec();
        let band_bins = active_bin_count(&edges, &active);
        Ok(Self {
            stft,
            edges,
            active,
            window_power: power,
            band_bins,
            frames,
            slots: frames / FRAMES_PER_SLOT,
            bins: band.bins(),
        })
    }

    fn statistics(&self, mid: &[Vec<Complex<f32>>]) -> Vec<Option<f64>> {
        (0..self.slots)
            .map(|slot| {
                slot_measurement(
                    |frame| mid.get(frame).map(Vec::as_slice),
                    slot * FRAMES_PER_SLOT,
                    &self.edges,
                    &self.active,
                    self.window_power,
                    self.band_bins,
                )
                .filter(|measurement| !measurement.punctured)
                .map(|measurement| measurement.statistic)
            })
            .collect()
    }
}

fn mid_spectra(spectra: &[StftFrames], frames: usize, bins: usize) -> Vec<Vec<Complex<f32>>> {
    let scale = 1.0f32 / spectra.len() as f32;
    (0..frames)
        .map(|frame| {
            let mut accumulator = vec![Complex::new(0.0f32, 0.0); bins];
            for channel in spectra {
                if let Some(source) = channel.frame(frame) {
                    for (slot, value) in accumulator.iter_mut().zip(source.iter()) {
                        *slot += *value;
                    }
                }
            }
            for value in &mut accumulator {
                *value *= scale;
            }
            accumulator
        })
        .collect()
}

fn apply_shifts(spectra: &mut [StftFrames], plan: &Plan, shifts: &[f64]) {
    for (slot, &shift) in shifts.iter().enumerate() {
        if shift == 0.0 || !shift.is_finite() {
            continue;
        }
        let up = (shift / 4.0).exp() as f32;
        let down = (-shift / 4.0).exp() as f32;
        for offset in 0..FRAMES_PER_SLOT {
            let frame = slot * FRAMES_PER_SLOT + offset;
            for channel in spectra.iter_mut() {
                let Some(spectrum) = channel.frame_mut(frame) else {
                    continue;
                };
                for &pair in &plan.active {
                    for bin in plan.edges[2 * pair]..plan.edges[2 * pair + 1] {
                        if let Some(value) = spectrum.get_mut(bin) {
                            *value *= up;
                        }
                    }
                    for bin in plan.edges[2 * pair + 1]..plan.edges[2 * pair + 2] {
                        if let Some(value) = spectrum.get_mut(bin) {
                            *value *= down;
                        }
                    }
                }
            }
        }
    }
}

fn forward_all(stft: &Stft, audio: &AudioBuffer) -> Result<Vec<StftFrames>, WatermarkError> {
    (0..audio.channels())
        .map(|channel| {
            let plane = audio.channel(channel).ok_or(WatermarkError::Empty)?;
            stft.forward(plane).map_err(WatermarkError::from)
        })
        .collect()
}

fn inverse_all(
    stft: &Stft,
    spectra: &[StftFrames],
    sample_rate: u32,
    frames: usize,
) -> Result<AudioBuffer, WatermarkError> {
    let mut planes = Vec::with_capacity(spectra.len());
    for channel in spectra {
        planes.push(stft.inverse(channel, frames)?);
    }
    AudioBuffer::from_channels(sample_rate, &planes).map_err(WatermarkError::from)
}

/// Largest per-bin amplitude change inside the band, measured between the two signals rather than
/// inferred from the shift the embedder asked for.
fn band_gain_db(
    plan: &Plan,
    before: &AudioBuffer,
    after: &AudioBuffer,
) -> Result<BinGain, WatermarkError> {
    let original = forward_all(&plan.stft, before)?;
    let marked = forward_all(&plan.stft, after)?;
    let floor = plan.window_power * 1e-10;
    let mut deviations: Vec<f64> = Vec::new();
    let mut weighted = 0.0f64;
    let mut weight = 0.0f64;
    for (left, right) in original.iter().zip(marked.iter()) {
        for frame in 0..plan.frames {
            let (Some(a), Some(b)) = (left.frame(frame), right.frame(frame)) else {
                continue;
            };
            for &pair in &plan.active {
                for bin in plan.edges[2 * pair]..plan.edges[2 * pair + 2] {
                    let (Some(x), Some(y)) = (a.get(bin), b.get(bin)) else {
                        continue;
                    };
                    let power = f64::from(x.norm_sqr());
                    if power < floor {
                        continue;
                    }
                    let deviation = (10.0 * (f64::from(y.norm_sqr()) / power).log10()).abs();
                    weighted += power * deviation * deviation;
                    weight += power;
                    deviations.push(deviation);
                }
            }
        }
    }
    Ok(BinGain {
        median_db: percentile(&mut deviations, 0.5).unwrap_or(0.0),
        p95_db: percentile(&mut deviations, 0.95).unwrap_or(0.0),
        max_db: deviations.last().copied().unwrap_or(0.0),
        energy_weighted_rms_db: if weight > 0.0 {
            (weighted / weight).sqrt()
        } else {
            0.0
        },
    })
}

fn percentile(values: &mut [f64], fraction: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let index = ((values.len() - 1) as f64 * fraction).round() as usize;
    values.get(index).copied()
}

/// Fraction of an applied shift that reappears in the next analysis of the same slot.
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
    median(&mut ratios).map(|value| value.clamp(CLOSURE_GAIN_FLOOR, CLOSURE_GAIN_CEILING))
}

pub(crate) fn embed_into(
    profile_key: &[u8],
    namespace: u8,
    audio: &AudioBuffer,
    payload: Payload,
    measure_closure: bool,
) -> Result<(AudioBuffer, EmbedReport), WatermarkError> {
    let source = admit(audio)?;
    let band = Band::new(source.sample_rate())?;
    let plan = Plan::new(&band, source.frames())?;
    if plan.slots < BLOCK_SLOTS {
        return Err(WatermarkError::TooShort {
            frames: source.frames(),
            needed: BLOCK_SLOTS * FRAMES_PER_SLOT * band.hop(),
        });
    }

    let interleaved = interleave(&encode(&payload.to_message()));
    let epochs = plan.slots / (BLOCK_SLOTS * BLOCKS_PER_EPOCH) + 1;
    let mut schedules = Vec::with_capacity(epochs);
    for epoch in 0..epochs {
        schedules.push(Schedule::derive(profile_key, namespace, epoch as u64)?);
    }

    let mut signal = source.clone();
    let mut targets: Vec<Option<f64>> = Vec::new();
    let mut cumulative = vec![0.0f64; plan.slots];
    let mut previous: Vec<Option<f64>> = Vec::new();
    let mut first_pass: Vec<Option<f64>> = Vec::new();
    let mut previous_step: Vec<f64> = Vec::new();
    let mut gain = 1.0f64;
    let mut punctured = 0usize;
    let mut residual_p95 = None;
    let mut residual_max = None;
    let mut passes = 0usize;

    for pass in 0..MAX_PASSES {
        passes = pass + 1;
        let mut spectra = forward_all(&plan.stft, &signal)?;
        let mid = mid_spectra(&spectra, plan.frames, plan.bins);
        let measured = plan.statistics(&mid);

        if pass == 0 {
            punctured = measured.iter().filter(|value| value.is_none()).count();
            first_pass = measured.clone();
            targets = measured
                .iter()
                .enumerate()
                .map(|(slot, value)| {
                    let statistic = (*value)?;
                    let block = slot / BLOCK_SLOTS;
                    let schedule =
                        schedules.get((block / BLOCKS_PER_EPOCH).min(schedules.len() - 1))?;
                    let bit = slot_bit(schedule, &interleaved, slot)?;
                    let dither = f64::from(bit) * DELTA / 2.0 + schedule.dither(slot % BLOCK_SLOTS);
                    Some(DELTA * ((statistic - dither) / DELTA).round() + dither)
                })
                .collect();
        } else if let Some(estimate) = closure_gain(&previous, &measured, &previous_step) {
            gain = estimate;
        }

        let residual: Vec<f64> = targets
            .iter()
            .zip(measured.iter())
            .map(|(target, value)| match (target, value) {
                (Some(target), Some(value)) => target - value,
                _ => 0.0,
            })
            .collect();
        let mut magnitudes: Vec<f64> = residual
            .iter()
            .zip(targets.iter())
            .filter(|(_, target)| target.is_some())
            .map(|(value, _)| value.abs())
            .collect();
        residual_p95 = percentile(&mut magnitudes, 0.95);
        residual_max = magnitudes.last().copied();

        let converged = pass > 0 && residual_p95.is_some_and(|value| value <= OLA_RESIDUAL_LIMIT);
        if converged || pass + 1 == MAX_PASSES {
            if !measure_closure {
                residual_p95 = None;
                residual_max = None;
            }
            break;
        }

        let step: Vec<f64> = residual.iter().map(|value| value / gain).collect();
        apply_shifts(&mut spectra, &plan, &step);
        for (total, applied) in cumulative.iter_mut().zip(step.iter()) {
            *total += applied;
        }
        signal = inverse_all(&plan.stft, &spectra, source.sample_rate(), source.frames())?;
        previous = measured;
        previous_step = step;
    }

    let max_applied = cumulative
        .iter()
        .fold(0.0f64, |worst, value| worst.max(value.abs()));
    let max_intended = targets
        .iter()
        .zip(first_pass.iter())
        .filter_map(|(target, before)| Some(((*target)? - (*before)?).abs()))
        .fold(0.0f64, f64::max);
    let bin_gain = if measure_closure {
        Some(band_gain_db(&plan, &source, &signal)?)
    } else {
        None
    };

    let report = EmbedReport {
        slots: plan.slots,
        blocks: plan.slots / BLOCK_SLOTS,
        punctured_slots: punctured,
        passes,
        max_applied_shift_nepers: max_applied,
        max_intended_shift_nepers: max_intended,
        bin_gain,
        closure_residual_nepers: residual_p95,
        closure_residual_max_nepers: residual_max,
        measured_closure_gain: gain,
    };
    Ok((signal, report))
}
