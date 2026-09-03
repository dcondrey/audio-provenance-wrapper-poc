//! A session reset has to restart every counter that coverage grades against
//! another. Restarting one side alone silently caps every later export at
//! `partial_observed_path`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::net::UdpSocket;

use apw_daemon::{Daemon, DaemonConfig, DaemonServices, SourceCategory};
use serde_json::json;

fn routed_packet(sequence: i64, prev: &str, window: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "event_type": "buffer_hash",
        "proof_level": "directly_observed",
        "plugin_instance_id": "instance-a",
        "plugin_capture_session_id": "plugin-session",
        "event_sequence": sequence,
        "timestamp_ms": 1_000 + sequence,
        "window_hash": window,
        "prev_hash": prev,
        "rms_level": 0.25,
        "zero_crossing_rate": 0.1,
        "sample_rate_hz": 48_000,
        "window_size_samples": 4_096,
        "telemetry": {
            "buffers_submitted": sequence,
            "samples_submitted": sequence * 4_096,
            "windows_hashed": sequence,
            "fifo_samples_dropped": 0,
            "fifo_windows_dropped": 0,
            "midi_events_dropped": 0,
            "events_prepared": sequence,
            "udp_sends_attempted": sequence,
            "udp_sends_failed": 0,
        },
    }))
    .unwrap_or_default()
}

#[test]
fn a_reset_take_can_still_grade_complete() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let plugin = UdpSocket::bind("127.0.0.1:0").expect("plugin socket");
    let plugin_address = plugin.local_addr().expect("plugin address");

    let daemon = Daemon::new(
        DaemonConfig {
            udp_port: 0,
            evidence_dir: workspace.path().join("evidence"),
            manifest_dir: workspace.path().join("manifests"),
            sample_dir: workspace.path().join("samples"),
            session_id: Some("capture-test".to_owned()),
            source_category: SourceCategory::MidiVstSynth,
            ..DaemonConfig::default()
        },
        DaemonServices::default(),
    )
    .expect("daemon");

    let mut previous = "genesis".to_owned();
    for sequence in 1..=3_i64 {
        let window = format!("w{sequence}");
        let outcome = daemon.accept_datagram(&routed_packet(sequence, &previous, &window));
        assert!(outcome.event.is_some());
        assert!(daemon
            .receiver()
            .send_acknowledgement(plugin_address, &outcome.acknowledgement));
        previous = window;
    }

    assert_eq!(
        daemon.coverage()["status"],
        json!("complete_observed_path"),
        "a clean first take grades complete"
    );

    daemon.reset_session_state();
    let diagnostics = daemon.receiver().diagnostics();
    assert_eq!(diagnostics.events_received, 0);
    assert_eq!(diagnostics.packets_received, 0);
    assert_eq!(diagnostics.acknowledgements_sent, 0);
    assert_eq!(daemon.receiver().tracked_stream_count(), 0);
    assert_eq!(daemon.coverage()["status"], json!("unknown_coverage"));

    // The plug-in was re-added, so its cumulative counters restart with the take.
    let mut previous = "genesis".to_owned();
    for sequence in 1..=2_i64 {
        let window = format!("v{sequence}");
        let outcome = daemon.accept_datagram(&routed_packet(sequence, &previous, &window));
        assert!(outcome.event.is_some());
        assert!(daemon
            .receiver()
            .send_acknowledgement(plugin_address, &outcome.acknowledgement));
        previous = window;
    }

    let coverage = daemon.coverage();
    assert_eq!(
        coverage["status"],
        json!("complete_observed_path"),
        "the second take grades on its own counters, not the first take's: {coverage}"
    );
    assert_eq!(coverage["counters"]["buffer_hash_events_received"], json!(2));
    assert_eq!(coverage["counters"]["events_received"], json!(2));
}
