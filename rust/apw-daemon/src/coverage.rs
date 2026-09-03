use apw_core::{ProofLevel, PROOF_LEVEL_KEY};
use serde_json::{json, Map, Value};

use crate::receiver::ReceiverDiagnostics;

/// Cumulative plug-in counters that must all be present before coverage can be
/// graded at all. A build that omits one cannot be graded complete, only unknown.
const REQUIRED_TELEMETRY: [&str; 9] = [
    "buffers_submitted",
    "samples_submitted",
    "windows_hashed",
    "fifo_samples_dropped",
    "fifo_windows_dropped",
    "midi_events_dropped",
    "events_prepared",
    "udp_sends_attempted",
    "udp_sends_failed",
];

const COMPLETE_BASIS: &str =
    "All submitted routed windows represented by the plug-in counters were received, \
     and the prepared/received event prefix agrees with no reported FIFO loss, UDP send \
     failure, sequence gap, chain break, or daemon acknowledgement dispatch failure. \
     Scope ends at the last received telemetry event; plug-in processing of each ACK is not \
     observable from the daemon.";

const PARTIAL_BASIS: &str =
    "Routed audio was observed, but one or more counters show or cannot exclude loss.";

pub struct CoverageInputs<'a> {
    pub telemetry: &'a [(String, i64)],
    pub receiver: &'a ReceiverDiagnostics,
    pub plugin_instance_count: usize,
    pub chain_length: u64,
    pub feature_window_drops: u64,
}

/// The signed `observation_coverage` record.
///
/// IMPORTANT: `complete_observed_path` is claimed only when every counter agrees.
/// Any gap, drop, eviction, missing sequence or undelivered acknowledgement grades
/// partial, and a missing counter grades unknown; the status is never upgraded by
/// the absence of evidence.
pub fn derive_coverage(inputs: &CoverageInputs) -> Value {
    let telemetry = |key: &str| -> Option<i64> {
        inputs
            .telemetry
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| *value)
    };
    let telemetry_or_zero = |key: &str| telemetry(key).unwrap_or(0);

    let mut counters = Map::new();
    for (key, value) in inputs.telemetry {
        counters.insert(key.clone(), json!(value));
    }
    if let Value::Object(receiver) = inputs.receiver.to_json() {
        for (key, value) in receiver {
            counters.insert(key, value);
        }
    }
    counters.insert(
        "udp_sends_locally_emitted".to_owned(),
        json!(telemetry_or_zero("udp_sends_attempted")
            .saturating_sub(telemetry_or_zero("udp_sends_failed"))
            .max(0)),
    );
    counters.insert(
        "buffer_hash_events_received".to_owned(),
        json!(inputs.chain_length),
    );
    counters.insert(
        "feature_windows_dropped_from_alignment_buffer".to_owned(),
        json!(inputs.feature_window_drops),
    );

    let missing_counter = REQUIRED_TELEMETRY
        .iter()
        .any(|required| telemetry(required).is_none());
    if inputs.chain_length == 0 || missing_counter {
        return json!({
            "status": apw_core::CoverageStatus::UnknownCoverage.as_str(),
            "basis": if inputs.chain_length == 0 {
                "No routed hash windows were received."
            } else {
                "The plug-in stream did not include every required cumulative counter."
            },
            "counters": Value::Object(counters),
            PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
        });
    }

    let receiver = inputs.receiver;
    let loss_count: i64 = [
        telemetry_or_zero("fifo_samples_dropped"),
        telemetry_or_zero("fifo_windows_dropped"),
        telemetry_or_zero("midi_events_dropped"),
        // Optional: older plug-in builds omit it, so it is not in REQUIRED_TELEMETRY.
        telemetry_or_zero("midi_unsupported_dropped"),
        telemetry_or_zero("udp_sends_failed"),
        receiver.sequence_gaps as i64,
        receiver.sequence_out_of_order as i64,
        receiver.hash_chain_breaks as i64,
        receiver.stream_evictions as i64,
    ]
    .iter()
    .fold(0_i64, |total, value| total.saturating_add(*value));

    let complete = telemetry("windows_hashed") == Some(inputs.chain_length as i64)
        && telemetry("events_prepared") == Some(receiver.events_received as i64)
        && loss_count == 0
        && receiver.events_missing_sequence == 0
        && receiver.acknowledgements_sent == receiver.packets_received
        && receiver.acknowledgements_failed == 0
        && inputs.plugin_instance_count == 1;

    let status = if complete {
        apw_core::CoverageStatus::CompleteObservedPath
    } else {
        apw_core::CoverageStatus::PartialObservedPath
    };
    json!({
        "status": status.as_str(),
        "basis": if complete { COMPLETE_BASIS } else { PARTIAL_BASIS },
        "counters": Value::Object(counters),
        PROOF_LEVEL_KEY: ProofLevel::Inferred.as_str(),
    })
}
