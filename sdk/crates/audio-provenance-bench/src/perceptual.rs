use crate::error::AudioError;
use audio_provenance_audio::AudioBuffer;
use audio_provenance_audio::fft::RealFft;
use serde::Serialize;

pub const FRAME: usize = 2048;
pub const HOP: usize = 1024;
const SEG_FRAME: usize = 1024;
const SILENCE_FLOOR_DBFS: f64 = -80.0;
const SEG_SNR_MIN_DB: f64 = -20.0;
// IMPORTANT: the speech-coding convention clamps segmental SNR at 35 dB. A watermark residual sits
// 60 to 100 dB down, so that ceiling would flatten every item to the same number and hide exactly
// what this measurement is for.
const SEG_SNR_MAX_DB: f64 = 100.0;
const FULL_SCALE_SPL_DB: f64 = 96.0;
const TONALITY_ALPHA: f64 = 0.5;

pub const PERCEPTUAL_LIMITS: &str = "WHAT THESE NUMBERS ARE. Segmental SNR is an energy ratio per \
short frame; it says how much smaller the embedding residual is than the signal, and nothing about \
whether a listener can hear it. The masking measure is a noise-to-mask ratio computed from a \
Schroeder spreading function over a half-Bark partition, a fixed tonality assumption (alpha = 0.5) \
rather than a measured one, and an absolute-threshold curve anchored by assuming digital full scale \
is 96 dB SPL. Band energies are normalised into per-sample power so that floor is comparable at all; \
the divisor is exact for noise-like content and understates a tonal masker by roughly 5 dB, which \
errs toward calling a residual audible rather than inaudible. Both figures are measured per channel \
and reported for the WORST channel, never for a mixdown, which would halve a residual living in one \
channel. WHAT THEY ARE NOT. Neither is PEAQ (ITU-R BS.1387), neither yields an ODG, and \
neither is a listening test. A negative NMR means the residual sits below this model's threshold in \
this model's terms; it is evidence for inaudibility, not a measurement of it. Only a blinded ABX \
panel can retire that gap.";

#[derive(Debug, Clone, Serialize)]
pub struct PerceptualMeasurement {
    pub segmental_snr_db: Option<f64>,
    pub segments_scored: usize,
    pub segments_skipped_silent: usize,
    pub noise_to_mask_mean_db: Option<f64>,
    pub noise_to_mask_max_db: Option<f64>,
    pub frames_above_mask_fraction: Option<f64>,
    pub peak_residual_dbfs: Option<f64>,
    pub channels_measured: usize,
    pub limits: &'static str,
}

impl PerceptualMeasurement {
    pub const fn empty() -> Self {
        Self {
            segmental_snr_db: None,
            segments_scored: 0,
            segments_skipped_silent: 0,
            noise_to_mask_mean_db: None,
            noise_to_mask_max_db: None,
            frames_above_mask_fraction: None,
            peak_residual_dbfs: None,
            channels_measured: 0,
            limits: PERCEPTUAL_LIMITS,
        }
    }
}

fn hann(size: usize) -> Vec<f64> {
    (0..size)
        .map(|n| 0.5 - 0.5 * (core::f64::consts::TAU * n as f64 / size as f64).cos())
        .collect()
}

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
struct BarkPartition {
    band_of_bin: Vec<usize>,
    centres: Vec<f64>,
    thresholds_in_quiet: Vec<f64>,
}

impl BarkPartition {
    fn new(sample_rate: u32, bins: usize) -> Self {
        let nyquist = f64::from(sample_rate) / 2.0;
        let mut band_of_bin = Vec::with_capacity(bins);
        let mut edges: Vec<f64> = Vec::new();
        for k in 0..bins {
            let frequency = nyquist * k as f64 / (bins - 1).max(1) as f64;
            let z = bark(frequency);
            let band = (z / 0.5).floor().max(0.0) as usize;
            band_of_bin.push(band);
            while edges.len() <= band {
                edges.push(edges.len() as f64 * 0.5 + 0.25);
            }
        }
        let count = edges.len().max(1);
        let centres: Vec<f64> = (0..count).map(|b| b as f64 * 0.5 + 0.25).collect();
        // Anchor the threshold in quiet on the assumption that digital full scale is 96 dB SPL, the
        // conventional 16-bit anchor. The assumption is stated in `PERCEPTUAL_LIMITS` because it
        // moves every absolute number here.
        let mut thresholds_in_quiet = vec![f64::INFINITY; count];
        for (k, &band) in band_of_bin.iter().enumerate() {
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
            centres,
            thresholds_in_quiet,
        }
    }

    fn bands(&self) -> usize {
        self.centres.len()
    }

    fn accumulate(&self, spectrum: &[f64]) -> Vec<f64> {
        let mut bands = vec![0.0; self.bands()];
        for (k, &power) in spectrum.iter().enumerate() {
            if let Some(&band) = self.band_of_bin.get(k)
                && let Some(slot) = bands.get_mut(band)
            {
                *slot += power;
            }
        }
        bands
    }

    fn masking_threshold(&self, reference_bands: &[f64]) -> Vec<f64> {
        let mut threshold = vec![0.0; self.bands()];
        for (b, slot) in threshold.iter_mut().enumerate() {
            let zb = self.centres[b];
            let mut spread = 0.0;
            for (j, &energy) in reference_bands.iter().enumerate() {
                if energy <= 0.0 {
                    continue;
                }
                spread += energy * 10f64.powf(spreading_db(zb - self.centres[j]) / 10.0);
            }
            let offset_db = TONALITY_ALPHA * (14.5 + zb) + (1.0 - TONALITY_ALPHA) * 5.5;
            let masked = spread / 10f64.powf(offset_db / 10.0);
            *slot = masked.max(self.thresholds_in_quiet[b]);
        }
        threshold
    }
}

fn normalise(mut bands: Vec<f64>, divisor: f64) -> Vec<f64> {
    if divisor > 0.0 {
        for band in &mut bands {
            *band /= divisor;
        }
    }
    bands
}

fn segmental_snr(reference: &[f32], residual: &[f32]) -> (Option<f64>, usize, usize) {
    if reference.len() != residual.len() || reference.len() < SEG_FRAME {
        return (None, 0, 0);
    }
    let floor = 10f64.powf(SILENCE_FLOOR_DBFS / 10.0);
    let mut total = 0.0;
    let mut scored = 0usize;
    let mut skipped = 0usize;
    for start in (0..=reference.len() - SEG_FRAME).step_by(SEG_FRAME) {
        let signal: f64 = reference[start..start + SEG_FRAME]
            .iter()
            .map(|&s| f64::from(s) * f64::from(s))
            .sum::<f64>()
            / SEG_FRAME as f64;
        if signal < floor {
            skipped += 1;
            continue;
        }
        let noise: f64 = residual[start..start + SEG_FRAME]
            .iter()
            .map(|&s| f64::from(s) * f64::from(s))
            .sum::<f64>()
            / SEG_FRAME as f64;
        let ratio = 10.0 * (signal / noise.max(1e-20)).log10();
        total += ratio.clamp(SEG_SNR_MIN_DB, SEG_SNR_MAX_DB);
        scored += 1;
    }
    if scored == 0 {
        return (None, 0, skipped);
    }
    (Some(total / scored as f64), scored, skipped)
}

fn measure_channel(reference: &[f32], test: &[f32], sample_rate: u32) -> PerceptualMeasurement {
    let residual: Vec<f32> = reference
        .iter()
        .zip(test.iter())
        .map(|(a, b)| b - a)
        .collect();
    let peak_residual = residual.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let (seg_snr, scored, skipped) = segmental_snr(reference, &residual);

    let mut measurement = PerceptualMeasurement {
        segmental_snr_db: seg_snr,
        segments_scored: scored,
        segments_skipped_silent: skipped,
        peak_residual_dbfs: if peak_residual > 0.0 {
            Some(20.0 * f64::from(peak_residual).log10())
        } else {
            None
        },
        ..PerceptualMeasurement::empty()
    };

    let Ok(fft) = RealFft::<f64>::new(FRAME) else {
        return measurement;
    };
    if reference.len() < FRAME {
        return measurement;
    }
    let window = hann(FRAME);
    // IMPORTANT: the band energies must land in the same per-sample power units as the threshold in
    // quiet, or the absolute-threshold floor never binds and a residual far below audibility still
    // reports a positive noise-to-mask ratio. The divisor is exact for noise-like content, which is
    // what an embedding residual is; for a tonal masker it understates the masker by about 5 dB,
    // which errs toward reporting the residual as more audible than it is.
    let window_power: f64 = window.iter().map(|w| w * w).sum::<f64>() * FRAME as f64;
    let partition = BarkPartition::new(sample_rate, FRAME / 2 + 1);
    let floor = 10f64.powf(SILENCE_FLOOR_DBFS / 10.0);
    let mut nmr_total = 0.0;
    let mut nmr_max = f64::NEG_INFINITY;
    let mut frames = 0usize;
    let mut above = 0usize;

    for start in (0..=reference.len() - FRAME).step_by(HOP) {
        let energy: f64 = reference[start..start + FRAME]
            .iter()
            .map(|&s| f64::from(s) * f64::from(s))
            .sum::<f64>()
            / FRAME as f64;
        if energy < floor {
            continue;
        }
        let ref_frame: Vec<f64> = reference[start..start + FRAME]
            .iter()
            .zip(window.iter())
            .map(|(&s, &w)| f64::from(s) * w)
            .collect();
        let err_frame: Vec<f64> = residual[start..start + FRAME]
            .iter()
            .zip(window.iter())
            .map(|(&s, &w)| f64::from(s) * w)
            .collect();
        let (Ok(ref_power), Ok(err_power)) = (
            fft.power_spectrum(&ref_frame),
            fft.power_spectrum(&err_frame),
        ) else {
            continue;
        };
        let ref_bands = normalise(partition.accumulate(&ref_power), window_power);
        let err_bands = normalise(partition.accumulate(&err_power), window_power);
        let threshold = partition.masking_threshold(&ref_bands);
        let mut ratio_sum = 0.0;
        for (b, &err) in err_bands.iter().enumerate() {
            ratio_sum += err / threshold[b].max(1e-20);
        }
        let nmr_db = 10.0 * (ratio_sum / partition.bands() as f64).max(1e-20).log10();
        nmr_total += nmr_db;
        nmr_max = nmr_max.max(nmr_db);
        if nmr_db > 0.0 {
            above += 1;
        }
        frames += 1;
    }

    if frames > 0 {
        measurement.noise_to_mask_mean_db = Some(nmr_total / frames as f64);
        measurement.noise_to_mask_max_db = Some(nmr_max);
        measurement.frames_above_mask_fraction = Some(above as f64 / frames as f64);
    }
    measurement
}

fn worse_min(current: Option<f64>, candidate: Option<f64>) -> Option<f64> {
    match (current, candidate) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

fn worse_max(current: Option<f64>, candidate: Option<f64>) -> Option<f64> {
    match (current, candidate) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Compares an original against its watermarked copy. Both must have the same rate, channel count
/// and length; an embedder that changes any of them cannot be measured this way and says so.
///
/// IMPORTANT: measured per channel and reported for the WORST one. A mixdown halves a residual that
/// lives in a single channel, which is exactly what a channel-asymmetric embedder produces, so a
/// mono-summed transparency figure flatters precisely the design most likely to be audible.
pub fn measure(
    original: &AudioBuffer,
    marked: &AudioBuffer,
) -> Result<PerceptualMeasurement, AudioError> {
    for (what, left, right) in [
        (
            "sample rate",
            u64::from(original.sample_rate()),
            u64::from(marked.sample_rate()),
        ),
        (
            "channel count",
            original.channels() as u64,
            marked.channels() as u64,
        ),
        (
            "frame count",
            original.frames() as u64,
            marked.frames() as u64,
        ),
    ] {
        if left != right {
            return Err(AudioError::Mismatch {
                what,
                original: left,
                marked: right,
            });
        }
    }

    let mut worst = PerceptualMeasurement::empty();
    let mut channels_measured = 0usize;

    for index in 0..original.channels() {
        let (Some(plane), Some(other)) = (original.channel(index), marked.channel(index)) else {
            continue;
        };
        let channel = measure_channel(plane, other, original.sample_rate());
        worst.segmental_snr_db = worse_min(worst.segmental_snr_db, channel.segmental_snr_db);
        worst.noise_to_mask_mean_db =
            worse_max(worst.noise_to_mask_mean_db, channel.noise_to_mask_mean_db);
        worst.noise_to_mask_max_db =
            worse_max(worst.noise_to_mask_max_db, channel.noise_to_mask_max_db);
        worst.frames_above_mask_fraction = worse_max(
            worst.frames_above_mask_fraction,
            channel.frames_above_mask_fraction,
        );
        worst.peak_residual_dbfs = worse_max(worst.peak_residual_dbfs, channel.peak_residual_dbfs);
        worst.segments_scored += channel.segments_scored;
        worst.segments_skipped_silent += channel.segments_skipped_silent;
        channels_measured += 1;
    }
    worst.channels_measured = channels_measured;
    Ok(worst)
}
