//! The null test: detection over never-marked audio, at scale, across the whole channel matrix.
//!
//! [`crate::runner::run`] already carries a false-positive arm, but it is welded one-to-one to the
//! marked arm: one unmarked trial per corpus item per channel. A ten-item corpus therefore bounds
//! the false-positive rate at 3/370, and the bound, not the mark, is what limits what verification
//! may claim. This module runs the unmarked arm alone so the corpus can be a thousand works instead
//! of ten, and reports the bound the way a verifier actually meets it.
//!
//! # The headline bound is per channel, not pooled
//!
//! A `audio-provenance verify` call presents one file that has been through one distribution path. The
//! probability it wants bounded is P(accept | never-marked audio, that path). Pooling every channel
//! into one denominator answers a question nobody asks and divides the bound by the channel count,
//! so this module reports the WORST channel's bound as the headline and keeps the pooled figure
//! beside it, labelled. Trials sharing a work across channels are correlated; the pooled bound
//! would treat them as independent.

use crate::channel::Channel;
use crate::corpus::{CorpusFeed, CorpusItem};
use crate::error::BenchError;
use crate::ports::CommandRunner;
use crate::report::cell_seed;
use crate::report::{
    BenchReport, ChannelExpectation, ChannelReport, CodecReport, FalsePositiveArm, RowError,
    RowVerdict, SCHEMA, Thresholds, Totals, TrialRow, mean, percentile, standard_disclaimers,
};
use crate::watermark::{Detection, WatermarkCodec};
use audio_provenance_core::CodedError;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

pub const NULL_TEST_NOTE: &str = "This report is a NULL TEST: no audio in it was ever marked, and \
no marked-arm figure appears. Every recovery column is empty by construction. The only measured \
quantity is the rate at which the detector accepted a payload out of audio that never carried one.";

pub const BOUND_NOTE: &str = "The headline 95% upper bound is the worst single channel's bound over \
its own scored trials, because a verifier meets one channel at a time. The pooled bound over all \
channels is reported beside it and is NOT the product figure: trials that share a source work are \
correlated, so pooling overstates the evidence.";

/// How the corpus was assembled, carried into the report so a bound can never be read without the
/// sample it rests on.
#[derive(Debug, Clone, Serialize)]
pub struct NullCorpusProvenance {
    pub description: String,
    pub distinct_works: usize,
    pub manifest_sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NullConfig {
    pub seed: u64,
    /// Concurrent trial workers. Channel rows stay sequential so per-channel aggregation is exact.
    pub workers: usize,
    pub max_false_positive_rate: f64,
    pub generated_at: Option<String>,
    pub provenance: NullCorpusProvenance,
    /// Emitted to stderr as rows complete. The run is long enough that a silent hour is
    /// indistinguishable from a hang.
    pub progress: bool,
}

impl Default for NullConfig {
    fn default() -> Self {
        Self {
            seed: 0x6765_6E6F_746F_6E65,
            workers: 1,
            max_false_positive_rate: 0.0,
            generated_at: None,
            provenance: NullCorpusProvenance {
                description: String::new(),
                distinct_works: 0,
                manifest_sha256: None,
            },
            progress: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelBound {
    pub channel: String,
    pub scored_trials: usize,
    pub errors: usize,
    pub accepts: usize,
    pub rate: Option<f64>,
    /// 95% upper bound on this channel's accept rate. Rule of three at zero accepts, exact
    /// Clopper-Pearson otherwise.
    pub upper_bound_95: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NullTestSummary {
    pub corpus: NullCorpusProvenance,
    pub channels: usize,
    pub trials_attempted: usize,
    pub trials_scored: usize,
    pub trial_errors: usize,
    pub accepts: usize,
    pub accepted_payloads_hex: Vec<String>,
    pub pooled_rate: Option<f64>,
    pub pooled_upper_bound_95: Option<f64>,
    pub worst_channel: Option<String>,
    pub worst_channel_scored_trials: usize,
    /// How many channels share the headline bound. When every channel ties, naming one of them as
    /// "worst" invites the reader to believe that channel was measurably weaker; it was not.
    pub channels_at_headline_bound: usize,
    pub headline_upper_bound_95: Option<f64>,
    pub headline_basis: &'static str,
    /// Distinct works that would have to run, at zero accepts, to bound each channel at these
    /// rates. Stated so the gap between what was run and what the product claims is arithmetic
    /// rather than opinion.
    pub works_required_for_bound: BTreeMap<String, usize>,
    pub per_channel: Vec<ChannelBound>,
    pub notes: Vec<String>,
}

/// Rule of three: at zero events in `n` trials the 95% upper bound is about 3/n.
///
/// Slightly looser than the exact `1 - 0.05^(1/n)`, and kept because every other report in this
/// crate states 3/n and a bound that moved between reports would read as a measurement changing.
fn rule_of_three(n: usize) -> Option<f64> {
    (n > 0).then(|| (3.0 / n as f64).min(1.0))
}

/// P(X <= k) for X ~ Binomial(n, p), by forward recurrence. No log-gamma, and `k` here is a
/// handful at most.
fn binomial_cdf(k: usize, n: usize, p: f64) -> f64 {
    if p <= 0.0 {
        return 1.0;
    }
    if p >= 1.0 {
        return if k >= n { 1.0 } else { 0.0 };
    }
    let mut term = (1.0 - p).powi(i32::try_from(n).unwrap_or(i32::MAX));
    let mut sum = term;
    for i in 0..k.min(n) {
        term *= ((n - i) as f64 / (i + 1) as f64) * (p / (1.0 - p));
        sum += term;
    }
    sum.min(1.0)
}

/// Exact Clopper-Pearson 95% upper confidence limit on a binomial proportion, by bisection on the
/// CDF. Used whenever at least one accept was seen, where the rule of three does not apply.
fn clopper_pearson_upper_95(k: usize, n: usize) -> Option<f64> {
    if n == 0 {
        return None;
    }
    if k >= n {
        return Some(1.0);
    }
    let (mut low, mut high) = (k as f64 / n as f64, 1.0);
    for _ in 0..200 {
        let mid = f64::midpoint(low, high);
        if binomial_cdf(k, n, mid) > 0.05 {
            low = mid;
        } else {
            high = mid;
        }
    }
    Some(high)
}

pub fn upper_bound_95(accepts: usize, scored: usize) -> Option<f64> {
    if accepts == 0 {
        rule_of_three(scored)
    } else {
        clopper_pearson_upper_95(accepts, scored)
    }
}

/// Distinct works per channel needed to bound that channel at `target`, given zero accepts.
fn works_required(target: f64) -> usize {
    if target <= 0.0 {
        return usize::MAX;
    }
    (3.0 / target).ceil() as usize
}

fn row_error(error: &dyn CodedError, message: String) -> RowError {
    RowError {
        code: error.code().to_owned(),
        message,
    }
}

fn unmarked_trial(
    codec: &dyn WatermarkCodec,
    channel: &dyn Channel,
    runner: &dyn CommandRunner,
    item: &CorpusItem,
    seed: u64,
) -> (TrialRow, Option<Detection>) {
    let started = Instant::now();
    let degraded = match channel.apply(&item.audio, seed, runner) {
        Ok(audio) => audio,
        Err(error) => {
            let message = error.to_string();
            return (
                TrialRow {
                    item_id: item.meta.id.clone(),
                    class: item.meta.class,
                    seed,
                    exact: false,
                    payload_hex: None,
                    bit_error_rate: None,
                    confidence: 0.0,
                    bits_corrected: 0,
                    degraded_frames: 0,
                    frames_delta: 0,
                    degrade_seconds: started.elapsed().as_secs_f64(),
                    detect_seconds: 0.0,
                    error: Some(row_error(&error, message)),
                },
                None,
            );
        }
    };
    let degrade_seconds = started.elapsed().as_secs_f64();
    let frames = degraded.frames();
    let frames_delta = frames as i64 - item.audio.frames() as i64;

    let detect_started = Instant::now();
    let detection = codec.detect(&degraded);
    let detect_seconds = detect_started.elapsed().as_secs_f64();

    match detection {
        Ok(detection) => {
            let payload_hex = detection.payload.as_deref().map(crate::report::to_hex);
            (
                TrialRow {
                    item_id: item.meta.id.clone(),
                    class: item.meta.class,
                    seed,
                    exact: false,
                    payload_hex,
                    bit_error_rate: None,
                    confidence: detection.confidence,
                    bits_corrected: detection.bits_corrected,
                    degraded_frames: frames,
                    frames_delta,
                    degrade_seconds,
                    detect_seconds,
                    error: None,
                },
                Some(detection),
            )
        }
        Err(error) => {
            let message = error.to_string();
            (
                TrialRow {
                    item_id: item.meta.id.clone(),
                    class: item.meta.class,
                    seed,
                    exact: false,
                    payload_hex: None,
                    bit_error_rate: None,
                    confidence: 0.0,
                    bits_corrected: 0,
                    degraded_frames: frames,
                    frames_delta,
                    degrade_seconds,
                    detect_seconds,
                    error: Some(row_error(&error, message)),
                },
                None,
            )
        }
    }
}

fn run_channel(
    codec: &dyn WatermarkCodec,
    channel: &dyn Channel,
    corpus: &dyn CorpusFeed,
    runner: &dyn CommandRunner,
    config: &NullConfig,
) -> Vec<TrialRow> {
    let metas = corpus.metas();
    let slots: Mutex<Vec<Option<TrialRow>>> = Mutex::new(vec![None; metas.len()]);
    let next = AtomicUsize::new(0);
    let workers = config.workers.clamp(1, metas.len().max(1));

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(meta) = metas.get(index) else {
                        break;
                    };
                    let seed = cell_seed(config.seed, &meta.id, channel.name());
                    let row = match corpus.item(index) {
                        Ok(item) => unmarked_trial(codec, channel, runner, &item, seed).0,
                        Err(error) => TrialRow {
                            item_id: meta.id.clone(),
                            class: meta.class,
                            seed,
                            exact: false,
                            payload_hex: None,
                            bit_error_rate: None,
                            confidence: 0.0,
                            bits_corrected: 0,
                            degraded_frames: 0,
                            frames_delta: 0,
                            degrade_seconds: 0.0,
                            detect_seconds: 0.0,
                            error: Some(row_error(&error, error.to_string())),
                        },
                    };
                    if let Ok(mut guard) = slots.lock()
                        && let Some(slot) = guard.get_mut(index)
                    {
                        *slot = Some(row);
                    }
                }
            });
        }
    });

    // A poisoned mutex would mean a worker panicked mid-trial; the run is then not a measurement
    // and an empty row set makes the channel an explicit error row rather than a silent short count.
    slots
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect()
}

/// The null-test arm. Never embeds; every trial is unmarked audio through one channel.
#[allow(clippy::too_many_lines)]
pub fn run_null(
    codec: &dyn WatermarkCodec,
    corpus: &dyn CorpusFeed,
    matrix: &[Box<dyn Channel>],
    runner: &dyn CommandRunner,
    config: &NullConfig,
) -> Result<BenchReport, BenchError> {
    if corpus.metas().is_empty() {
        return Err(BenchError::EmptyCorpus);
    }
    if matrix.is_empty() {
        return Err(BenchError::EmptyMatrix);
    }

    let mut rows: Vec<ChannelReport> = Vec::new();
    let mut bounds: Vec<ChannelBound> = Vec::new();
    let mut accepted_payloads: Vec<String> = Vec::new();

    for (position, channel) in matrix.iter().enumerate() {
        let missing: Vec<String> = channel
            .required_programs()
            .into_iter()
            .filter(|program| !runner.is_available(program))
            .collect();

        if !missing.is_empty() {
            rows.push(empty_row(
                channel.as_ref(),
                Some(RowError {
                    code: "port_unavailable".to_owned(),
                    message: format!(
                        "required external program(s) not available: {}",
                        missing.join(", ")
                    ),
                }),
            ));
            bounds.push(ChannelBound {
                channel: channel.name().to_owned(),
                scored_trials: 0,
                errors: 0,
                accepts: 0,
                rate: None,
                upper_bound_95: None,
            });
            continue;
        }

        let started = Instant::now();
        let fp_rows = run_channel(codec, channel.as_ref(), corpus, runner, config);

        let attempted = fp_rows.len();
        let errors = fp_rows.iter().filter(|row| row.error.is_some()).count();
        let scored = attempted.saturating_sub(errors);
        let accepts = fp_rows
            .iter()
            .filter(|row| row.error.is_none() && row.payload_hex.is_some())
            .count();
        for row in &fp_rows {
            if let Some(hex) = &row.payload_hex {
                accepted_payloads.push(format!("{}@{}:{hex}", row.item_id, channel.name()));
            }
        }
        let rate = (scored > 0).then(|| accepts as f64 / scored as f64);
        let bound = upper_bound_95(accepts, scored);

        let detect_times: Vec<f64> = fp_rows
            .iter()
            .filter(|row| row.error.is_none())
            .map(|row| row.detect_seconds)
            .collect();
        let deltas: Vec<f64> = fp_rows
            .iter()
            .filter(|row| row.error.is_none())
            .map(|row| row.frames_delta as f64)
            .collect();

        let all_errored = attempted > 0 && errors == attempted;
        let verdict = if all_errored || attempted < corpus.metas().len() {
            RowVerdict::Error
        } else if rate.is_some_and(|value| value > config.max_false_positive_rate) {
            RowVerdict::Fail
        } else {
            RowVerdict::Recorded
        };

        if config.progress {
            eprintln!(
                "[{:>2}/{}] {:<50} scored {:>5}  accepts {:>3}  bound {:>9}  {:.1}s",
                position + 1,
                matrix.len(),
                channel.name(),
                scored,
                accepts,
                bound.map_or_else(|| "-".to_owned(), |value| format!("{value:.6}")),
                started.elapsed().as_secs_f64()
            );
        }

        bounds.push(ChannelBound {
            channel: channel.name().to_owned(),
            scored_trials: scored,
            errors,
            accepts,
            rate,
            upper_bound_95: bound,
        });

        rows.push(ChannelReport {
            channel: channel.name().to_owned(),
            family: channel.family(),
            params: channel.params(),
            simulated_physical_path: channel.is_simulated_physical_path(),
            expectation: ChannelExpectation::ExpectedFailure,
            verdict,
            error: if all_errored {
                fp_rows
                    .iter()
                    .find_map(|row| row.error.clone())
                    .map(|mut first| {
                        first.message = format!("every trial errored; first: {}", first.message);
                        first
                    })
            } else {
                None
            },
            trials: 0,
            trial_errors: 0,
            exact_recoveries: 0,
            exact_recovery_rate: None,
            payload_returned_rate: None,
            mean_bit_error_rate: None,
            mean_detect_seconds: mean(&detect_times),
            p95_detect_seconds: percentile(detect_times.clone(), 0.95),
            total_detect_seconds: detect_times.iter().sum(),
            mean_frames_delta: mean(&deltas),
            false_positive: FalsePositiveArm {
                trials: attempted,
                accepts,
                rate,
                errors,
            },
            trial_rows: Vec::new(),
            false_positive_rows: fp_rows,
        });
    }

    let attempted: usize = bounds.iter().map(|b| b.scored_trials + b.errors).sum();
    let scored: usize = bounds.iter().map(|b| b.scored_trials).sum();
    let trial_errors: usize = bounds.iter().map(|b| b.errors).sum();
    let accepts: usize = bounds.iter().map(|b| b.accepts).sum();

    let worst = bounds
        .iter()
        .filter(|b| b.upper_bound_95.is_some())
        .max_by(|a, b| {
            a.upper_bound_95
                .unwrap_or(1.0)
                .total_cmp(&b.upper_bound_95.unwrap_or(1.0))
        });
    let headline = worst.and_then(|b| b.upper_bound_95);
    let tied = bounds
        .iter()
        .filter(|b| b.upper_bound_95 == headline && headline.is_some())
        .count();
    let pooled_rate = (scored > 0).then(|| accepts as f64 / scored as f64);

    let mut required = BTreeMap::new();
    for (label, target) in [("1e-2", 0.01), ("1e-3", 0.001), ("1e-4", 0.0001)] {
        required.insert(label.to_owned(), works_required(target));
    }

    let mut notes = vec![NULL_TEST_NOTE.to_owned(), BOUND_NOTE.to_owned()];
    if trial_errors > 0 {
        notes.push(format!(
            "{trial_errors} of {attempted} trials errored and are excluded from every denominator; \
             a channel whose scored count is below the corpus size has a correspondingly weaker \
             bound."
        ));
    }
    let short: Vec<&ChannelBound> = bounds
        .iter()
        .filter(|b| b.scored_trials > 0 && b.scored_trials * 20 < corpus.metas().len() * 19)
        .collect();
    if !short.is_empty() {
        notes.push(format!(
            "channels scored on materially fewer works than the corpus holds: {}",
            short
                .iter()
                .map(|b| format!("{} ({})", b.channel, b.scored_trials))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if accepts > 0 {
        notes.push(
            "At least one false accept was seen, so the rule of three does not apply and the \
             per-channel bounds are exact Clopper-Pearson upper limits. Consumers that read \
             `totals.overall_false_positive_rate` receive a point estimate, not a bound."
                .to_owned(),
        );
    }
    notes.push(format!(
        "Detection wall times in this report were measured with {} concurrent workers and are not \
         single-trial latency figures.",
        config.workers.max(1)
    ));

    let summary = NullTestSummary {
        corpus: config.provenance.clone(),
        channels: matrix.len(),
        trials_attempted: attempted,
        trials_scored: scored,
        trial_errors,
        accepts,
        accepted_payloads_hex: accepted_payloads,
        pooled_rate,
        pooled_upper_bound_95: upper_bound_95(accepts, scored),
        worst_channel: worst.map(|b| b.channel.clone()),
        worst_channel_scored_trials: worst.map_or(0, |b| b.scored_trials),
        channels_at_headline_bound: tied,
        headline_upper_bound_95: headline,
        headline_basis: "worst_channel_95_upper_bound",
        works_required_for_bound: required,
        per_channel: bounds,
        notes,
    };

    let mut reasons = Vec::new();
    if accepts > 0 {
        reasons.push(format!(
            "{accepts} false accept(s) in {scored} scored never-marked trials"
        ));
    }
    let rows_error = rows
        .iter()
        .filter(|row| row.verdict == RowVerdict::Error)
        .count();
    if rows_error > 0 {
        reasons.push(format!("{rows_error} channel row(s) could not run"));
    }

    let totals = Totals {
        rows_pass: 0,
        rows_fail: rows
            .iter()
            .filter(|row| row.verdict == RowVerdict::Fail)
            .count(),
        rows_recorded: rows
            .iter()
            .filter(|row| row.verdict == RowVerdict::Recorded)
            .count(),
        rows_error,
        trials: 0,
        trial_errors: 0,
        exact_recoveries: 0,
        false_positive_trials: attempted,
        false_positive_accepts: accepts,
        overall_false_positive_rate: pooled_rate,
        // IMPORTANT: this is the WORST CHANNEL's bound, not 3/pooled. `apw_trace`'s NullTestTable
        // reads this field as the rate a soft binding is priced at, and a verifier meets one
        // channel, so pooling here would publish a number an order of magnitude better than any
        // single path was measured to.
        false_positive_upper_bound_95: (accepts == 0).then_some(headline).flatten(),
        embed_errors: 0,
    };

    let uses_simulated = matrix.iter().any(|c| c.is_simulated_physical_path());
    let mut disclaimers = standard_disclaimers(uses_simulated, codec.is_bench_fixture());
    disclaimers.insert(0, NULL_TEST_NOTE.to_owned());
    disclaimers.insert(1, BOUND_NOTE.to_owned());

    let first = corpus.metas().first();
    Ok(BenchReport {
        schema: SCHEMA,
        generated_at: config.generated_at.clone(),
        seed: config.seed,
        payload_hex: String::new(),
        codec: CodecReport {
            name: codec.name().to_owned(),
            description: codec.describe(),
            is_bench_fixture: codec.is_bench_fixture(),
            payload_len: codec.payload_len(),
        },
        working_sample_rate: first.map_or(0, |meta| meta.sample_rate),
        working_channels: first.map_or(0, |meta| meta.channels),
        corpus: corpus.metas().to_vec(),
        channels_expected: matrix.len(),
        channels_reported: rows.len(),
        truncated: matrix.len() != rows.len(),
        thresholds: Thresholds {
            default: ChannelExpectation::ExpectedFailure,
            per_channel: BTreeMap::new(),
            max_false_positive_rate: config.max_false_positive_rate,
            fail_on_error_row: true,
        },
        rows,
        perceptual: Vec::new(),
        embed_errors: Vec::new(),
        totals,
        verdict: if reasons.is_empty() {
            RowVerdict::Pass
        } else {
            RowVerdict::Fail
        },
        verdict_reasons: reasons,
        metric_definitions: crate::report::metric_definitions(),
        disclaimers,
        null_test: Some(summary),
    })
}

fn empty_row(channel: &dyn Channel, error: Option<RowError>) -> ChannelReport {
    ChannelReport {
        channel: channel.name().to_owned(),
        family: channel.family(),
        params: channel.params(),
        simulated_physical_path: channel.is_simulated_physical_path(),
        expectation: ChannelExpectation::ExpectedFailure,
        verdict: RowVerdict::Error,
        error,
        trials: 0,
        trial_errors: 0,
        exact_recoveries: 0,
        exact_recovery_rate: None,
        payload_returned_rate: None,
        mean_bit_error_rate: None,
        mean_detect_seconds: None,
        p95_detect_seconds: None,
        total_detect_seconds: 0.0,
        mean_frames_delta: None,
        false_positive: FalsePositiveArm {
            trials: 0,
            accepts: 0,
            rate: None,
            errors: 0,
        },
        trial_rows: Vec::new(),
        false_positive_rows: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{clopper_pearson_upper_95, rule_of_three, upper_bound_95};

    /// The bound is the product claim, so it is pinned rather than trusted. Zero accepts must never
    /// yield zero, and one accept must never yield a bound below the point estimate.
    #[test]
    fn bounds_never_collapse_to_zero() {
        assert!(upper_bound_95(0, 1_000).is_some_and(|b| (b - 0.003).abs() < 1e-12));
        assert_eq!(rule_of_three(0), None);
        for n in [1usize, 10, 370, 1_000, 37_000] {
            let bound = upper_bound_95(0, n).unwrap_or(0.0);
            assert!(
                bound > 0.0,
                "zero accepts in {n} trials must not bound at 0"
            );
        }
        // Clopper-Pearson at 1/1000 brackets the textbook value 0.00473.
        let one = clopper_pearson_upper_95(1, 1_000).unwrap_or(0.0);
        assert!(one > 0.001 && (one - 0.004736).abs() < 1e-4, "{one}");
        // The bound is monotone in the accept count at fixed n.
        let two = clopper_pearson_upper_95(2, 1_000).unwrap_or(0.0);
        assert!(two > one);
    }
}
