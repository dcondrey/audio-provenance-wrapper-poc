//! The signed `observation_coverage` record, byte for byte against the Python
//! daemon. Fixtures come from `fixtures/generate_coverage_fixtures.py`, which
//! drives `daemon.manifest_builder.generator.derive_coverage` itself; no
//! expected byte here was written by hand.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use apw_daemon::{derive_coverage, CoverageInputs, ReceiverDiagnostics};

fn clean_diagnostics() -> ReceiverDiagnostics {
    ReceiverDiagnostics {
        packets_received: 3,
        events_received: 3,
        acknowledgements_attempted: 3,
        acknowledgements_sent: 3,
        ..ReceiverDiagnostics::default()
    }
}

fn full_telemetry() -> Vec<(String, i64)> {
    [
        ("buffers_submitted", 10),
        ("samples_submitted", 100),
        ("windows_hashed", 3),
        ("fifo_samples_dropped", 0),
        ("fifo_windows_dropped", 0),
        ("midi_events_dropped", 0),
        ("bypassed_buffers", 0),
        ("bypassed_samples", 0),
        ("events_prepared", 3),
        ("udp_sends_attempted", 3),
        ("udp_sends_failed", 0),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value))
    .collect()
}

fn oracle() -> BTreeMap<String, String> {
    let raw = include_str!("fixtures/coverage_oracle.json");
    serde_json::from_str(raw).expect("coverage oracle fixture")
}

fn rendered(inputs: &CoverageInputs) -> String {
    // Compact insertion-ordered JSON: `Value` equality ignores key order, and the
    // counter order is part of what the signature commits to.
    serde_json::to_string(&derive_coverage(inputs)).expect("render coverage")
}

#[test]
fn coverage_records_match_the_python_generator_byte_for_byte() {
    let oracle = oracle();
    let telemetry = full_telemetry();
    let clean = clean_diagnostics();

    assert_eq!(
        rendered(&CoverageInputs {
            telemetry: &telemetry,
            receiver: &clean,
            plugin_instance_count: 1,
            chain_length: 3,
            feature_window_drops: 0,
            telemetry_regressions: 0,
        }),
        oracle["complete"]
    );

    assert_eq!(
        rendered(&CoverageInputs {
            telemetry: &telemetry,
            receiver: &clean,
            plugin_instance_count: 1,
            chain_length: 0,
            feature_window_drops: 0,
            telemetry_regressions: 0,
        }),
        oracle["no_chain"]
    );

    let partial_telemetry: Vec<(String, i64)> = telemetry
        .iter()
        .filter(|(name, _)| name != "midi_events_dropped")
        .cloned()
        .collect();
    assert_eq!(
        rendered(&CoverageInputs {
            telemetry: &partial_telemetry,
            receiver: &clean,
            plugin_instance_count: 1,
            chain_length: 3,
            feature_window_drops: 2,
            telemetry_regressions: 0,
        }),
        oracle["missing_counter"]
    );

    let evicted = ReceiverDiagnostics {
        stream_evictions: 1,
        ..clean_diagnostics()
    };
    assert_eq!(
        rendered(&CoverageInputs {
            telemetry: &telemetry,
            receiver: &evicted,
            plugin_instance_count: 1,
            chain_length: 3,
            feature_window_drops: 0,
            telemetry_regressions: 0,
        }),
        oracle["stream_evicted"],
        "an evicted stream can never grade complete"
    );

    assert_eq!(
        rendered(&CoverageInputs {
            telemetry: &telemetry,
            receiver: &clean,
            plugin_instance_count: 2,
            chain_length: 3,
            feature_window_drops: 0,
            telemetry_regressions: 0,
        }),
        oracle["two_plugin_instances"],
        "a second plug-in instance means the observed path is not the only path"
    );

    for (name, telemetry, regressions) in [
        ("telemetry_regressed", full_telemetry(), 1),
        (
            "bypassed_buffers",
            full_telemetry()
                .into_iter()
                .map(|(name, value)| if name == "bypassed_buffers" { (name, 2) } else { (name, value) })
                .collect(),
            0,
        ),
        (
            "missing_bypassed_counter",
            full_telemetry().into_iter().filter(|(name, _)| name != "bypassed_samples").collect(),
            0,
        ),
    ] {
        assert_eq!(
            rendered(&CoverageInputs {
                telemetry: &telemetry,
                receiver: &clean,
                plugin_instance_count: 1,
                chain_length: 3,
                feature_window_drops: 0,
                telemetry_regressions: regressions,
            }),
            oracle[name],
            "{name}"
        );
    }
}
