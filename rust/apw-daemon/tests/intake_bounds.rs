//! The three intake invariants that actually bite: a flood cannot grow memory
//! without bound, a partially written export is never hashed, and hostile socket
//! input is rejected instead of panicking.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use apw_daemon::{
    CorrelationEngine, Delay, EvidenceReceiver, ExportWatcher, LayerEvent, SessionState,
    StabilityPolicy, MAX_SESSION_EVENTS, MAX_TRACKED_STREAMS,
};
use serde_json::{json, Value};

fn buffer_hash_event(instance: &str, sequence: i64, prev: &str, window: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "event_type": "buffer_hash",
        "proof_level": "directly_observed",
        "plugin_instance_id": instance,
        "plugin_capture_session_id": "plugin-session",
        "event_sequence": sequence,
        "timestamp_ms": 1_000 + sequence,
        "window_hash": window,
        "prev_hash": prev,
        "rms_level": 0.25,
        "zero_crossing_rate": 0.1,
        "sample_rate_hz": 48_000,
        "window_size_samples": 4_096,
    }))
    .unwrap_or_default()
}

#[test]
fn a_stream_flood_cannot_grow_receiver_or_session_memory_without_bound() {
    let evidence = tempfile::tempdir().expect("tempdir");
    let receiver = EvidenceReceiver::bind(
        "127.0.0.1",
        0,
        &evidence.path().join("plugin_events.jsonl"),
        Some("capture-test".to_owned()),
        Some("stem-1".to_owned()),
    )
    .expect("bind");

    // Every packet invents a new sender-controlled stream key, which is the whole
    // flood vector: the table must evict, not grow.
    for index in 0..5_000 {
        let datagram = buffer_hash_event(&format!("instance-{index}"), 1, "genesis", "aa");
        let outcome = receiver.process_packet(&datagram);
        assert!(outcome.event.is_some(), "packet {index} should be accepted");
    }

    assert_eq!(receiver.tracked_stream_count(), MAX_TRACKED_STREAMS);
    let diagnostics = receiver.diagnostics();
    assert_eq!(diagnostics.packets_received, 5_000);
    assert_eq!(diagnostics.stream_evictions, 5_000 - MAX_TRACKED_STREAMS as u64);
    let receipt = receiver.receipt_summary();
    assert_eq!(receipt["status"], json!("degraded"));
    assert_eq!(receipt[apw_core::PROOF_LEVEL_KEY], json!("unknown_unobserved"));
    assert_eq!(
        receipt["streams"].as_array().map(Vec::len),
        Some(MAX_TRACKED_STREAMS)
    );

    let mut session = SessionState::new();
    for index in 0..(MAX_SESSION_EVENTS + 2_000) {
        session.record_plugin_event(
            &json!({
                "event_type": "buffer_hash",
                "plugin_instance_id": format!("instance-{index}"),
                "window_hash": "aa",
                "telemetry": {format!("counter-{index}"): index},
            }),
            "audio_buffer",
        );
    }
    assert_eq!(session.events_retained(), MAX_SESSION_EVENTS);
    assert_eq!(session.events_dropped(), 2_000);
    assert_eq!(
        session.plugin_instance_count(),
        apw_daemon::MAX_TRACKED_PLUGIN_KEYS
    );
    assert_eq!(
        session.telemetry().len(),
        apw_daemon::MAX_TRACKED_PLUGIN_KEYS
    );
    let snapshot = session.snapshot();
    assert_eq!(snapshot.feature_events.len(), apw_daemon::MAX_FEATURE_WINDOWS);
    assert!(snapshot.feature_window_drops > 0);

    let correlation = CorrelationEngine::new(2_000, &evidence.path().join("composite.jsonl"));
    for index in 0..5_000_i64 {
        correlation.ingest(LayerEvent::new(
            "audio_buffer",
            "buffer_hash",
            index,
            json!({"event_type": "buffer_hash"}),
        ));
    }
    let correlation_diagnostics = correlation.diagnostics();
    assert!(correlation_diagnostics.buffer_events <= correlation_diagnostics.max_buffer_events);

    session.reset();
    assert_eq!(session.events_retained(), 0);
    assert_eq!(session.chain_length(), 0);
}

/// Appends to the export on the first `growing_waits` stability samples, exactly
/// as a DAW still writing the render would.
struct WritingDelay {
    path: std::path::PathBuf,
    waits: Mutex<u32>,
    growing_waits: u32,
}

impl Delay for WritingDelay {
    fn wait(&self, _duration: Duration) {
        let Ok(mut waits) = self.waits.lock() else {
            return;
        };
        *waits += 1;
        if *waits <= self.growing_waits {
            append_bytes(&self.path, 4_096);
        }
    }
}

fn append_bytes(path: &Path, count: usize) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open export");
    file.write_all(&vec![0x5a_u8; count]).expect("append");
    file.flush().expect("flush");
    // APFS timestamps are nanosecond-resolution, but a same-nanosecond append is
    // still caught by the size half of the signature.
}

#[test]
fn a_partially_written_export_is_never_offered_for_hashing() {
    let exports = tempfile::tempdir().expect("tempdir");
    let export_path = exports.path().join("take.wav");
    let mut watcher = ExportWatcher::new(
        exports.path(),
        StabilityPolicy {
            checks: 3,
            interval: Duration::from_millis(0),
        },
    );
    watcher.mark_existing_seen();

    // A zero-byte file that never grows is stable and still must not be sealed.
    std::fs::write(&export_path, b"").expect("create export");
    let idle = WritingDelay {
        path: export_path.clone(),
        waits: Mutex::new(0),
        growing_waits: 0,
    };
    assert!(
        watcher.detect(&idle).is_empty(),
        "an empty export must never be detected"
    );

    append_bytes(&export_path, 4_096);
    let writing = WritingDelay {
        path: export_path.clone(),
        waits: Mutex::new(0),
        growing_waits: 3,
    };
    assert!(
        watcher.detect(&writing).is_empty(),
        "an export that is still growing must not be detected"
    );
    let size_while_writing = std::fs::metadata(&export_path).map(|m| m.len()).unwrap_or(0);

    let settled = WritingDelay {
        path: export_path.clone(),
        waits: Mutex::new(0),
        growing_waits: 0,
    };
    let detected = watcher.detect(&settled);
    assert_eq!(detected.len(), 1, "a settled export must be detected");
    let export = detected.first().expect("detected export");
    assert_eq!(export.version, 1);
    assert_eq!(
        std::fs::metadata(&export.path).map(|m| m.len()).unwrap_or(0),
        size_while_writing,
        "the detected file is the complete one, not a prefix"
    );

    watcher.commit(export);
    assert!(
        watcher.detect(&settled).is_empty(),
        "a committed export must not re-seal on the next poll"
    );

    append_bytes(&export_path, 8);
    let rerendered = watcher.detect(&settled);
    assert_eq!(
        rerendered.first().map(|export| export.version),
        Some(2),
        "a re-render of the same path is version 2"
    );
}

#[test]
fn hostile_datagrams_are_rejected_with_the_python_verifier_wording() {
    // Every expected reason below was produced by running the datagram through
    // `daemon.evidence_receiver.taxonomy.validate_network_event`, not by reading
    // the Rust that answers it.
    let evidence = tempfile::tempdir().expect("tempdir");
    let receiver = EvidenceReceiver::bind(
        "127.0.0.1",
        0,
        &evidence.path().join("plugin_events.jsonl"),
        None,
        None,
    )
    .expect("bind");

    let mut deep = String::new();
    for _ in 0..10_000 {
        deep.push('[');
    }
    let oversize_string = "a".repeat(60_000);

    let corpus: Vec<(Vec<u8>, &str)> = vec![
        (vec![0xff, 0xfe, 0xfd], "invalid JSON/UTF-8 object"),
        (b"[]".to_vec(), "invalid JSON/UTF-8 object"),
        (b"{".to_vec(), "invalid JSON/UTF-8 object"),
        (b"".to_vec(), "invalid JSON/UTF-8 object"),
        (deep.into_bytes(), "invalid JSON/UTF-8 object"),
        (b"{}".to_vec(), "Unknown event type: None"),
        (
            br#"{"event_type":"nope","proof_level":"directly_observed"}"#.to_vec(),
            "Unknown event type: nope",
        ),
        // Verified divergence: Python raises `TypeError: unhashable type: 'dict'`
        // inside `validate_event`, which `process_packet_with_ack` does not catch,
        // so the datagram is dropped with no acknowledgement and no rejection
        // counted. Rejecting it is the fix, not a parity break.
        (
            br#"{"event_type":{"nested":1},"proof_level":"directly_observed"}"#.to_vec(),
            "Unknown event type: {'nested': 1}",
        ),
        (
            br#"{"event_type":"buffer_hash","proof_level":"absolute"}"#.to_vec(),
            "Unknown proof level: absolute",
        ),
        (
            br#"{"event_type":"buffer_hash","proof_level":"directly_observed","window_hash":"a","rms_level":0.1,"zero_crossing_rate":0.1}"#.to_vec(),
            "Missing required field 'prev_hash' for buffer_hash",
        ),
        (
            br#"{"event_type":"buffer_hash","proof_level":"externally_verified","window_hash":"a","prev_hash":"genesis","rms_level":0.1,"zero_crossing_rate":0.1}"#.to_vec(),
            "Proof level 'externally_verified' exceeds network cap 'directly_observed' for buffer_hash",
        ),
        (
            br#"{"event_type":"composite_edit","proof_level":"inferred","edit_type":"undo","confidence":0.9,"contributing_events":[]}"#.to_vec(),
            "Event type not accepted from the network: composite_edit (daemon-origin event types must not arrive over UDP)",
        ),
        (
            br#"{"event_type":"buffer_hash","proof_level":"directly_observed","window_hash":"a","prev_hash":"genesis","rms_level":"loud","zero_crossing_rate":0.1}"#.to_vec(),
            "Field 'rms_level' must be a finite number for buffer_hash",
        ),
        (
            br#"{"event_type":"buffer_hash","proof_level":"directly_observed","window_hash":"a","prev_hash":"genesis","rms_level":0.1,"zero_crossing_rate":0.1,"energy_envelope":[1,2,"x",4]}"#.to_vec(),
            "Field 'energy_envelope' must be a list of finite numbers",
        ),
        (
            format!(
                r#"{{"event_type":"buffer_hash","proof_level":"directly_observed","window_hash":"{oversize_string}","rms_level":0.1,"zero_crossing_rate":0.1}}"#
            )
            .into_bytes(),
            "Missing required field 'prev_hash' for buffer_hash",
        ),
    ];

    for (datagram, expected_reason) in &corpus {
        let outcome = receiver.process_packet(datagram);
        assert!(
            outcome.event.is_none(),
            "datagram must be rejected: {expected_reason}"
        );
        let ack = &outcome.acknowledgement;
        assert_eq!(ack["accepted"], json!(false));
        assert_eq!(ack["receipt_state"], json!("rejected_invalid"));
        assert_eq!(ack["reason"], json!(expected_reason));
        assert_eq!(ack["protocol"], json!(apw_daemon::ACK_PROTOCOL));
        // The acknowledgement is emitted as canonical JSON on the socket; a
        // rejection that cannot be encoded would be a silent drop.
        assert!(apw_core::canonical_json_utf8(ack).is_ok());
    }

    let diagnostics = receiver.diagnostics();
    assert_eq!(diagnostics.events_rejected, corpus.len() as u64);
    assert_eq!(diagnostics.events_received, 0);
    assert_eq!(receiver.tracked_stream_count(), 0);
}

#[test]
fn an_out_of_order_or_broken_chain_degrades_the_receipt_instead_of_being_accepted() {
    let evidence = tempfile::tempdir().expect("tempdir");
    let receiver = EvidenceReceiver::bind(
        "127.0.0.1",
        0,
        &evidence.path().join("plugin_events.jsonl"),
        None,
        None,
    )
    .expect("bind");

    assert!(receiver
        .process_packet(&buffer_hash_event("instance", 1, "genesis", "w1"))
        .event
        .is_some());
    // A window whose prev_hash does not continue the chain this receiver saw.
    let broken = receiver.process_packet(&buffer_hash_event("instance", 2, "forged", "w2"));
    assert!(broken.event.is_some(), "the event is kept, but flagged");
    assert_eq!(broken.acknowledgement["receipt_state"], json!("accepted_chain_break"));
    // A replayed sequence is refused outright.
    let replay = receiver.process_packet(&buffer_hash_event("instance", 2, "w2", "w3"));
    assert!(replay.event.is_none());
    assert_eq!(
        replay.acknowledgement["receipt_state"],
        json!("rejected_duplicate_or_out_of_order")
    );
    // A gap is accepted but counted.
    let gap = receiver.process_packet(&buffer_hash_event("instance", 9, "w2", "w9"));
    assert_eq!(gap.acknowledgement["receipt_state"], json!("accepted_with_gap"));

    let diagnostics = receiver.diagnostics();
    assert_eq!(diagnostics.hash_chain_breaks, 1);
    assert_eq!(diagnostics.sequence_out_of_order, 1);
    assert_eq!(diagnostics.sequence_gaps, 6);
    let receipt = receiver.receipt_summary();
    assert_eq!(receipt["status"], json!("degraded"));
    let stream: &Value = &receipt["streams"][0];
    assert_eq!(stream["highest_accepted_sequence"], json!(9));
    assert_eq!(stream["highest_contiguous_sequence"], json!(2));
    assert_eq!(stream["chain_breaks"], json!(1));
    assert_eq!(stream["rejections"], json!(1));
}
