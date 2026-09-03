use std::path::Path;

use apw_core::is_truthy;
use serde_json::Value;

use crate::extract::extract_feature_sequence;
use crate::feature::Feature;
use crate::record::{AssociationRecord, MAX_FEATURE_WINDOWS};
use crate::similarity::compare_feature_sequences;

const MISSING_METADATA: &str =
    "routed events lack comparable feature, sample-rate, or window metadata";
/// IMPORTANT: divergence from the oracle, deliberately. `associate_export`
/// calls `int()` on whatever the event carries, so a non-numeric sample rate
/// raises out of the daemon; these values arrive over UDP, so an unavailable
/// record is returned instead of a panic.
const NON_NUMERIC_METADATA: &str =
    "routed events carry a non-numeric sample rate or window size";

/// Compare an export against the routed windows of a capture session.
///
/// Never fails: an unreadable or unsupported export produces an `unavailable`
/// record, which records the absence of a comparison and is not evidence that
/// the routed audio is absent from the export.
pub fn associate_export(export_path: &Path, routed_events: &[Value]) -> AssociationRecord {
    let mut routed_features: Vec<Feature> = Vec::new();
    let mut sample_rate: i64 = 0;
    let mut window_size: i64 = 0;

    for event in routed_events {
        if event.get("event_type").and_then(Value::as_str) != Some("buffer_hash") {
            continue;
        }
        if let (Some(rms), Some(zcr)) = (
            python_number(event.get("rms_level")),
            python_number(event.get("zero_crossing_rate")),
        ) {
            routed_features.push(Feature {
                rms: Some(rms),
                zcr: Some(zcr),
                crest: python_number(event.get("crest_factor")),
                envelope: routed_envelope(event.get("energy_envelope")),
            });
        }
        let (Some(rate), Some(size)) = (
            python_int_or(event.get("sample_rate_hz"), sample_rate),
            python_int_or(event.get("window_size_samples"), window_size),
        ) else {
            return AssociationRecord::unavailable(NON_NUMERIC_METADATA);
        };
        sample_rate = rate;
        window_size = size;
    }

    if routed_features.is_empty() || sample_rate <= 0 || window_size <= 0 {
        return AssociationRecord::unavailable(MISSING_METADATA);
    }
    let window_seconds = window_size as f64 / sample_rate as f64;
    match extract_feature_sequence(export_path, window_seconds, MAX_FEATURE_WINDOWS) {
        Err(failure) => AssociationRecord::unavailable(failure.to_string()),
        Ok((export_features, details)) => {
            let mut record =
                compare_feature_sequences(&routed_features, &export_features, window_seconds);
            record.extraction = Some(details);
            record
        }
    }
}

/// `isinstance(value, (int, float))`, which in Python accepts `bool`.
fn python_number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        Value::Number(number) => number.as_f64(),
        _ => None,
    }
}

/// The envelope axis is all-or-nothing: the oracle appends it only if every
/// element converts, so one unconvertible element drops the whole axis.
fn routed_envelope(value: Option<&Value>) -> Option<Vec<f64>> {
    let Some(Value::Array(values)) = value else {
        return None;
    };
    if values.len() != 4 {
        return None;
    }
    values.iter().map(|item| python_number(Some(item))).collect()
}

/// `int(value or fallback)`. `None` means the oracle would have raised.
fn python_int_or(value: Option<&Value>, fallback: i64) -> Option<i64> {
    let Some(value) = value else {
        return Some(fallback);
    };
    if !is_truthy(value) {
        return Some(fallback);
    }
    match value {
        Value::Bool(_) => Some(1),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => Some(integer),
            None => {
                let float = number.as_f64()?;
                if float.is_finite() && float.abs() < 9.223_372_036_854_775e18 {
                    Some(float.trunc() as i64)
                } else {
                    None
                }
            }
        },
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}
