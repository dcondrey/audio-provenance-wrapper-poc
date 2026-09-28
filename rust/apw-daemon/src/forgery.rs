//! The statistical forgery screen: a port of `daemon/forgery_analysis/analyzer.py`
//! and `derive_forgery_analysis` in `daemon/manifest_builder/generator.py`.
//!
//! IMPORTANT: Python is the oracle and the comparison is byte-for-byte. Three
//! CPython behaviours are reproduced on purpose:
//! - `sum()` over floats is Neumaier-compensated since 3.12, so a plain left fold
//!   drifts in the last bit and can flip a threshold or a rendered digit;
//! - `x ** 2` and `x ** 3` call libm `pow`, so they use `powf` here, not `x * x`;
//! - `f"{ratio:.0%}"` multiplies by 100 before formatting.

use apw_core::{ProofLevel, PROOF_LEVEL_KEY};
use serde_json::{json, Map, Value};

use crate::services::ForgeryAnalyzer;
use crate::util::python_round;

/// A single indicator of potential forgery or synthetic generation.
#[derive(Debug, Clone, PartialEq)]
pub struct ForgeryFlag {
    pub name: &'static str,
    pub description: &'static str,
    /// 0.0 (informational) to 1.0 (strong indicator).
    pub severity: f64,
    pub evidence: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ForgeryReport {
    pub flags: Vec<ForgeryFlag>,
    pub sample_count: usize,
}

impl ForgeryReport {
    /// Each flag contributes its severity weighted by 0.3, clamped to [0, 1].
    pub fn suspicion_score(&self) -> f64 {
        if self.flags.is_empty() {
            return 0.0;
        }
        let raw = python_sum(self.flags.iter().map(|flag| flag.severity * 0.3));
        raw.min(1.0)
    }

    fn render(&self) -> Map<String, Value> {
        let mut rendered = Map::new();
        rendered.insert(
            "suspicion_score".to_owned(),
            json!(python_round(self.suspicion_score(), 3)),
        );
        rendered.insert("sample_count".to_owned(), json!(self.sample_count));
        let flags: Vec<Value> = self
            .flags
            .iter()
            .map(|flag| {
                json!({
                    "name": flag.name,
                    "description": flag.description,
                    "severity": flag.severity,
                    "evidence": flag.evidence,
                })
            })
            .collect();
        rendered.insert("flags".to_owned(), Value::Array(flags));
        rendered
    }
}

/// CPython 3.12+ `sum()` of floats: Neumaier compensated summation.
fn python_sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut total = 0.0_f64;
    let mut compensation = 0.0_f64;
    for value in values {
        let next = total + value;
        if total.abs() >= value.abs() {
            compensation += (total - next) + value;
        } else {
            compensation += (value - next) + total;
        }
        total = next;
    }
    if compensation != 0.0 && compensation.is_finite() {
        total += compensation;
    }
    total
}

fn mean_std(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let n = values.len() as f64;
    let mean = python_sum(values.iter().copied()) / n;
    let variance = python_sum(values.iter().map(|value| (value - mean).powf(2.0))) / n;
    (mean, variance.sqrt())
}

/// `_finite_number`: a JSON int or float that is not a bool, inf or NaN.
fn finite_number(value: Option<&Value>) -> Option<f64> {
    let Value::Number(number) = value? else {
        return None;
    };
    number.as_f64().filter(|number| number.is_finite())
}

/// Python `int(float)`, truncating toward zero.
fn truncate(value: f64) -> i128 {
    value.trunc() as i128
}

/// A JSON value that Python's `isinstance(v, int)` accepts (bool excluded),
/// saturated to `i128` for integers wider than that.
fn python_int(value: Option<&Value>) -> Option<i128> {
    let Value::Number(number) = value? else {
        return None;
    };
    let text = number.to_string();
    if text.contains(['.', 'e', 'E']) {
        return None;
    }
    Some(text.parse::<i128>().unwrap_or(if text.starts_with('-') {
        i128::MIN
    } else {
        i128::MAX
    }))
}

fn ratio_percent(ratio: f64, digits: usize) -> String {
    format!("{:.digits$}%", ratio * 100.0)
}

// ----------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AudioStreamAnalyzer {
    min_windows: usize,
    rms_values: Vec<f64>,
    zcr_values: Vec<f64>,
    centroid_values: Vec<f64>,
    transition_intervals_ms: Vec<f64>,
    last_transition_ms: Option<i128>,
}

impl AudioStreamAnalyzer {
    pub fn new(min_windows: usize) -> Self {
        AudioStreamAnalyzer {
            min_windows,
            rms_values: Vec::new(),
            zcr_values: Vec::new(),
            centroid_values: Vec::new(),
            transition_intervals_ms: Vec::new(),
            last_transition_ms: None,
        }
    }

    pub fn ingest_buffer_hash(&mut self, event: &Value) {
        if let Some(rms) = finite_number(event.get("rms_level")) {
            self.rms_values.push(rms);
        }
        if let Some(zcr) = finite_number(event.get("zero_crossing_rate")) {
            self.zcr_values.push(zcr);
        }
        if let Some(centroid) = finite_number(event.get("spectral_centroid_hz")) {
            self.centroid_values.push(centroid);
        }
    }

    pub fn ingest_transition(&mut self, event: &Value) {
        let Some(timestamp) = finite_number(event.get("timestamp_ms")) else {
            return;
        };
        let timestamp = truncate(timestamp);
        if let Some(last) = self.last_transition_ms {
            let interval = timestamp - last;
            if 0 < interval && interval < 300_000 {
                self.transition_intervals_ms.push(interval as f64);
            }
        }
        self.last_transition_ms = Some(timestamp);
    }

    pub fn analyze(&self) -> ForgeryReport {
        let mut flags = Vec::new();
        if self.rms_values.len() >= self.min_windows {
            flags.extend(self.check_rms_regularity());
            flags.extend(self.check_spectral_regularity());
        }
        if self.transition_intervals_ms.len() >= 10 {
            flags.extend(self.check_transition_regularity());
            flags.extend(self.check_impossible_timing());
        }
        ForgeryReport {
            flags,
            sample_count: self.rms_values.len(),
        }
    }

    fn check_rms_regularity(&self) -> Option<ForgeryFlag> {
        let (mean, std) = mean_std(&self.rms_values);
        if mean > 0.0 {
            let cv = std / mean;
            if cv < 0.05 {
                return Some(ForgeryFlag {
                    name: "too_regular_rms",
                    description: "RMS level is unusually consistent across windows",
                    severity: 0.7,
                    evidence: format!("CV={cv:.4} (threshold: 0.05)"),
                });
            }
        }
        None
    }

    fn check_spectral_regularity(&self) -> Option<ForgeryFlag> {
        if self.centroid_values.len() < self.min_windows {
            return None;
        }
        let (mean, std) = mean_std(&self.centroid_values);
        if mean > 0.0 {
            let cv = std / mean;
            if cv < 0.02 {
                return Some(ForgeryFlag {
                    name: "too_regular_spectrum",
                    description: "Spectral centroid is unusually consistent",
                    severity: 0.6,
                    evidence: format!("CV={cv:.4} (threshold: 0.02)"),
                });
            }
        }
        None
    }

    fn check_transition_regularity(&self) -> Option<ForgeryFlag> {
        let (mean, std) = mean_std(&self.transition_intervals_ms);
        if mean > 0.0 {
            let cv = std / mean;
            if cv < 0.1 {
                return Some(ForgeryFlag {
                    name: "metronomic_transitions",
                    description: "Play/stop timing is suspiciously regular",
                    severity: 0.8,
                    evidence: format!("CV={cv:.4}, mean_interval={mean:.0}ms (threshold: 0.1)"),
                });
            }
        }
        None
    }

    fn check_impossible_timing(&self) -> Option<ForgeryFlag> {
        let fast_count = self.transition_intervals_ms.iter().filter(|t| **t < 100.0).count();
        let total = self.transition_intervals_ms.len();
        if total > 0 {
            let fast_ratio = fast_count as f64 / total as f64;
            if fast_ratio > 0.10 {
                return Some(ForgeryFlag {
                    name: "superhuman_speed",
                    description: "Many transitions are faster than human reaction time",
                    severity: 0.9,
                    evidence: format!(
                        "{fast_count}/{total} transitions < 100ms ({})",
                        ratio_percent(fast_ratio, 0)
                    ),
                });
            }
        }
        None
    }
}

// ----------------------------------------------------------------------------

/// Cap on the repeats one keystroke batch may contribute, matching Python.
const MAX_BATCH_REPEAT: i128 = 10_000;

#[derive(Debug, Clone)]
pub struct InputBehaviorAnalyzer {
    min_samples: usize,
    iki_ms: Vec<f64>,
}

impl InputBehaviorAnalyzer {
    pub fn new(min_samples: usize) -> Self {
        InputBehaviorAnalyzer {
            min_samples,
            iki_ms: Vec::new(),
        }
    }

    pub fn ingest_keystroke_batch(&mut self, event: &Value) {
        let Some(mean_iki) = finite_number(event.get("mean_iki_ms")) else {
            return;
        };
        // Python: `event.get("count", 0)`, so an absent count is 0 and a
        // present non-int (float, string, null, bool) skips the batch.
        let count = match event.get("count") {
            None => 0,
            Some(value) => match python_int(Some(value)) {
                Some(count) => count,
                None => return,
            },
        };
        let repeats = count.clamp(0, MAX_BATCH_REPEAT);
        for _ in 0..repeats {
            self.iki_ms.push(mean_iki);
        }
    }

    pub fn analyze(&self) -> ForgeryReport {
        if self.iki_ms.len() < self.min_samples {
            return ForgeryReport {
                flags: Vec::new(),
                sample_count: self.iki_ms.len(),
            };
        }
        let mut flags = Vec::new();
        flags.extend(self.check_regularity());
        flags.extend(self.check_missing_pauses());
        flags.extend(self.check_superhuman_speed());
        flags.extend(self.check_fatigue());
        flags.extend(self.check_skewness());
        ForgeryReport {
            flags,
            sample_count: self.iki_ms.len(),
        }
    }

    fn check_regularity(&self) -> Option<ForgeryFlag> {
        let (mean, std) = mean_std(&self.iki_ms);
        if mean > 0.0 {
            let cv = std / mean;
            if cv < 0.2 {
                return Some(ForgeryFlag {
                    name: "too_regular_iki",
                    description: "Inter-key intervals are unusually consistent",
                    severity: 0.7,
                    evidence: format!("CV={cv:.4} (threshold: 0.2)"),
                });
            }
        }
        None
    }

    fn check_missing_pauses(&self) -> Option<ForgeryFlag> {
        let pause_count = self.iki_ms.iter().filter(|t| 150.0 <= **t && **t <= 500.0).count();
        let total = self.iki_ms.len();
        if total > 0 {
            let ratio = pause_count as f64 / total as f64;
            if ratio < 0.05 {
                return Some(ForgeryFlag {
                    name: "missing_human_pauses",
                    description: "Very few micro-pauses detected between actions",
                    severity: 0.6,
                    evidence: format!(
                        "{pause_count}/{total} in 150-500ms range ({})",
                        ratio_percent(ratio, 1)
                    ),
                });
            }
        }
        None
    }

    fn check_superhuman_speed(&self) -> Option<ForgeryFlag> {
        let fast = self.iki_ms.iter().filter(|t| **t < 20.0).count();
        let total = self.iki_ms.len();
        if total > 0 {
            let ratio = fast as f64 / total as f64;
            if ratio > 0.10 {
                return Some(ForgeryFlag {
                    name: "superhuman_input_speed",
                    description: "Many keystrokes are faster than human capability",
                    severity: 0.9,
                    evidence: format!(
                        "{fast}/{total} intervals < 20ms ({})",
                        ratio_percent(ratio, 0)
                    ),
                });
            }
        }
        None
    }

    fn check_fatigue(&self) -> Option<ForgeryFlag> {
        let n = self.iki_ms.len();
        if n < 100 {
            return None;
        }
        let quarter = n / 4;
        let (first_mean, _) = mean_std(self.iki_ms.get(..quarter).unwrap_or_default());
        let (last_mean, _) = mean_std(self.iki_ms.get(n - quarter..).unwrap_or_default());
        if first_mean > 0.0 {
            let slowdown = (last_mean - first_mean) / first_mean;
            if slowdown < 0.01 {
                return Some(ForgeryFlag {
                    name: "no_fatigue_pattern",
                    description: "No slowdown detected over session duration",
                    severity: 0.5,
                    evidence: format!(
                        "First quarter mean: {first_mean:.1}ms, last quarter: {last_mean:.1}ms, change: {:+.1}%",
                        slowdown * 100.0
                    ),
                });
            }
        }
        None
    }

    fn check_skewness(&self) -> Option<ForgeryFlag> {
        let n = self.iki_ms.len();
        if n < 30 {
            return None;
        }
        let (mean, std) = mean_std(&self.iki_ms);
        if std <= 0.0 {
            return None;
        }
        let skew = python_sum(self.iki_ms.iter().map(|x| ((x - mean) / std).powf(3.0))) / n as f64;
        if skew < 0.5 {
            return Some(ForgeryFlag {
                name: "wrong_iki_statistics",
                description: "IKI distribution shape does not match human patterns",
                severity: 0.6,
                evidence: format!("skewness={skew:.3} (expected > 1.0 for human input)"),
            });
        }
        None
    }
}

// ----------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct HashChainAnalyzer {
    hashes: Vec<String>,
    prev_hashes: Vec<String>,
    timestamps: Vec<i128>,
}

impl HashChainAnalyzer {
    pub fn new() -> Self {
        HashChainAnalyzer::default()
    }

    pub fn ingest_buffer_hash(&mut self, event: &Value) {
        if let Some(hash) = event.get("window_hash").and_then(Value::as_str) {
            self.hashes.push(hash.to_owned());
        }
        if let Some(previous) = event.get("prev_hash").and_then(Value::as_str) {
            self.prev_hashes.push(previous.to_owned());
        }
        if let Some(timestamp) = finite_number(event.get("timestamp_ms")) {
            self.timestamps.push(truncate(timestamp));
        }
    }

    pub fn analyze(&self) -> ForgeryReport {
        let mut flags = Vec::new();
        flags.extend(self.check_chain_continuity());
        flags.extend(self.check_duplicate_hashes());
        flags.extend(self.check_timestamp_monotonicity());
        ForgeryReport {
            flags,
            sample_count: self.hashes.len(),
        }
    }

    fn check_chain_continuity(&self) -> Option<ForgeryFlag> {
        let limit = self.hashes.len().min(self.prev_hashes.len());
        let breaks = (1..limit)
            .filter(|index| self.prev_hashes.get(*index) != self.hashes.get(index - 1))
            .count();
        (breaks > 0).then(|| ForgeryFlag {
            name: "chain_break",
            description: "Hash chain has discontinuities",
            severity: 1.0,
            evidence: format!("{breaks} break(s) in {} windows", self.hashes.len()),
        })
    }

    fn check_duplicate_hashes(&self) -> Option<ForgeryFlag> {
        let mut seen = std::collections::HashSet::new();
        let duplicates = self.hashes.iter().filter(|hash| !seen.insert(hash.as_str())).count();
        (duplicates > 0).then(|| ForgeryFlag {
            name: "duplicate_hashes",
            description: "Hash chain contains duplicate window hashes (possible replay)",
            severity: 0.9,
            evidence: format!("{duplicates} duplicate(s) in {} windows", self.hashes.len()),
        })
    }

    fn check_timestamp_monotonicity(&self) -> Option<ForgeryFlag> {
        let reversals = self
            .timestamps
            .windows(2)
            .filter(|pair| matches!(pair, [earlier, later] if later < earlier))
            .count();
        (reversals > 0).then(|| ForgeryFlag {
            name: "timestamp_reversal",
            description: "Timestamps are not monotonically increasing",
            severity: 1.0,
            evidence: format!("{reversals} reversal(s) in {} events", self.timestamps.len()),
        })
    }
}

// ----------------------------------------------------------------------------

const DEFAULT_MIN_WINDOWS: usize = 50;
const DEFAULT_MIN_SAMPLES: usize = 100;
const NO_INPUT_NOTE: &str = "The input_capture layer is not active in this session; \
no keystroke statistics were available to analyze.";
const SCREEN_SCOPE: &str = "Statistical screening of the routed-audio stream and hash chain for \
synthetic or scripted patterns. An empty flag list is not proof of \
authenticity; a flag is a lead, not a verdict.";

/// `derive_forgery_analysis`: the `forgery_analysis` manifest section.
pub fn derive_forgery_analysis(events: &[Value]) -> Value {
    let mut audio = AudioStreamAnalyzer::new(DEFAULT_MIN_WINDOWS);
    let mut chain = HashChainAnalyzer::new();
    let mut input = InputBehaviorAnalyzer::new(DEFAULT_MIN_SAMPLES);
    for event in events {
        match event.get("event_type").and_then(Value::as_str) {
            Some("buffer_hash") => {
                audio.ingest_buffer_hash(event);
                chain.ingest_buffer_hash(event);
            }
            Some("audio_transition") => audio.ingest_transition(event),
            Some("input_keystroke_stats") => input.ingest_keystroke_batch(event),
            _ => {}
        }
    }
    let audio_report = audio.analyze();
    let chain_report = chain.analyze();
    let input_report = input.analyze();
    let mut input_rendered = input_report.render();
    if input_report.sample_count == 0 {
        input_rendered.insert("note".to_owned(), json!(NO_INPUT_NOTE));
    }
    let score = audio_report
        .suspicion_score()
        .max(chain_report.suspicion_score())
        .max(input_report.suspicion_score());
    let mut analyzers = Map::new();
    analyzers.insert("audio_stream".to_owned(), Value::Object(audio_report.render()));
    analyzers.insert("hash_chain".to_owned(), Value::Object(chain_report.render()));
    analyzers.insert("input_behavior".to_owned(), Value::Object(input_rendered));

    let mut section = Map::new();
    section.insert(PROOF_LEVEL_KEY.to_owned(), json!(ProofLevel::Inferred.as_str()));
    section.insert("method".to_owned(), json!("statistical_screen_v1"));
    section.insert("scope".to_owned(), json!(SCREEN_SCOPE));
    section.insert("suspicion_score".to_owned(), json!(python_round(score, 3)));
    section.insert("analyzers".to_owned(), Value::Object(analyzers));
    Value::Object(section)
}

/// The compiled-in statistical screen.
pub struct StatisticalForgeryScreen;

impl ForgeryAnalyzer for StatisticalForgeryScreen {
    fn analyze(&self, events: &[Value]) -> Value {
        derive_forgery_analysis(events)
    }
}
