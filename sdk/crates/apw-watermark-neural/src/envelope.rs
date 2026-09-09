use serde::{Deserialize, Serialize};

use crate::error::NeuralWatermarkError;
use crate::params::{
    MAX_DURATION_S, MAX_PRESENCE_FALSE_POSITIVE_RATE, MEASURED_LIMITED, MIN_BLIND_DETECTION_RATE,
    MIN_CALIBRATION_TRIALS, MIN_DISTANCE_M, MIN_MICROPHONES, MIN_PRESENCE_RECALL, MIN_ROOMS,
    MIN_SPEAKERS,
};

/// The unvalidated record as it appears in a model card. Deserialising one asserts nothing.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct EnvelopeClaim {
    pub max_distance_m: f64,
    pub min_duration_s: f64,
    pub rooms_measured: u32,
    pub speakers_measured: u32,
    pub microphones_measured: u32,
    /// Blind, CRC-gated locator recovery on the physical corpus. Never a mean bit accuracy, and
    /// never a best-of-offset-search maximum scored against known bits (spec 11).
    pub blind_detection_rate: f64,
    /// Established on the SIMULATED calibration arm, because a physical campaign yields hundreds
    /// of unmarked captures and cannot bound 1e-3 (spec 12.2).
    pub false_positive_rate: f64,
    pub false_positive_trials: u64,
    pub presence_recall: f64,
    pub unmarked_physical_accepts: u64,
    pub unmarked_physical_trials: u64,
}

/// A validated acoustic operating envelope.
///
/// IMPORTANT: this type has no public constructor and no public field mutation, and the crate-
/// internal `EnvelopeClaim::validate` that produces one is not reachable from outside this crate.
/// So the only route to one is loading a model card whose envelope passes every numeric gate in
/// WATERMARK_N_SPEC.md section 12.3. That is what makes an untrained or uncalibrated build
/// STRUCTURALLY incapable of reporting acoustic re-recording as anything but `"unsupported"`:
/// [`crate::capabilities::AcousticRerecording::MeasuredLimited`] cannot be named without one.
#[derive(Debug, Clone, PartialEq)]
pub struct AcousticEnvelope {
    max_distance_m: f64,
    min_duration_s: f64,
    rooms_measured: u32,
    speakers_measured: u32,
    microphones_measured: u32,
    blind_detection_rate: f64,
    false_positive_rate: f64,
    false_positive_trials: u64,
    presence_recall: f64,
    unmarked_physical_trials: u64,
}

impl AcousticEnvelope {
    pub const fn status(&self) -> &'static str {
        MEASURED_LIMITED
    }

    pub const fn max_distance_m(&self) -> f64 {
        self.max_distance_m
    }

    pub const fn min_duration_s(&self) -> f64 {
        self.min_duration_s
    }

    pub const fn rooms_measured(&self) -> u32 {
        self.rooms_measured
    }

    pub const fn speakers_measured(&self) -> u32 {
        self.speakers_measured
    }

    pub const fn microphones_measured(&self) -> u32 {
        self.microphones_measured
    }

    pub const fn blind_detection_rate(&self) -> f64 {
        self.blind_detection_rate
    }

    pub const fn false_positive_rate(&self) -> f64 {
        self.false_positive_rate
    }

    pub const fn false_positive_trials(&self) -> u64 {
        self.false_positive_trials
    }

    pub const fn presence_recall(&self) -> f64 {
        self.presence_recall
    }

    /// The bound the physical arm actually supports: zero accepts over N trials bounds the rate at
    /// roughly 3/N, not at zero (spec 12.2). A presence rate quoted without its N is not a result.
    pub fn physical_false_positive_bound(&self) -> f64 {
        if self.unmarked_physical_trials == 0 {
            return 1.0;
        }
        3.0 / self.unmarked_physical_trials as f64
    }

    pub const fn unmarked_physical_trials(&self) -> u64 {
        self.unmarked_physical_trials
    }
}

fn reject(reason: impl Into<String>) -> NeuralWatermarkError {
    NeuralWatermarkError::EnvelopeRejected {
        reason: reason.into(),
    }
}

impl EnvelopeClaim {
    /// Every gate here is a kill criterion from WATERMARK_N_SPEC.md section 12.3. Failing any one of
    /// them leaves the capability at the literal `"unsupported"`; there is no partial credit and no
    /// aggregate score.
    pub(crate) fn validate(&self) -> Result<AcousticEnvelope, NeuralWatermarkError> {
        for (field, value) in [
            ("max_distance_m", self.max_distance_m),
            ("min_duration_s", self.min_duration_s),
            ("blind_detection_rate", self.blind_detection_rate),
            ("false_positive_rate", self.false_positive_rate),
            ("presence_recall", self.presence_recall),
        ] {
            if !value.is_finite() {
                return Err(reject(format!("{field} is not finite")));
            }
        }
        for (field, value) in [
            ("blind_detection_rate", self.blind_detection_rate),
            ("false_positive_rate", self.false_positive_rate),
            ("presence_recall", self.presence_recall),
        ] {
            if !(0.0..=1.0).contains(&value) {
                return Err(reject(format!("{field} = {value} is not a rate in 0..=1")));
            }
        }
        if self.false_positive_trials < MIN_CALIBRATION_TRIALS {
            return Err(reject(format!(
                "K5a: {} unmarked calibration trials, under the {MIN_CALIBRATION_TRIALS} the \
                 1e-3 operating point needs",
                self.false_positive_trials
            )));
        }
        if self.false_positive_rate > MAX_PRESENCE_FALSE_POSITIVE_RATE {
            return Err(reject(format!(
                "K5a: false positive rate {} exceeds {MAX_PRESENCE_FALSE_POSITIVE_RATE}; the FPR \
                 is the constraint and the recall is the result",
                self.false_positive_rate
            )));
        }
        if self.presence_recall < MIN_PRESENCE_RECALL {
            return Err(reject(format!(
                "K5: presence recall {} is under {MIN_PRESENCE_RECALL} at the frozen threshold",
                self.presence_recall
            )));
        }
        if self.blind_detection_rate < MIN_BLIND_DETECTION_RATE {
            return Err(reject(format!(
                "K4: blind CRC-gated recovery {} is under {MIN_BLIND_DETECTION_RATE}",
                self.blind_detection_rate
            )));
        }
        if self.unmarked_physical_trials == 0 {
            return Err(reject(
                "K5b: the physical false-positive arm is empty; it cannot be synthesised later",
            ));
        }
        if self.unmarked_physical_accepts != 0 {
            return Err(reject(format!(
                "K5b: {} accepts on the unmarked physical corpus; zero is required",
                self.unmarked_physical_accepts
            )));
        }
        if self.max_distance_m < MIN_DISTANCE_M {
            return Err(reject(format!(
                "K6: {} m is under {MIN_DISTANCE_M} m; that is a coupling test, not re-recording",
                self.max_distance_m
            )));
        }
        if self.min_duration_s <= 0.0 || self.min_duration_s > MAX_DURATION_S {
            return Err(reject(format!(
                "K8: {} s of capture is outside the 0 to {MAX_DURATION_S} s range a realistic \
                 capture covers",
                self.min_duration_s
            )));
        }
        if self.rooms_measured < MIN_ROOMS
            || self.speakers_measured < MIN_SPEAKERS
            || self.microphones_measured < MIN_MICROPHONES
        {
            return Err(reject(format!(
                "K4: {} rooms / {} speakers / {} microphones is under the {MIN_ROOMS} / \
                 {MIN_SPEAKERS} / {MIN_MICROPHONES} grid the gate is stated over",
                self.rooms_measured, self.speakers_measured, self.microphones_measured
            )));
        }
        Ok(AcousticEnvelope {
            max_distance_m: self.max_distance_m,
            min_duration_s: self.min_duration_s,
            rooms_measured: self.rooms_measured,
            speakers_measured: self.speakers_measured,
            microphones_measured: self.microphones_measured,
            blind_detection_rate: self.blind_detection_rate,
            false_positive_rate: self.false_positive_rate,
            false_positive_trials: self.false_positive_trials,
            presence_recall: self.presence_recall,
            unmarked_physical_trials: self.unmarked_physical_trials,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing() -> EnvelopeClaim {
        EnvelopeClaim {
            max_distance_m: 1.0,
            min_duration_s: 30.0,
            rooms_measured: 3,
            speakers_measured: 2,
            microphones_measured: 2,
            blind_detection_rate: 0.55,
            false_positive_rate: 5e-4,
            false_positive_trials: 5_000,
            presence_recall: 0.82,
            unmarked_physical_accepts: 0,
            unmarked_physical_trials: 300,
        }
    }

    /// The one gate that can actually reject the tier (K5a), and the one that would let a
    /// physical campaign pretend to a precision it cannot deliver (K5b). Both must bite.
    #[test]
    fn rejects_a_recall_recovered_by_moving_the_threshold_and_an_unbounded_physical_arm() {
        assert!(passing().validate().is_ok());

        let mut loose = passing();
        loose.false_positive_rate = 2e-3;
        loose.presence_recall = 0.97;
        assert!(loose.validate().is_err(), "K5a: the FPR is the constraint");

        let mut thin = passing();
        thin.false_positive_trials = 400;
        assert!(thin.validate().is_err(), "K5a needs >= 5000 trials");

        let mut leaking = passing();
        leaking.unmarked_physical_accepts = 1;
        assert!(leaking.validate().is_err(), "K5b requires zero accepts");

        let mut close = passing();
        close.max_distance_m = 0.15;
        assert!(close.validate().is_err(), "K6: that is a coupling test");
    }

    /// 3/N with N stated, never zero. A presence rate quoted without its N is not a result.
    #[test]
    fn publishes_the_physical_bound_as_three_over_n() {
        let envelope = passing().validate().unwrap();
        assert!((envelope.physical_false_positive_bound() - 0.01).abs() < 1e-12);
        assert_eq!(envelope.unmarked_physical_trials(), 300);
    }
}
