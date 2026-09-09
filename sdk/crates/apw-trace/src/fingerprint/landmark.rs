//! Landmark/constellation hashing over the spectrogram.
//!
//! Chroma and HPCP are rejected on purpose: they match compositions, so a cover version or a
//! different master would return the original's manifest. That is an identity error dressed as
//! robustness.
//!
//! Every parameter below is fixed by TRACE_SPEC. The one documented deviation is the
//! resampler: the spec names a 64-tap Kaiser (beta 8.6), and this uses `audio_provenance_audio::resample`
//! instead. What matching needs is that the index build and the query agree, which one shared
//! implementation gives; a second polyphase resampler alongside the crate's own would be a parallel
//! DSP abstraction with no benefit.

use std::collections::VecDeque;

use audio_provenance_audio::{AudioBuffer, Stft, Symmetry, resample, window::hann};

use crate::error::TraceError;

/// The fingerprint algorithm id, as it appears in a manifest's `audio_fingerprint.algorithm`.
pub const ALGORITHM_ID: &str = "apw-trace-landmark-v1";

pub const SCHEME_VERSION: u8 = 1;

pub const ANALYSIS_RATE: u32 = 11_025;
pub const WINDOW_LEN: usize = 1024;
pub const HOP: usize = 128;
/// 86.1328125 frames per second.
pub const FRAMES_PER_SECOND: f64 = ANALYSIS_RATE as f64 / HOP as f64;
/// 10.7666015625 Hz per bin.
pub const HZ_PER_BIN: f64 = ANALYSIS_RATE as f64 / WINDOW_LEN as f64;

/// Hashing band: bins 3..430, which is 32.3 Hz to 4.63 kHz.
pub const BAND_LOW_BIN: usize = 3;
pub const BAND_HIGH_BIN: usize = 430;

const NEIGHBOURHOOD_BINS: usize = 11;
const NEIGHBOURHOOD_FRAMES: usize = 5;

/// Six log-spaced adaptive-floor bands across the hashing band.
const FLOOR_BANDS: usize = 6;
/// The adaptive floor decays by `exp(-1/30)` per frame in the energy domain. The spectrogram
/// carries log10 AMPLITUDE, so the equivalent step is `log10(exp(-1/30)) / 2`, subtracted rather
/// than multiplied: a log value can be negative, and scaling a negative floor would raise it.
const FLOOR_DECAY_LOG10_AMPLITUDE: f64 = 0.007_238_142_524_240_1;

/// +6 dB expressed in log10 amplitude, which is what the spectrogram carries.
const SIX_DB_LOG10: f64 = 6.0 / 20.0;

pub const TARGET_PEAK_DENSITY: f64 = 28.0;
pub const PEAK_DENSITY_TOLERANCE: f64 = 6.0;
const DENSITY_BISECTIONS: usize = 3;
const GLOBAL_FLOOR_LOW: f64 = -1.0;
const GLOBAL_FLOOR_HIGH: f64 = 2.0;

pub const MIN_DT_FRAMES: u32 = 2;
pub const MAX_DT_FRAMES: u32 = 63;
pub const MAX_DF_BINS: i32 = 63;
pub const FAN_OUT: usize = 8;

/// Query-side frequency smear: each query hash is emitted three times with the target bin nudged.
pub const QUERY_BIN_SMEAR: [i32; 3] = [-1, 0, 1];

/// Playback-rate hypotheses, tried only when the 1.00 pass fails.
pub const RATE_SWEEP: [f64; 4] = [0.98, 0.99, 1.01, 1.02];

/// Guards against a hostile or pathological input producing an unbounded peak list.
const MAX_PEAKS: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Peak {
    pub frame: u32,
    pub bin: u16,
}

/// The peaks of one recording plus the frame count they came from.
#[derive(Debug, Clone)]
pub struct Constellation {
    peaks: Vec<Peak>,
    frames: usize,
}

impl Constellation {
    /// Rebuilds a constellation from a decoded reference. The caller owns the ordering and range
    /// invariants; `reference::decode` is the only caller and enforces both before it gets here.
    pub const fn from_parts(peaks: Vec<Peak>, frames: usize) -> Self {
        Self { peaks, frames }
    }

    pub fn peaks(&self) -> &[Peak] {
        &self.peaks
    }

    pub const fn frames(&self) -> usize {
        self.frames
    }

    pub fn duration_seconds(&self) -> f64 {
        self.frames as f64 / FRAMES_PER_SECOND
    }

    pub fn density_per_second(&self) -> f64 {
        let seconds = self.duration_seconds();
        if seconds <= 0.0 {
            return 0.0;
        }
        self.peaks.len() as f64 / seconds
    }
}

/// A landmark pair reduced to its index key and anchor time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Landmark {
    pub hash: u32,
    pub t1: u32,
}

/// `f1(9) | f2(9) | dt(6) | schemeVersion(8)`, scheme in the low byte so the high 24 bits are
/// exactly the discriminative part the offset table indexes.
pub const fn pack_hash(f1: u16, f2: u16, dt: u32) -> u32 {
    ((f1 as u32 & 0x1FF) << 23)
        | ((f2 as u32 & 0x1FF) << 14)
        | ((dt & 0x3F) << 8)
        | SCHEME_VERSION as u32
}

pub const fn hash_prefix(hash: u32) -> u32 {
    hash >> 8
}

/// Mono sum, resample to 11025 Hz, 1-pole 20 Hz high-pass. No gain normalization: landmark
/// selection is amplitude-invariant by construction, and normalizing would only couple the peak
/// set to whatever else is in the file.
pub fn preprocess(audio: &AudioBuffer) -> Result<Vec<f32>, TraceError> {
    let mono = audio.mono_sum();
    if mono.is_empty() {
        return Ok(Vec::new());
    }
    let buffer = AudioBuffer::from_channels(audio.sample_rate(), &[mono]).map_err(|error| {
        TraceError::Undecodable {
            reason: error.to_string(),
        }
    })?;
    let resampled = if audio.sample_rate() == ANALYSIS_RATE {
        buffer
    } else {
        resample(&buffer, ANALYSIS_RATE).map_err(|error| TraceError::Undecodable {
            reason: error.to_string(),
        })?
    };
    let mut samples = resampled.channel(0).unwrap_or(&[]).to_vec();
    high_pass_20hz(&mut samples);
    Ok(samples)
}

/// One pole, as the spec names it. A biquad would be a different filter, not a tidier spelling of
/// this one.
fn high_pass_20hz(samples: &mut [f32]) {
    let rc = 1.0 / (2.0 * std::f64::consts::PI * 20.0);
    let dt = 1.0 / f64::from(ANALYSIS_RATE);
    let alpha = (rc / (rc + dt)) as f32;
    let mut previous_input = 0.0f32;
    let mut previous_output = 0.0f32;
    for sample in samples.iter_mut() {
        let input = *sample;
        let output = alpha * (previous_output + input - previous_input);
        previous_input = input;
        previous_output = output;
        *sample = output;
    }
}

struct Spectrogram {
    /// Row-major `frames * band_width`, log10 magnitude over bins `BAND_LOW_BIN..BAND_HIGH_BIN`.
    values: Vec<f32>,
    frames: usize,
    band_width: usize,
}

impl Spectrogram {
    fn at(&self, frame: usize, band_index: usize) -> f32 {
        self.values
            .get(frame * self.band_width + band_index)
            .copied()
            .unwrap_or(f32::NEG_INFINITY)
    }
}

fn spectrogram(samples: &[f32]) -> Result<Spectrogram, TraceError> {
    let band_width = BAND_HIGH_BIN - BAND_LOW_BIN;
    if samples.len() < WINDOW_LEN {
        return Ok(Spectrogram {
            values: Vec::new(),
            frames: 0,
            band_width,
        });
    }
    let stft = Stft::new(hann(WINDOW_LEN, Symmetry::Periodic), HOP).map_err(|error| {
        TraceError::Undecodable {
            reason: error.to_string(),
        }
    })?;
    let frames = stft
        .forward(samples)
        .map_err(|error| TraceError::Undecodable {
            reason: error.to_string(),
        })?;
    let frame_count = frames.frames();
    let mut values = vec![f32::NEG_INFINITY; frame_count * band_width];
    for frame in 0..frame_count {
        let Some(spectrum) = frames.frame(frame) else {
            continue;
        };
        for (offset, slot) in values[frame * band_width..(frame + 1) * band_width]
            .iter_mut()
            .enumerate()
        {
            let bin = BAND_LOW_BIN + offset;
            let magnitude = spectrum
                .get(bin)
                .map_or(0.0, |value| value.re.hypot(value.im));
            *slot = (f64::from(magnitude) + 1e-12).log10() as f32;
        }
    }
    Ok(Spectrogram {
        values,
        frames: frame_count,
        band_width,
    })
}

fn band_edges(band_width: usize) -> [usize; FLOOR_BANDS + 1] {
    let low = (BAND_LOW_BIN as f64).max(1.0);
    let high = (BAND_LOW_BIN + band_width) as f64;
    let ratio = (high / low).ln() / FLOOR_BANDS as f64;
    let mut edges = [0usize; FLOOR_BANDS + 1];
    for (index, edge) in edges.iter_mut().enumerate() {
        let bin = low * (ratio * index as f64).exp();
        let clamped = (bin.round() as usize).clamp(BAND_LOW_BIN, BAND_LOW_BIN + band_width);
        *edge = clamped - BAND_LOW_BIN;
    }
    edges[FLOOR_BANDS] = band_width;
    edges
}

/// Separable sliding-window maximum over `+-radius` along the rows of a row-major grid.
///
/// PERF: a direct neighbourhood scan is O(rows * cols * (2*radius_bins+1) * (2*radius_frames+1)),
/// which is ~5e8 comparisons for a minute of audio. Two monotonic-deque passes are O(rows * cols).
fn max_filter_rows(values: &[f32], rows: usize, cols: usize, radius: usize) -> Vec<f32> {
    let mut out = vec![f32::NEG_INFINITY; values.len()];
    let mut deque: VecDeque<usize> = VecDeque::with_capacity(cols.min(2 * radius + 2));
    for row in 0..rows {
        deque.clear();
        let base = row * cols;
        for column in 0..cols {
            while deque
                .back()
                .is_some_and(|back| values[base + *back] <= values[base + column])
            {
                deque.pop_back();
            }
            deque.push_back(column);
            if column >= radius {
                let centre = column - radius;
                let window_start = centre.saturating_sub(radius);
                while deque.front().is_some_and(|front| *front < window_start) {
                    deque.pop_front();
                }
                if let Some(front) = deque.front() {
                    out[base + centre] = values[base + front];
                }
            }
        }
        for centre in cols.saturating_sub(radius)..cols {
            let window_start = centre.saturating_sub(radius);
            while deque.front().is_some_and(|front| *front < window_start) {
                deque.pop_front();
            }
            if let Some(front) = deque.front() {
                out[base + centre] = values[base + front];
            }
        }
    }
    out
}

fn transpose(values: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    let mut out = vec![f32::NEG_INFINITY; values.len()];
    for row in 0..rows {
        for col in 0..cols {
            out[col * rows + row] = values[row * cols + col];
        }
    }
    out
}

fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return f32::NEG_INFINITY;
    }
    values.sort_by(f32::total_cmp);
    values[values.len() / 2]
}

/// Everything about a spectrogram that the global-floor bisection does NOT move, computed once so
/// the bisection costs four cheap passes instead of four full neighbourhood searches.
struct PeakContext {
    spec: Spectrogram,
    /// True where the point is the maximum of its `+-11` bin by `+-5` frame neighbourhood.
    local_max: Vec<bool>,
    /// Per-frame median plus 6 dB.
    median_gate: Vec<f32>,
    edges: [usize; FLOOR_BANDS + 1],
}

impl PeakContext {
    fn build(spec: Spectrogram) -> Self {
        let (frames, width) = (spec.frames, spec.band_width);
        let edges = band_edges(width);
        let mut median_gate = vec![f32::NEG_INFINITY; frames];
        let mut scratch = vec![0.0f32; width];
        for (frame, gate) in median_gate.iter_mut().enumerate().take(frames) {
            scratch.copy_from_slice(&spec.values[frame * width..(frame + 1) * width]);
            *gate = median(&mut scratch) + SIX_DB_LOG10 as f32;
        }

        let along_bins = max_filter_rows(&spec.values, frames, width, NEIGHBOURHOOD_BINS);
        let by_frame = transpose(&along_bins, frames, width);
        let along_frames = max_filter_rows(&by_frame, width, frames, NEIGHBOURHOOD_FRAMES);
        let neighbourhood_max = transpose(&along_frames, width, frames);

        let local_max = spec
            .values
            .iter()
            .zip(neighbourhood_max.iter())
            .map(|(value, maximum)| value.is_finite() && value >= maximum)
            .collect();

        Self {
            spec,
            local_max,
            median_gate,
            edges,
        }
    }
}

/// Selects peaks at one global floor offset. The offset is what the density bisection moves, and it
/// gates both the per-band adaptive floor and the per-frame median so it actually controls density
/// whichever of the two is binding.
fn select_peaks(context: &PeakContext, global_floor: f64) -> Vec<Peak> {
    let spec = &context.spec;
    let mut peaks = Vec::new();
    if spec.frames == 0 || spec.band_width == 0 {
        return peaks;
    }
    let offset = global_floor as f32;
    let mut floors = [f32::NEG_INFINITY; FLOOR_BANDS];

    for frame in 0..spec.frames {
        let median_gate = context.median_gate[frame] + offset;
        for floor in &mut floors {
            if floor.is_finite() {
                *floor -= FLOOR_DECAY_LOG10_AMPLITUDE as f32;
            }
        }

        for (band, floor) in floors.iter_mut().enumerate() {
            let (lo, hi) = (context.edges[band], context.edges[band + 1]);
            for band_index in lo..hi {
                let value = spec.at(frame, band_index);
                if !value.is_finite() || value <= median_gate {
                    continue;
                }
                if floor.is_finite() && value <= *floor + offset {
                    continue;
                }
                if !context.local_max[frame * spec.band_width + band_index] {
                    continue;
                }
                if peaks.len() >= MAX_PEAKS {
                    return peaks;
                }
                peaks.push(Peak {
                    frame: frame as u32,
                    bin: (BAND_LOW_BIN + band_index) as u16,
                });
                *floor = floor.max(value);
            }
        }
    }
    peaks
}

/// Extracts the constellation, bisecting the global floor at most three times to land inside the
/// target density band. Uniform density is what stops a loud section starving a quiet one.
pub fn constellation(samples: &[f32]) -> Result<Constellation, TraceError> {
    let spec = spectrogram(samples)?;
    if spec.frames == 0 {
        return Ok(Constellation {
            peaks: Vec::new(),
            frames: 0,
        });
    }
    let frames = spec.frames;
    let seconds = frames as f64 / FRAMES_PER_SECOND;
    let context = PeakContext::build(spec);

    let mut low = GLOBAL_FLOOR_LOW;
    let mut high = GLOBAL_FLOOR_HIGH;
    let mut best = select_peaks(&context, 0.0);
    let mut best_error = (best.len() as f64 / seconds - TARGET_PEAK_DENSITY).abs();

    if best_error > PEAK_DENSITY_TOLERANCE {
        for _ in 0..DENSITY_BISECTIONS {
            let midpoint = f64::midpoint(low, high);
            let candidate = select_peaks(&context, midpoint);
            let density = candidate.len() as f64 / seconds;
            let error = (density - TARGET_PEAK_DENSITY).abs();
            if error < best_error {
                best_error = error;
                best = candidate;
            }
            if error <= PEAK_DENSITY_TOLERANCE {
                break;
            }
            if density > TARGET_PEAK_DENSITY {
                low = midpoint;
            } else {
                high = midpoint;
            }
        }
    }

    best.sort_unstable();
    Ok(Constellation {
        peaks: best,
        frames,
    })
}

/// Pairs each anchor with up to `FAN_OUT` targets in its zone and packs the landmark hashes.
pub fn landmarks(constellation: &Constellation) -> Vec<Landmark> {
    let peaks = constellation.peaks();
    let mut out = Vec::new();
    for (index, anchor) in peaks.iter().enumerate() {
        let mut emitted = 0usize;
        for target in peaks.iter().skip(index + 1) {
            let dt = target.frame.saturating_sub(anchor.frame);
            if dt < MIN_DT_FRAMES {
                continue;
            }
            if dt > MAX_DT_FRAMES {
                break;
            }
            let df = i32::from(target.bin) - i32::from(anchor.bin);
            if df.abs() > MAX_DF_BINS {
                continue;
            }
            out.push(Landmark {
                hash: pack_hash(anchor.bin, target.bin, dt),
                t1: anchor.frame,
            });
            emitted += 1;
            if emitted >= FAN_OUT {
                break;
            }
        }
    }
    out
}

/// Query-side landmarks: each pair emitted three times with the target bin nudged by one, which
/// absorbs the bin quantisation a re-encode introduces.
pub fn query_landmarks(constellation: &Constellation) -> Vec<Landmark> {
    let peaks = constellation.peaks();
    let mut out = Vec::new();
    for (index, anchor) in peaks.iter().enumerate() {
        let mut emitted = 0usize;
        for target in peaks.iter().skip(index + 1) {
            let dt = target.frame.saturating_sub(anchor.frame);
            if dt < MIN_DT_FRAMES {
                continue;
            }
            if dt > MAX_DT_FRAMES {
                break;
            }
            let df = i32::from(target.bin) - i32::from(anchor.bin);
            if df.abs() > MAX_DF_BINS {
                continue;
            }
            for smear in QUERY_BIN_SMEAR {
                let smeared = i32::from(target.bin) + smear;
                if smeared < BAND_LOW_BIN as i32 || smeared >= BAND_HIGH_BIN as i32 {
                    continue;
                }
                out.push(Landmark {
                    hash: pack_hash(anchor.bin, smeared as u16, dt),
                    t1: anchor.frame,
                });
            }
            emitted += 1;
            if emitted >= FAN_OUT {
                break;
            }
        }
    }
    out
}

/// A playback-rate hypothesis applied to an already-extracted constellation.
///
/// A rate change moves both axes: at rate `s` the recording plays `s` times faster and every
/// partial lands `s` times higher. Undoing it on the peak list costs one pass instead of a second
/// STFT, and only runs when the 1.00 pass has already failed.
pub fn rescale(constellation: &Constellation, rate: f64) -> Constellation {
    if !rate.is_finite() || rate <= 0.0 {
        return constellation.clone();
    }
    let mut peaks: Vec<Peak> = constellation
        .peaks()
        .iter()
        .filter_map(|peak| {
            let frame = (f64::from(peak.frame) * rate).round();
            let bin = (f64::from(peak.bin) / rate).round();
            if !(0.0..=f64::from(u32::MAX)).contains(&frame) {
                return None;
            }
            if !(BAND_LOW_BIN as f64..BAND_HIGH_BIN as f64).contains(&bin) {
                return None;
            }
            Some(Peak {
                frame: frame as u32,
                bin: bin as u16,
            })
        })
        .collect();
    peaks.sort_unstable();
    peaks.dedup();
    let frames = ((constellation.frames() as f64) * rate).round() as usize;
    Constellation { peaks, frames }
}
