use crate::error::NeuralWatermarkError;
use crate::params::{
    CONFIDENCE_LOCATOR_MULTI, CONFIDENCE_LOCATOR_SINGLE, DENSE_CHUNK_MARGIN_SECONDS,
    DENSE_CHUNK_SECONDS, FLIP_PATTERNS, FLIP_SEARCH_BITS, FLIP_SEARCH_WEIGHT, FRAME_RATE_HZ,
    MESSAGE_BITS,
};
use crate::payload::{Payload, accept_message};
use crate::session::DecoderSession;
use crate::spectral::Analysis;
use crate::thresholds::Thresholds;

/// WATERMARK_N_SPEC.md section 5.4, minus the two registry-resolved classes.
///
/// `locator_ambiguous` and the `mark_disagreement` verdict live above this crate: both need a
/// registry lookup and Watermark-Q's decode, and neither is a property of the signal. A caller that
/// resolves a bucket holding two or more candidates MUST report `untrusted / ambiguous_binding`
/// rather than choosing; at registry scale spec 5.3 puts that at roughly 39% of hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionClass {
    None,
    Presence,
    LocatorSingle,
    LocatorMulti,
    /// Two windows each passed the CRC carrying DIFFERENT payloads. Spec 8.5: disagreement is
    /// terminal, with no descent down the ladder. Two marks from different works in one file is a
    /// splice or a forgery, and returning either one would be a coin flip that produces an
    /// identity.
    LocatorConflict,
}

impl DetectionClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Presence => "presence",
            Self::LocatorSingle => "locator_single",
            Self::LocatorMulti => "locator_multi",
            Self::LocatorConflict => "locator_conflict",
        }
    }

    pub const fn confidence(self) -> f64 {
        match self {
            // A presence hit contributes a Finding and never an identity, so it carries no match
            // value at all (spec 5.4).
            Self::None | Self::Presence | Self::LocatorConflict => 0.0,
            Self::LocatorSingle => CONFIDENCE_LOCATOR_SINGLE,
            Self::LocatorMulti => CONFIDENCE_LOCATOR_MULTI,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NeuralDetection {
    class: DetectionClass,
    payload: Option<Payload>,
    presence_score: f64,
    presence_positive_seconds: f64,
    frames_analysed: usize,
    windows_examined: usize,
    crc_trials: u64,
    bits_corrected: u32,
}

impl NeuralDetection {
    pub const fn class(&self) -> DetectionClass {
        self.class
    }

    pub const fn payload(&self) -> Option<Payload> {
        self.payload
    }

    /// The pooled presence statistic: the largest window mean of `sigmoid(p[t])` seen. Comparable
    /// only against the threshold frozen in the model card that produced it.
    pub const fn presence_score(&self) -> f64 {
        self.presence_score
    }

    pub const fn presence_positive_seconds(&self) -> f64 {
        self.presence_positive_seconds
    }

    pub const fn frames_analysed(&self) -> usize {
        self.frames_analysed
    }

    pub const fn windows_examined(&self) -> usize {
        self.windows_examined
    }

    pub const fn crc_trials(&self) -> u64 {
        self.crc_trials
    }

    pub const fn bits_corrected(&self) -> u32 {
        self.bits_corrected
    }

    pub const fn confidence(&self) -> f64 {
        self.class.confidence()
    }
}

fn sigmoid(value: f32) -> f64 {
    1.0 / (1.0 + (-f64::from(value)).exp())
}

fn median_smooth(values: &[f64], radius: usize) -> Vec<f64> {
    if radius == 0 {
        return values.to_vec();
    }
    let mut out = Vec::with_capacity(values.len());
    let mut scratch: Vec<f64> = Vec::with_capacity(radius * 2 + 1);
    for index in 0..values.len() {
        let low = index.saturating_sub(radius);
        let high = (index + radius + 1).min(values.len());
        scratch.clear();
        scratch.extend_from_slice(&values[low..high]);
        scratch.sort_by(f64::total_cmp);
        out.push(scratch[scratch.len() / 2]);
    }
    out
}

/// The 11 flip patterns of WATERMARK_N_SPEC.md section 3.2: the all-zero pattern, the four singles
/// and the six doubles over the FOUR least confident bits, and nothing wider.
///
/// IMPORTANT: widening this budget buys recovery by spending false-accept margin. Spec 3.4 counts
/// 64 windows x 11 patterns x 2^-24 = 4.2e-5 false accepts per file; if either number moves, the
/// CRC width has to be redone with it, and widening one without the other is how a false identity
/// attribution ships.
fn flip_patterns() -> [u8; FLIP_PATTERNS] {
    let mut patterns = [0u8; FLIP_PATTERNS];
    let mut count = 0usize;
    for mask in 0u8..(1 << FLIP_SEARCH_BITS) {
        if (mask.count_ones() as usize) <= FLIP_SEARCH_WEIGHT && count < FLIP_PATTERNS {
            patterns[count] = mask;
            count += 1;
        }
    }
    patterns
}

struct WindowRead {
    start: usize,
    payload: Payload,
    bits_corrected: u32,
}

/// Ordered-statistics decode of one window's pooled bit logits. There is NO expected payload here
/// and there is none anywhere in this crate's API: every published physical result in this field is
/// a best-of-search maximum scored against the known bits, and reproducing that pattern is what
/// would make a measurement from this detector meaningless.
fn decode_window(logits: &[f32; MESSAGE_BITS]) -> (Option<(Payload, u32)>, u64) {
    let mut order: Vec<usize> = (0..MESSAGE_BITS).collect();
    order.sort_by(|a, b| logits[*a].abs().total_cmp(&logits[*b].abs()));
    let hard: Vec<u8> = logits.iter().map(|logit| u8::from(*logit > 0.0)).collect();
    let mut trials = 0u64;
    for pattern in flip_patterns() {
        let mut candidate = hard.clone();
        for bit in 0..FLIP_SEARCH_BITS {
            if pattern & (1 << bit) != 0 {
                candidate[order[bit]] ^= 1;
            }
        }
        trials += 1;
        if let Some(payload) = accept_message(&candidate) {
            return (Some((payload, pattern.count_ones())), trials);
        }
    }
    (None, trials)
}

/// Runs the detector densely over the signal and pools per-frame scores.
///
/// THERE IS NO ALIGNMENT SEARCH, because there is no frame grid to align to: the decoder is
/// fully convolutional in time and the message read is a global average over the presence-weighted
/// span, which is invariant to translation. Bulk delay is a pure translation and clock drift is a
/// slow dilation, and pooling absorbs both (spec 4.2).
pub fn detect(
    decoder: &DecoderSession,
    analysis: &Analysis,
    thresholds: &Thresholds,
    signal: &[f32],
) -> Result<NeuralDetection, NeuralWatermarkError> {
    let frames = analysis.forward(signal)?;
    let time = frames.frames();
    let log_mag = analysis.band_log_magnitude(&frames);
    let dense = dense_presence(decoder, &log_mag, time)?;

    let gate: Vec<f64> = dense.iter().copied().map(sigmoid).collect();
    let radius = ((thresholds.frame_gate_median_seconds * FRAME_RATE_HZ) / 2.0).round() as usize;
    let smoothed = median_smooth(&gate, radius);
    let positive: Vec<bool> = smoothed
        .iter()
        .map(|value| *value > thresholds.presence_frame_gate)
        .collect();
    let presence_positive_seconds =
        positive.iter().filter(|flag| **flag).count() as f64 / FRAME_RATE_HZ;

    let duration_s = time as f64 / FRAME_RATE_HZ;
    let mut presence_score = 0.0f64;
    if duration_s >= thresholds.min_presence_seconds {
        let width = seconds_to_frames(thresholds.presence_window_seconds);
        let hop = seconds_to_frames(thresholds.presence_window_hop_seconds).max(1);
        let mut start = 0usize;
        loop {
            let end = (start + width).min(time);
            // The final window is anchored on the END of the signal rather than on the hop grid, so
            // no frame sits outside every window; a dropped tail would move the integral the model
            // card's operating point is frozen against.
            let low = end.saturating_sub(width);
            let mean = gate[low..end].iter().sum::<f64>() / (end - low) as f64;
            presence_score = presence_score.max(mean);
            if end == time {
                break;
            }
            start += hop;
        }
    }

    let mut reads: Vec<WindowRead> = Vec::new();
    let mut windows_examined = 0usize;
    let mut crc_trials = 0u64;
    if duration_s >= thresholds.min_locator_seconds {
        let width = seconds_to_frames(thresholds.locator_window_seconds);
        let hop = seconds_to_frames(thresholds.locator_window_hop_seconds).max(1);
        let mut start = 0usize;
        while start < time && windows_examined < thresholds.max_windows {
            let end = (start + width).min(time);
            if end.saturating_sub(start) < seconds_to_frames(thresholds.min_locator_seconds) {
                break;
            }
            let span =
                positive[start..end].iter().filter(|flag| **flag).count() as f64 / FRAME_RATE_HZ;
            if span >= thresholds.min_presence_span_seconds {
                windows_examined += 1;
                let slice = slice_tensor(&log_mag, time, start, end);
                let window = decoder.run(&slice, end - start)?;
                let (accepted, trials) = decode_window(&window.message_logits);
                crc_trials += trials;
                if let Some((payload, bits_corrected)) = accepted {
                    reads.push(WindowRead {
                        start,
                        payload,
                        bits_corrected,
                    });
                }
            }
            if end == time {
                break;
            }
            start += hop;
        }
    }

    let width = seconds_to_frames(thresholds.locator_window_seconds);
    let (class, payload, bits_corrected) = classify(&reads, width);
    let class =
        if class == DetectionClass::None && presence_score >= thresholds.presence_accept_score {
            DetectionClass::Presence
        } else {
            class
        };

    Ok(NeuralDetection {
        class,
        payload,
        presence_score,
        presence_positive_seconds,
        frames_analysed: time,
        windows_examined,
        crc_trials,
        bits_corrected,
    })
}

/// `locator_multi` needs two NON-OVERLAPPING windows carrying identical payload bits. Two
/// overlapping windows share most of their integration span, so agreement between them is close to
/// one observation restated and must not be counted as two (spec 5.4).
fn classify(reads: &[WindowRead], width: usize) -> (DetectionClass, Option<Payload>, u32) {
    let Some(first) = reads.first() else {
        return (DetectionClass::None, None, 0);
    };
    if reads.iter().any(|read| read.payload != first.payload) {
        return (DetectionClass::LocatorConflict, None, 0);
    }
    for (index, read) in reads.iter().enumerate() {
        for other in &reads[index + 1..] {
            if other.payload == read.payload && other.start >= read.start + width {
                return (
                    DetectionClass::LocatorMulti,
                    Some(read.payload),
                    read.bits_corrected.max(other.bits_corrected),
                );
            }
        }
    }
    (
        DetectionClass::LocatorSingle,
        Some(first.payload),
        first.bits_corrected,
    )
}

/// Runs the decoder over the whole signal in bounded chunks and stitches the per-frame presence
/// logits back together, keeping only each chunk's interior.
///
/// This is not an alignment search and cannot become one: the chunk grid is fixed by duration
/// alone, every frame is scored exactly once, and nothing here compares a result against a payload.
fn dense_presence(
    decoder: &DecoderSession,
    log_mag: &[f32],
    time: usize,
) -> Result<Vec<f32>, NeuralWatermarkError> {
    let margin = seconds_to_frames(DENSE_CHUNK_MARGIN_SECONDS);
    let interior = seconds_to_frames(DENSE_CHUNK_SECONDS);
    if time <= interior + 2 * margin {
        return Ok(decoder.run(log_mag, time)?.presence_logits);
    }
    let mut out = Vec::with_capacity(time);
    let mut keep_from = 0usize;
    while keep_from < time {
        let keep_to = (keep_from + interior).min(time);
        let start = keep_from.saturating_sub(margin);
        let end = (keep_to + margin).min(time);
        let slice = slice_tensor(log_mag, time, start, end);
        let chunk = decoder.run(&slice, end - start)?;
        let offset = keep_from - start;
        let Some(interior_logits) = chunk
            .presence_logits
            .get(offset..offset + (keep_to - keep_from))
        else {
            return Err(NeuralWatermarkError::Inference {
                graph: "decoder",
                reason: format!(
                    "chunk {start}..{end} returned {} frames",
                    chunk.presence_logits.len()
                ),
            });
        };
        out.extend_from_slice(interior_logits);
        keep_from = keep_to;
    }
    Ok(out)
}

fn seconds_to_frames(seconds: f64) -> usize {
    (seconds * FRAME_RATE_HZ).round().max(1.0) as usize
}

/// Frequency-major `[320, time]` sliced on the time axis, preserving the layout the graph expects.
fn slice_tensor(tensor: &[f32], time: usize, start: usize, end: usize) -> Vec<f32> {
    let width = end - start;
    let mut out = vec![0.0f32; crate::params::BAND_BINS * width];
    for row in 0..crate::params::BAND_BINS {
        let source = row * time + start;
        let Some(chunk) = tensor.get(source..source + width) else {
            continue;
        };
        out[row * width..(row + 1) * width].copy_from_slice(chunk);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::PAYLOAD_VERSION;

    #[test]
    fn the_flip_budget_is_exactly_eleven_patterns_of_weight_at_most_two() {
        let patterns = flip_patterns();
        assert_eq!(patterns.len(), FLIP_PATTERNS);
        assert!(
            patterns
                .iter()
                .all(|mask| (mask.count_ones() as usize) <= FLIP_SEARCH_WEIGHT
                    && *mask < (1 << FLIP_SEARCH_BITS))
        );
        let mut sorted = patterns;
        sorted.sort_unstable();
        sorted
            .windows(2)
            .for_each(|pair| assert_ne!(pair[0], pair[1]));
    }

    /// Two corrupted bits placed on the two least confident positions must be recovered; a third
    /// must not be, or the budget spec 3.4 counts is not the budget being spent.
    #[test]
    fn recovers_two_flips_among_the_least_confident_and_refuses_three() {
        let payload = Payload::new(PAYLOAD_VERSION, 5, 0x0009_ABCD).unwrap();
        let truth = payload.to_message_bits();
        let mut logits = [0.0f32; MESSAGE_BITS];
        for (index, bit) in truth.iter().enumerate() {
            logits[index] = if *bit == 1 { 4.0 } else { -4.0 };
        }
        for (rank, index) in [7usize, 19, 41].into_iter().enumerate() {
            logits[index] = -logits[index] * (0.1 + 0.01 * rank as f32);
        }
        let (two_wrong, _) = decode_window(&logits);
        assert_eq!(two_wrong, None, "three flipped bits must not be recovered");

        logits[41] = if truth[41] == 1 { 4.0 } else { -4.0 };
        let (recovered, trials) = decode_window(&logits);
        assert_eq!(recovered, Some((payload, 2)));
        assert!(trials <= FLIP_PATTERNS as u64);
    }
}

#[cfg(test)]
mod chunking {
    use super::*;
    use std::path::Path;

    use crate::card::ModelCard;
    use crate::params::BAND_BINS;

    /// Every frame is scored exactly once and the chunk grid never changes a value. The fixture
    /// decoder's presence head is exactly local, so a stitched pass and a single dense pass must
    /// agree to the bit; a stitching bug that dropped or duplicated a frame would move the
    /// presence integral and, through it, the operating point the model card freezes.
    #[test]
    fn chunked_dense_pass_equals_a_single_pass() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let card = ModelCard::read(&fixtures.join("apw-watermark-neural-fixture-v1.card.json")).unwrap();
        let bytes = card
            .read_graph(&fixtures, "decoder", &card.graphs.decoder)
            .unwrap();
        let decoder = DecoderSession::new(
            &bytes,
            &card.io.decoder_input,
            &card.io.decoder_presence_output,
            &card.io.decoder_message_output,
        )
        .unwrap();

        let time = 13_000usize;
        let mut tensor = vec![0.0f32; BAND_BINS * time];
        for row in 0..BAND_BINS {
            for t in 0..time {
                tensor[row * time + t] = ((row * 31 + t * 7) % 23) as f32 * 0.125 - 1.0;
            }
        }
        let stitched = dense_presence(&decoder, &tensor, time).unwrap();
        let single = decoder.run(&tensor, time).unwrap().presence_logits;
        assert_eq!(stitched.len(), time);
        assert_eq!(single.len(), time);
        for (index, (a, b)) in stitched.iter().zip(single.iter()).enumerate() {
            assert!((a - b).abs() < 1e-6, "frame {index}: {a} vs {b}");
        }
    }
}

#[cfg(test)]
mod agreement {
    use super::*;
    use crate::params::PAYLOAD_VERSION;

    fn read(start: usize, locator: u32) -> WindowRead {
        WindowRead {
            start,
            payload: Payload::new(PAYLOAD_VERSION, 1, locator).unwrap(),
            bits_corrected: 0,
        }
    }

    /// Spec 8.5: DISAGREEMENT IS TERMINAL. Two windows that each pass the CRC with different
    /// payloads must yield no identity at all, never the first one seen.
    #[test]
    fn disagreeing_windows_yield_no_payload() {
        let (class, payload, _) = classify(&[read(0, 11), read(4000, 12)], 2813);
        assert_eq!(class, DetectionClass::LocatorConflict);
        assert_eq!(payload, None);
        assert_eq!(class.confidence(), 0.0);
    }

    /// Agreement between two OVERLAPPING windows is close to one observation restated, so it is
    /// `locator_single`; only non-overlapping agreement earns `locator_multi`.
    #[test]
    fn only_non_overlapping_agreement_is_multi() {
        let width = 2813usize;
        let (overlapping, _, _) = classify(&[read(0, 11), read(1406, 11)], width);
        assert_eq!(overlapping, DetectionClass::LocatorSingle);
        let (separated, payload, _) = classify(&[read(0, 11), read(width, 11)], width);
        assert_eq!(separated, DetectionClass::LocatorMulti);
        assert!(payload.is_some());
    }
}
