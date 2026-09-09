//! The measured false-positive rate behind a soft binding, and the only way to obtain one.
//!
//! [`FalsePositiveRate`] has a private field and no public constructor. The single mint site is
//! [`NullTestTable::rate_for`], which reads a `audio-provenance-bench` report produced by the null-test arm
//! over never-marked audio. [`crate::result::BindingEvaluation::SoftMark`] cannot be built without
//! one, and that variant is the only soft path to `verified`, so soft-binding `verified` is
//! unreachable until the bench has actually run. That is the forcing function the specs ask for,
//! expressed in the type system rather than in a review checklist.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::error::TraceError;

pub const BENCH_REPORT_SCHEMA: &str = "audio-provenance-bench/1";

/// A measured P(accept | never-marked audio).
///
/// Not `Copy`-constructible from a float anywhere outside this module. Do not add `From<f64>`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FalsePositiveRate {
    value: f64,
    basis: RateBasis,
}

/// Whether the number is an observation or a bound on an observation of zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RateBasis {
    /// At least one false accept was seen; the value is accepts/trials.
    Observed,
    /// Zero false accepts were seen. A reported 0.0 would be a claim the trial count cannot
    /// support, so the rule-of-three 95% upper bound is reported instead.
    UpperBound95,
}

impl RateBasis {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::UpperBound95 => "upper_bound_95",
        }
    }
}

impl FalsePositiveRate {
    pub const fn value(self) -> f64 {
        self.value
    }

    pub const fn rate_basis(self) -> RateBasis {
        self.basis
    }
}

/// Null-test results keyed by algorithm id.
#[derive(Debug, Clone, Default)]
pub struct NullTestTable {
    rows: BTreeMap<String, FalsePositiveRate>,
}

impl NullTestTable {
    /// An empty table. Every soft binding priced against it comes back unpriced, which forces
    /// `soft_binding_false_positive_rate_unknown` rather than a `verified`.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Reads one `audio-provenance-bench` JSON report.
    ///
    /// A bench fixture codec (the deliberately weak reference marks the bench uses to prove itself
    /// honest) is refused: its null-test rate says nothing about Watermark.
    pub fn from_bench_report(json: &[u8]) -> Result<Self, TraceError> {
        let report: Value =
            serde_json::from_slice(json).map_err(|error| TraceError::NullTestMalformed {
                reason: error.to_string(),
            })?;

        let schema = report.get("schema").and_then(Value::as_str).unwrap_or("");
        if schema != BENCH_REPORT_SCHEMA {
            return Err(TraceError::NullTestMalformed {
                reason: format!("expected schema {BENCH_REPORT_SCHEMA}, found {schema:?}"),
            });
        }

        let codec = report
            .get("codec")
            .ok_or_else(|| TraceError::NullTestMalformed {
                reason: "report carries no codec block".to_string(),
            })?;
        if codec
            .get("is_bench_fixture")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            return Err(TraceError::NullTestFixtureCodec);
        }
        let algorithm = codec.get("name").and_then(Value::as_str).ok_or_else(|| {
            TraceError::NullTestMalformed {
                reason: "codec block carries no name".to_string(),
            }
        })?;

        let totals = report
            .get("totals")
            .ok_or_else(|| TraceError::NullTestMalformed {
                reason: "report carries no totals block".to_string(),
            })?;
        let trials = totals
            .get("false_positive_trials")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if trials == 0 {
            return Err(TraceError::NullTestNotRun);
        }
        let accepts = totals
            .get("false_positive_accepts")
            .and_then(Value::as_u64)
            .unwrap_or(0);

        let rate = if accepts == 0 {
            let bound = totals
                .get("false_positive_upper_bound_95")
                .and_then(Value::as_f64)
                .ok_or(TraceError::NullTestNotRun)?;
            FalsePositiveRate {
                value: bound,
                basis: RateBasis::UpperBound95,
            }
        } else {
            let observed = totals
                .get("overall_false_positive_rate")
                .and_then(Value::as_f64)
                .ok_or(TraceError::NullTestNotRun)?;
            FalsePositiveRate {
                value: observed,
                basis: RateBasis::Observed,
            }
        };
        if !rate.value.is_finite() || rate.value < 0.0 || rate.value > 1.0 {
            return Err(TraceError::NullTestMalformed {
                reason: format!("false-positive rate {} is not a probability", rate.value),
            });
        }

        let mut rows = BTreeMap::new();
        rows.insert(algorithm.to_string(), rate);
        Ok(Self { rows })
    }

    /// Folds another report in. The stricter number wins, so combining campaigns can only raise the
    /// stated risk.
    pub fn merge(&mut self, other: Self) {
        for (algorithm, rate) in other.rows {
            self.rows
                .entry(algorithm)
                .and_modify(|existing| {
                    if rate.value > existing.value {
                        *existing = rate;
                    }
                })
                .or_insert(rate);
        }
    }

    pub fn rate_for(&self, algorithm: &str) -> Option<FalsePositiveRate> {
        self.rows.get(algorithm).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}
