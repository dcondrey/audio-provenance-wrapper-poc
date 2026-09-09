use crate::channel::{ChannelFamily, Params};
use crate::corpus::{ContentClass, CorpusItemMeta};
use crate::perceptual::{PERCEPTUAL_LIMITS, PerceptualMeasurement};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;

pub const SCHEMA: &str = "audio-provenance-bench/1";

pub const ACOUSTIC_DISCLAIMER: &str = crate::channel::acoustic::SIMULATION_DISCLAIMER;

pub const FIXTURE_DISCLAIMER: &str = "The codec measured in this run is a BENCH FIXTURE: a \
deliberately simple stand-in that exists to validate the bench. Its numbers describe the fixture and \
say nothing about Watermark or any product watermark.";

pub const METRIC_DEFINITIONS: &[(&str, &str)] = &[
    (
        "exact_recovery_rate",
        "exact payload matches divided by trials that ran without error. The pass/fail metric.",
    ),
    (
        "mean_bit_error_rate",
        "mean differing-bit fraction over trials where the detector RETURNED a payload. Trials where \
         it declined contribute no bits and are excluded, so a low value alongside a high \
         payload-declined rate means the detector is silent rather than accurate. Read it next to \
         payload_returned_rate, never alone.",
    ),
    (
        "payload_returned_rate",
        "trials where the detector returned any payload, divided by trials that ran without error.",
    ),
    (
        "false_positive.rate",
        "accepts divided by unmarked trials that ran without error, on the SAME channel.",
    ),
    (
        "mean_detect_seconds",
        "wall time inside detect() only. Degradation time is reported separately per trial.",
    ),
    (
        "mean_frames_delta",
        "output frames minus input frames. Non-zero means the channel moved the audio in time; the \
         bench never realigns before detection, because sync is the detector's job.",
    ),
];

pub const FALSE_POSITIVE_NOTE: &str = "The false-positive arm runs the detector over UNMARKED audio \
through the same channel. A rate of 0.0 over N trials bounds the rate at roughly 3/N with 95% \
confidence; it does not establish that the rate is zero. The trial count is reported next to it so \
the bound can be computed rather than assumed.";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChannelExpectation {
    /// The row fails unless the exact-payload recovery rate reaches this.
    MinExactRecovery { rate: f64 },
    /// The row is measured and printed but cannot fail the run. Used where the design predicts
    /// failure in advance, so that a pass is a surprise rather than the threshold being lowered.
    ExpectedFailure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thresholds {
    pub default: ChannelExpectation,
    pub per_channel: BTreeMap<String, ChannelExpectation>,
    pub max_false_positive_rate: f64,
    pub fail_on_error_row: bool,
}

impl Thresholds {
    /// The targets the product is aiming at, taken from `docs/TEST_PLAN.md`. Rows the design
    /// predicts will fail are declared `ExpectedFailure` here rather than given a soft threshold.
    pub fn product_targets() -> Self {
        let mut per_channel = BTreeMap::new();
        let required: &[(&str, f64)] = &[
            ("identity", 0.99),
            ("mp3_320", 0.99),
            ("mp3_192", 0.99),
            ("mp3_128", 0.99),
            ("mp3_96", 0.90),
            ("mp3_64", 0.90),
            ("aac_256", 0.99),
            ("aac_128", 0.99),
            ("aac_64", 0.90),
            ("opus_128", 0.99),
            ("opus_64", 0.99),
            ("resample_via_44100", 0.99),
            ("resample_via_22050", 0.90),
            ("requantize_16bit", 0.99),
            ("requantize_8bit", 0.90),
            ("gain_minus_12db", 0.99),
            ("gain_minus_6db", 0.99),
            ("gain_plus_6db", 0.99),
            ("normalize_peak", 0.99),
            ("crop_0s5", 0.99),
            ("crop_2s", 0.99),
            ("crop_7s3", 0.99),
            ("drift_plus_0p1pct", 0.99),
            ("drift_minus_0p1pct", 0.99),
            ("noise_snr_40db", 0.99),
            ("noise_snr_30db", 0.99),
            ("noise_snr_20db", 0.90),
            ("lowpass_16k", 0.99),
            ("compress_dynamic", 0.90),
            ("limit_brickwall", 0.90),
            ("chain_transcode_mp3_128_aac_128", 0.99),
            ("chain_broadcast_limit_mp3_192", 0.90),
        ];
        for (name, rate) in required {
            per_channel.insert(
                (*name).to_owned(),
                ChannelExpectation::MinExactRecovery { rate: *rate },
            );
        }
        for name in [
            "lowpass_11k",
            "acoustic_small_room",
            "acoustic_medium_room",
            "acoustic_large_room",
            "chain_worst_case_limit_mp3_128_acoustic_mp3_192",
        ] {
            per_channel.insert(name.to_owned(), ChannelExpectation::ExpectedFailure);
        }
        Self {
            default: ChannelExpectation::MinExactRecovery { rate: 0.90 },
            per_channel,
            max_false_positive_rate: 0.0,
            fail_on_error_row: true,
        }
    }

    /// Every channel becomes a hard gate at `rate`, including the ones the design predicts will
    /// fail. Used to show what the matrix looks like with nothing excused.
    pub fn uniform_minimum(rate: f64) -> Self {
        Self {
            default: ChannelExpectation::MinExactRecovery { rate },
            per_channel: BTreeMap::new(),
            max_false_positive_rate: 0.0,
            fail_on_error_row: true,
        }
    }

    pub fn for_channel(&self, name: &str) -> ChannelExpectation {
        self.per_channel.get(name).copied().unwrap_or(self.default)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowVerdict {
    Pass,
    Fail,
    /// Measured and printed, with no threshold to meet.
    Recorded,
    /// The channel could not run. Never omitted, and it fails the run by default.
    Error,
}

impl RowVerdict {
    const fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Recorded => "REC ",
            Self::Error => "ERR ",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RowError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrialRow {
    pub item_id: String,
    pub class: ContentClass,
    pub seed: u64,
    pub exact: bool,
    pub payload_hex: Option<String>,
    pub bit_error_rate: Option<f64>,
    pub confidence: f64,
    pub bits_corrected: u32,
    pub degraded_frames: usize,
    pub frames_delta: i64,
    pub degrade_seconds: f64,
    pub detect_seconds: f64,
    pub error: Option<RowError>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct FalsePositiveArm {
    pub trials: usize,
    pub accepts: usize,
    pub rate: Option<f64>,
    pub errors: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelReport {
    pub channel: String,
    pub family: ChannelFamily,
    pub params: Params,
    pub simulated_physical_path: bool,
    pub expectation: ChannelExpectation,
    pub verdict: RowVerdict,
    pub error: Option<RowError>,
    pub trials: usize,
    pub trial_errors: usize,
    pub exact_recoveries: usize,
    pub exact_recovery_rate: Option<f64>,
    pub payload_returned_rate: Option<f64>,
    pub mean_bit_error_rate: Option<f64>,
    pub mean_detect_seconds: Option<f64>,
    pub p95_detect_seconds: Option<f64>,
    pub total_detect_seconds: f64,
    pub mean_frames_delta: Option<f64>,
    pub false_positive: FalsePositiveArm,
    pub trial_rows: Vec<TrialRow>,
    pub false_positive_rows: Vec<TrialRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PerceptualRow {
    pub item_id: String,
    pub class: ContentClass,
    pub measurement: PerceptualMeasurement,
    pub error: Option<RowError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodecReport {
    pub name: String,
    pub description: String,
    pub is_bench_fixture: bool,
    pub payload_len: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Totals {
    pub rows_pass: usize,
    pub rows_fail: usize,
    pub rows_recorded: usize,
    pub rows_error: usize,
    pub trials: usize,
    pub trial_errors: usize,
    pub exact_recoveries: usize,
    pub false_positive_trials: usize,
    pub false_positive_accepts: usize,
    pub overall_false_positive_rate: Option<f64>,
    /// Rule-of-three 95% upper bound on the false-positive rate when zero accepts were seen. Present
    /// so nobody has to compute it from the trial count to know what 0.0 is worth.
    pub false_positive_upper_bound_95: Option<f64>,
    pub embed_errors: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchReport {
    pub schema: &'static str,
    pub generated_at: Option<String>,
    pub seed: u64,
    pub payload_hex: String,
    pub codec: CodecReport,
    pub working_sample_rate: u32,
    pub working_channels: usize,
    pub corpus: Vec<CorpusItemMeta>,
    /// Set against `channels_reported` so silent truncation is checkable rather than trusted.
    pub channels_expected: usize,
    pub channels_reported: usize,
    pub truncated: bool,
    pub thresholds: Thresholds,
    pub rows: Vec<ChannelReport>,
    pub perceptual: Vec<PerceptualRow>,
    pub embed_errors: Vec<RowError>,
    pub totals: Totals,
    pub verdict: RowVerdict,
    pub verdict_reasons: Vec<String>,
    pub metric_definitions: BTreeMap<&'static str, &'static str>,
    pub disclaimers: Vec<String>,
    /// Present only on a null-test report. When it is present every recovery column above is empty
    /// by construction and the false-positive columns are the whole measurement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub null_test: Option<crate::null::NullTestSummary>,
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut acc, byte| {
        acc.push_str(&format!("{byte:02x}"));
        acc
    })
}

/// Deterministic per-cell seed. A row is replayable from `(item, channel, base seed)` alone.
pub fn cell_seed(base: u64, item: &str, channel: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ base;
    for byte in item
        .bytes()
        .chain(b"|".iter().copied())
        .chain(channel.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

pub fn percentile(mut values: Vec<f64>, fraction: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let index = ((values.len() - 1) as f64 * fraction).round() as usize;
    values.get(index).copied()
}

pub fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

fn rate(value: Option<f64>) -> String {
    value.map_or_else(|| "     -".to_owned(), |v| format!("{:6.3}", v))
}

impl BenchReport {
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Renders every row. Nothing is elided: a row that could not run prints its error text in the
    /// same table as the rows that did.
    pub fn to_table(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "audio-provenance-bench {}", SCHEMA);
        let _ = writeln!(
            out,
            "codec      : {} ({})",
            self.codec.name,
            if self.codec.is_bench_fixture {
                "BENCH FIXTURE, not a product watermark"
            } else {
                "production codec"
            }
        );
        let _ = writeln!(
            out,
            "corpus     : {} items at {} Hz, {} ch",
            self.corpus.len(),
            self.working_sample_rate,
            self.working_channels
        );
        let _ = writeln!(
            out,
            "payload    : {} ({} bytes)   seed: {}",
            self.payload_hex, self.codec.payload_len, self.seed
        );
        let _ = writeln!(
            out,
            "channels   : {} expected, {} reported{}",
            self.channels_expected,
            self.channels_reported,
            if self.truncated {
                "  *** TRUNCATED ***"
            } else {
                ""
            }
        );
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{:<48} {:>6} {:>6} {:>6} {:>6} {:>6} {:>8} {:<5}",
            "channel", "trials", "exact", "ber", "noPay", "fpr", "det ms", "v"
        );
        let _ = writeln!(out, "{}", "-".repeat(102));
        for row in &self.rows {
            let _ = writeln!(
                out,
                "{:<48} {:>6} {} {} {} {} {:>8} {:<5}",
                row.channel,
                row.trials,
                rate(row.exact_recovery_rate),
                rate(row.mean_bit_error_rate),
                rate(row.payload_returned_rate.map(|returned| 1.0 - returned)),
                rate(row.false_positive.rate),
                row.mean_detect_seconds
                    .map_or_else(|| "       -".to_owned(), |s| format!("{:8.2}", s * 1000.0)),
                row.verdict.label()
            );
            if let Some(error) = &row.error {
                let _ = writeln!(out, "    !! {} :: {}", error.code, error.message);
            }
            if row.trial_errors > 0 {
                let _ = writeln!(
                    out,
                    "    !! {} of {} trials errored",
                    row.trial_errors, row.trials
                );
            }
            if row.simulated_physical_path {
                let _ = writeln!(
                    out,
                    "    ~~ simulated physical path, not a measurement of one"
                );
            }
        }
        let _ = writeln!(out, "{}", "-".repeat(102));
        let _ = writeln!(
            out,
            "ber is measured ONLY over trials that returned a payload; noPay is the rate at which the\ndetector declined to answer. A low ber beside a high noPay means silent, not accurate."
        );
        let _ = writeln!(
            out,
            "rows       : {} pass, {} fail, {} recorded-no-threshold, {} error",
            self.totals.rows_pass,
            self.totals.rows_fail,
            self.totals.rows_recorded,
            self.totals.rows_error
        );
        let _ = writeln!(
            out,
            "false pos  : {} accepts in {} unmarked trials -> {}{}",
            self.totals.false_positive_accepts,
            self.totals.false_positive_trials,
            rate(self.totals.overall_false_positive_rate).trim(),
            self.totals
                .false_positive_upper_bound_95
                .map_or_else(String::new, |bound| format!(
                    "  (zero accepts; 95% upper bound {bound:.4})"
                ))
        );
        if let Some(null) = &self.null_test {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "NULL TEST over {} distinct never-marked works x {} channels",
                null.corpus.distinct_works, null.channels
            );
            let _ = writeln!(out, "  {}", null.corpus.description);
            let _ = writeln!(
                out,
                "  trials       : {} attempted, {} scored, {} errored",
                null.trials_attempted, null.trials_scored, null.trial_errors
            );
            let _ = writeln!(out, "  accepts      : {}", null.accepts);
            let bound = null
                .headline_upper_bound_95
                .map_or_else(|| "unmeasured".to_owned(), |b| format!("{b:.6}"));
            // A tie is the expected outcome at zero accepts with equal trial counts, and naming one
            // channel "worst" out of 37 identical bounds reads as a measured weakness that is not
            // there. Name a channel only when it stands alone at the top.
            if null.channels_at_headline_bound > 1 {
                let _ = writeln!(
                    out,
                    "  HEADLINE     : P(accept | never-marked) <= {bound} at 95% on EVERY channel. \
                     {} of {} channels sit exactly at this bound over {} scored works each; the \
                     remaining {} are tighter",
                    null.channels_at_headline_bound,
                    null.channels,
                    null.worst_channel_scored_trials,
                    null.channels
                        .saturating_sub(null.channels_at_headline_bound)
                );
            } else {
                let _ = writeln!(
                    out,
                    "  HEADLINE     : P(accept | never-marked) <= {bound} at 95%, worst channel {} \
                     over {} scored works",
                    null.worst_channel.as_deref().unwrap_or("-"),
                    null.worst_channel_scored_trials
                );
            }
            let _ = writeln!(
                out,
                "  pooled (NOT the product figure, correlated trials): {}",
                null.pooled_upper_bound_95
                    .map_or_else(|| "-".to_owned(), |b| format!("{b:.8}"))
            );
            for (label, works) in &null.works_required_for_bound {
                let _ = writeln!(
                    out,
                    "  to reach {label}: {works} distinct works per channel at zero accepts"
                );
            }
            for note in &null.notes {
                let _ = writeln!(out, "  * {note}");
            }
        }
        if !self.perceptual.is_empty() {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "{:<32} {:>10} {:>10} {:>10} {:>10}",
                "perceptual (original vs marked)", "segSNR dB", "NMR dB", "NMRmax dB", "resid dBFS"
            );
            let _ = writeln!(out, "{}", "-".repeat(76));
            for row in &self.perceptual {
                let _ = writeln!(
                    out,
                    "{:<32} {:>10} {:>10} {:>10} {:>10}",
                    row.item_id,
                    row.measurement
                        .segmental_snr_db
                        .map_or_else(|| "-".to_owned(), |v| format!("{v:.2}")),
                    row.measurement
                        .noise_to_mask_mean_db
                        .map_or_else(|| "-".to_owned(), |v| format!("{v:.2}")),
                    row.measurement
                        .noise_to_mask_max_db
                        .map_or_else(|| "-".to_owned(), |v| format!("{v:.2}")),
                    row.measurement
                        .peak_residual_dbfs
                        .map_or_else(|| "-".to_owned(), |v| format!("{v:.1}")),
                );
            }
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "VERDICT: {}", self.verdict.label().trim());
        for reason in &self.verdict_reasons {
            let _ = writeln!(out, "  - {reason}");
        }
        let _ = writeln!(out);
        for disclaimer in &self.disclaimers {
            let _ = writeln!(out, "* {disclaimer}");
            let _ = writeln!(out);
        }
        out
    }
}

pub fn metric_definitions() -> BTreeMap<&'static str, &'static str> {
    METRIC_DEFINITIONS.iter().copied().collect()
}

pub fn standard_disclaimers(uses_simulated_acoustic: bool, is_fixture: bool) -> Vec<String> {
    let mut out = Vec::new();
    if is_fixture {
        out.push(FIXTURE_DISCLAIMER.to_owned());
    }
    if uses_simulated_acoustic {
        out.push(ACOUSTIC_DISCLAIMER.to_owned());
    }
    out.push(FALSE_POSITIVE_NOTE.to_owned());
    out.push(PERCEPTUAL_LIMITS.to_owned());
    out
}
