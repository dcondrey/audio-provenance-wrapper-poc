use crate::channel::Channel;
use crate::corpus::CorpusItem;
use crate::error::BenchError;
use crate::perceptual;
use crate::ports::CommandRunner;
use crate::report::{
    BenchReport, ChannelExpectation, ChannelReport, CodecReport, FalsePositiveArm, PerceptualRow,
    RowError, RowVerdict, SCHEMA, Thresholds, Totals, TrialRow, mean, percentile,
    standard_disclaimers,
};
use crate::watermark::{Detection, WatermarkCodec, bit_error_rate};
use audio_provenance_audio::AudioBuffer;
use audio_provenance_core::CodedError;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct BenchConfig {
    pub seed: u64,
    pub payload: Vec<u8>,
    pub thresholds: Thresholds,
    pub measure_perceptual: bool,
    /// Taken as a parameter rather than read from the clock, so a report is reproducible.
    pub generated_at: Option<String>,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            seed: 0x6765_6E6F_746F_6E65,
            payload: vec![0x47, 0x54, 0x01, 0x9A, 0xC3, 0x5E, 0x00, 0x11],
            thresholds: Thresholds::product_targets(),
            measure_perceptual: true,
            generated_at: None,
        }
    }
}

pub use crate::report::{cell_seed, to_hex};

fn row_error(error: &dyn CodedError, message: String) -> RowError {
    RowError {
        code: error.code().to_owned(),
        message,
    }
}

struct TrialOutcome {
    row: TrialRow,
    detection: Option<Detection>,
}

fn run_trial(
    codec: &dyn WatermarkCodec,
    channel: &dyn Channel,
    runner: &dyn CommandRunner,
    item: &CorpusItem,
    input: &AudioBuffer,
    seed: u64,
    expected_payload: Option<&[u8]>,
) -> TrialOutcome {
    let started = Instant::now();
    let degraded = match channel.apply(input, seed, runner) {
        Ok(audio) => audio,
        Err(error) => {
            let message = error.to_string();
            return TrialOutcome {
                row: TrialRow {
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
                detection: None,
            };
        }
    };
    let degrade_seconds = started.elapsed().as_secs_f64();
    let frames = degraded.frames();
    let frames_delta = frames as i64 - input.frames() as i64;

    let detect_started = Instant::now();
    let detection = codec.detect(&degraded);
    let detect_seconds = detect_started.elapsed().as_secs_f64();

    match detection {
        Ok(detection) => {
            let payload = detection.payload.clone();
            let ber = match (expected_payload, payload.as_deref()) {
                (Some(expected), Some(observed)) => bit_error_rate(expected, observed),
                _ => None,
            };
            let exact =
                matches!((expected_payload, payload.as_deref()), (Some(a), Some(b)) if a == b);
            TrialOutcome {
                row: TrialRow {
                    item_id: item.meta.id.clone(),
                    class: item.meta.class,
                    seed,
                    exact,
                    payload_hex: payload.as_deref().map(to_hex),
                    bit_error_rate: ber,
                    confidence: detection.confidence,
                    bits_corrected: detection.bits_corrected,
                    degraded_frames: frames,
                    frames_delta,
                    degrade_seconds,
                    detect_seconds,
                    error: None,
                },
                detection: Some(detection),
            }
        }
        Err(error) => {
            let message = error.to_string();
            TrialOutcome {
                row: TrialRow {
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
                detection: None,
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
pub fn run(
    codec: &dyn WatermarkCodec,
    corpus: &[CorpusItem],
    matrix: &[Box<dyn Channel>],
    runner: &dyn CommandRunner,
    config: &BenchConfig,
) -> Result<BenchReport, BenchError> {
    if corpus.is_empty() {
        return Err(BenchError::EmptyCorpus);
    }
    if matrix.is_empty() {
        return Err(BenchError::EmptyMatrix);
    }

    let mut marked: Vec<(usize, AudioBuffer)> = Vec::new();
    let mut embed_errors: Vec<RowError> = Vec::new();
    let mut perceptual_rows: Vec<PerceptualRow> = Vec::new();

    for (index, item) in corpus.iter().enumerate() {
        match codec.embed(&item.audio, &config.payload) {
            Ok(audio) => {
                if config.measure_perceptual {
                    let measurement = perceptual::measure(&item.audio, &audio);
                    perceptual_rows.push(match measurement {
                        Ok(measurement) => PerceptualRow {
                            item_id: item.meta.id.clone(),
                            class: item.meta.class,
                            measurement,
                            error: None,
                        },
                        Err(error) => {
                            let message = format!(
                                "{}: the embedder changed rate, channel count or length, so the \
                                 original and the marked copy cannot be compared sample for sample",
                                error
                            );
                            PerceptualRow {
                                item_id: item.meta.id.clone(),
                                class: item.meta.class,
                                measurement: perceptual::PerceptualMeasurement::empty(),
                                error: Some(row_error(&error, message)),
                            }
                        }
                    });
                }
                marked.push((index, audio));
            }
            Err(error) => {
                let message = format!("embed failed for `{}`: {}", item.meta.id, error);
                embed_errors.push(row_error(&error, message));
            }
        }
    }

    let mut rows: Vec<ChannelReport> = Vec::new();
    for channel in matrix {
        let expectation = config.thresholds.for_channel(channel.name());
        let missing: Vec<String> = channel
            .required_programs()
            .into_iter()
            .filter(|program| !runner.is_available(program))
            .collect();

        if !missing.is_empty() {
            rows.push(ChannelReport {
                channel: channel.name().to_owned(),
                family: channel.family(),
                params: channel.params(),
                simulated_physical_path: channel.is_simulated_physical_path(),
                expectation,
                verdict: RowVerdict::Error,
                error: Some(RowError {
                    code: "port_unavailable".to_owned(),
                    message: format!(
                        "required external program(s) not available: {}",
                        missing.join(", ")
                    ),
                }),
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
            });
            continue;
        }

        let mut trial_rows = Vec::new();
        let mut fp_rows = Vec::new();
        let mut detect_times = Vec::new();
        let mut bers = Vec::new();
        let mut deltas = Vec::new();
        let mut exact = 0usize;
        let mut returned = 0usize;
        let mut trial_errors = 0usize;
        let mut fp_accepts = 0usize;
        let mut fp_errors = 0usize;

        for (index, audio) in &marked {
            let Some(item) = corpus.get(*index) else {
                continue;
            };
            let seed = cell_seed(config.seed, &item.meta.id, channel.name());
            let outcome = run_trial(
                codec,
                channel.as_ref(),
                runner,
                item,
                audio,
                seed,
                Some(&config.payload),
            );
            if outcome.row.error.is_some() {
                trial_errors += 1;
            } else {
                detect_times.push(outcome.row.detect_seconds);
                deltas.push(outcome.row.frames_delta as f64);
                if outcome.row.exact {
                    exact += 1;
                }
                if outcome.row.payload_hex.is_some() {
                    returned += 1;
                }
                if let Some(ber) = outcome.row.bit_error_rate {
                    bers.push(ber);
                }
            }
            trial_rows.push(outcome.row);

            let fp_outcome = run_trial(
                codec,
                channel.as_ref(),
                runner,
                item,
                &item.audio,
                seed,
                None,
            );
            if fp_outcome.row.error.is_some() {
                fp_errors += 1;
            } else if fp_outcome
                .detection
                .as_ref()
                .is_some_and(Detection::is_accept)
            {
                fp_accepts += 1;
            }
            fp_rows.push(fp_outcome.row);
        }

        let trials = trial_rows.len();
        let scored = trials.saturating_sub(trial_errors);
        let fp_scored = fp_rows.len().saturating_sub(fp_errors);
        let exact_rate = (scored > 0).then(|| exact as f64 / scored as f64);
        let fp_rate = (fp_scored > 0).then(|| fp_accepts as f64 / fp_scored as f64);

        let all_errored = trials > 0 && trial_errors == trials;
        let mut verdict = if all_errored {
            RowVerdict::Error
        } else {
            match expectation {
                ChannelExpectation::ExpectedFailure => RowVerdict::Recorded,
                ChannelExpectation::MinExactRecovery { rate } => {
                    if exact_rate.is_some_and(|value| value >= rate) {
                        RowVerdict::Pass
                    } else {
                        RowVerdict::Fail
                    }
                }
            }
        };
        if fp_rate.is_some_and(|value| value > config.thresholds.max_false_positive_rate)
            && verdict != RowVerdict::Error
        {
            verdict = RowVerdict::Fail;
        }

        let error = if all_errored {
            trial_rows
                .iter()
                .find_map(|row| row.error.clone())
                .map(|mut first| {
                    first.message = format!("every trial errored; first: {}", first.message);
                    first
                })
        } else {
            None
        };

        rows.push(ChannelReport {
            channel: channel.name().to_owned(),
            family: channel.family(),
            params: channel.params(),
            simulated_physical_path: channel.is_simulated_physical_path(),
            expectation,
            verdict,
            error,
            trials,
            trial_errors,
            exact_recoveries: exact,
            exact_recovery_rate: exact_rate,
            payload_returned_rate: (scored > 0).then(|| returned as f64 / scored as f64),
            mean_bit_error_rate: mean(&bers),
            mean_detect_seconds: mean(&detect_times),
            p95_detect_seconds: percentile(detect_times.clone(), 0.95),
            total_detect_seconds: detect_times.iter().sum(),
            mean_frames_delta: mean(&deltas),
            false_positive: FalsePositiveArm {
                trials: fp_rows.len(),
                accepts: fp_accepts,
                rate: fp_rate,
                errors: fp_errors,
            },
            trial_rows,
            false_positive_rows: fp_rows,
        });
    }

    let channels_expected = matrix.len();
    let channels_reported = rows.len();
    let truncated = channels_expected != channels_reported;

    let totals = Totals {
        rows_pass: rows
            .iter()
            .filter(|r| r.verdict == RowVerdict::Pass)
            .count(),
        rows_fail: rows
            .iter()
            .filter(|r| r.verdict == RowVerdict::Fail)
            .count(),
        rows_recorded: rows
            .iter()
            .filter(|r| r.verdict == RowVerdict::Recorded)
            .count(),
        rows_error: rows
            .iter()
            .filter(|r| r.verdict == RowVerdict::Error)
            .count(),
        trials: rows.iter().map(|r| r.trials).sum(),
        trial_errors: rows.iter().map(|r| r.trial_errors).sum(),
        exact_recoveries: rows.iter().map(|r| r.exact_recoveries).sum(),
        false_positive_trials: rows.iter().map(|r| r.false_positive.trials).sum(),
        false_positive_accepts: rows.iter().map(|r| r.false_positive.accepts).sum(),
        overall_false_positive_rate: None,
        false_positive_upper_bound_95: None,
        embed_errors: embed_errors.len(),
    };
    let fp_denominator: usize = rows
        .iter()
        .map(|r| {
            r.false_positive
                .trials
                .saturating_sub(r.false_positive.errors)
        })
        .sum();
    let totals = Totals {
        overall_false_positive_rate: (fp_denominator > 0)
            .then(|| totals.false_positive_accepts as f64 / fp_denominator as f64),
        false_positive_upper_bound_95: (fp_denominator > 0 && totals.false_positive_accepts == 0)
            .then(|| 3.0 / fp_denominator as f64),
        ..totals
    };

    let mut reasons = Vec::new();
    if truncated {
        reasons.push(format!(
            "{channels_expected} channels were requested but {channels_reported} were reported"
        ));
    }
    if !embed_errors.is_empty() {
        reasons.push(format!(
            "{} corpus item(s) could not be embedded",
            embed_errors.len()
        ));
    }
    if totals.rows_fail > 0 {
        reasons.push(format!(
            "{} channel row(s) missed their threshold",
            totals.rows_fail
        ));
    }
    if totals.rows_error > 0 {
        reasons.push(format!(
            "{} channel row(s) could not run",
            totals.rows_error
        ));
    }
    if let Some(rate) = totals.overall_false_positive_rate
        && rate > config.thresholds.max_false_positive_rate
    {
        reasons.push(format!(
            "false-positive rate {rate:.4} over {fp_denominator} unmarked trials exceeds the \
             {:.4} ceiling",
            config.thresholds.max_false_positive_rate
        ));
    }

    let fails = totals.rows_fail > 0
        || truncated
        || !embed_errors.is_empty()
        || (config.thresholds.fail_on_error_row && totals.rows_error > 0)
        || totals
            .overall_false_positive_rate
            .is_some_and(|rate| rate > config.thresholds.max_false_positive_rate);

    let uses_simulated = matrix.iter().any(|c| c.is_simulated_physical_path());
    let first = corpus.first().map(|item| &item.audio);

    Ok(BenchReport {
        schema: SCHEMA,
        generated_at: config.generated_at.clone(),
        seed: config.seed,
        payload_hex: to_hex(&config.payload),
        codec: CodecReport {
            name: codec.name().to_owned(),
            description: codec.describe(),
            is_bench_fixture: codec.is_bench_fixture(),
            payload_len: codec.payload_len(),
        },
        working_sample_rate: first.map_or(0, AudioBuffer::sample_rate),
        working_channels: first.map_or(0, AudioBuffer::channels),
        corpus: corpus.iter().map(|item| item.meta.clone()).collect(),
        channels_expected,
        channels_reported,
        truncated,
        thresholds: config.thresholds.clone(),
        rows,
        perceptual: perceptual_rows,
        embed_errors,
        totals,
        verdict: if fails {
            RowVerdict::Fail
        } else {
            RowVerdict::Pass
        },
        verdict_reasons: reasons,
        metric_definitions: crate::report::metric_definitions(),
        disclaimers: standard_disclaimers(uses_simulated, codec.is_bench_fixture()),
        null_test: None,
    })
}
