use serde::{Deserialize, Serialize};

use crate::error::NeuralWatermarkError;

/// Every decision boundary the detector applies, frozen in the model card.
///
/// IMPORTANT: none of these is a call-time argument anywhere in the public API. Spec 3.5 and kill
/// criterion K5a make the false-positive rate the constraint and the recall the result; a threshold
/// a caller can move at detection time is a threshold that gets moved to recover recall, which is
/// exactly the failure the whole program is written against.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Thresholds {
    /// Accept boundary on the pooled presence score, which is the mean of `sigmoid(p[t])` over the
    /// window, so it lives in (0, 1).
    pub presence_accept_score: f64,
    /// Per-frame gate for message pooling: frames whose smoothed `sigmoid(p[t])` sits below this
    /// do not count toward the presence-positive span (spec 4.5).
    pub presence_frame_gate: f64,
    pub frame_gate_median_seconds: f64,
    pub presence_window_seconds: f64,
    pub presence_window_hop_seconds: f64,
    pub min_presence_seconds: f64,
    pub locator_window_seconds: f64,
    pub locator_window_hop_seconds: f64,
    pub min_locator_seconds: f64,
    /// A window whose presence-positive span is under this does not attempt a message read.
    pub min_presence_span_seconds: f64,
    /// Design maximum on the sliding-window multiplicity that spec 3.4 counts the false-accept
    /// budget over. Raising it raises the per-file false-accept rate proportionally.
    pub max_windows: usize,
}

fn positive(field: &'static str, value: f64) -> Result<(), NeuralWatermarkError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(NeuralWatermarkError::ThresholdRange {
            field,
            found: value,
            range: "finite and greater than zero",
        });
    }
    Ok(())
}

fn unit_open(field: &'static str, value: f64) -> Result<(), NeuralWatermarkError> {
    if !value.is_finite() || value <= 0.0 || value >= 1.0 {
        return Err(NeuralWatermarkError::ThresholdRange {
            field,
            found: value,
            range: "the open interval (0, 1)",
        });
    }
    Ok(())
}

impl Thresholds {
    pub fn validate(&self) -> Result<(), NeuralWatermarkError> {
        unit_open("presence_accept_score", self.presence_accept_score)?;
        unit_open("presence_frame_gate", self.presence_frame_gate)?;
        positive("frame_gate_median_seconds", self.frame_gate_median_seconds)?;
        positive("presence_window_seconds", self.presence_window_seconds)?;
        positive(
            "presence_window_hop_seconds",
            self.presence_window_hop_seconds,
        )?;
        positive("min_presence_seconds", self.min_presence_seconds)?;
        positive("locator_window_seconds", self.locator_window_seconds)?;
        positive(
            "locator_window_hop_seconds",
            self.locator_window_hop_seconds,
        )?;
        positive("min_locator_seconds", self.min_locator_seconds)?;
        positive("min_presence_span_seconds", self.min_presence_span_seconds)?;
        if self.max_windows == 0 || self.max_windows > 64 {
            return Err(NeuralWatermarkError::ThresholdRange {
                field: "max_windows",
                found: self.max_windows as f64,
                range: "1..=64, the multiplicity spec 3.4 budgets",
            });
        }
        if self.min_locator_seconds < self.min_presence_span_seconds {
            return Err(NeuralWatermarkError::ThresholdRange {
                field: "min_locator_seconds",
                found: self.min_locator_seconds,
                range: "at least min_presence_span_seconds",
            });
        }
        if self.presence_window_hop_seconds > self.presence_window_seconds
            || self.locator_window_hop_seconds > self.locator_window_seconds
        {
            return Err(NeuralWatermarkError::ThresholdRange {
                field: "window hop",
                found: self.locator_window_hop_seconds,
                range: "no larger than its window length",
            });
        }
        Ok(())
    }
}
