import json
import tempfile
import threading
import unittest
from pathlib import Path

from daemon.evidence_receiver.taxonomy import (
    validate_event,
    validate_network_event,
)
from daemon.evidence_receiver.receiver import EvidenceReceiver
from daemon.evidence_receiver.correlation import SampleCorrelator


class TaxonomyTests(unittest.TestCase):
    VALID = {
        "buffer_hash": {
            "timestamp_ms": 12345, "window_hash": "abc123", "prev_hash": "genesis",
            "rms_level": 0.042, "zero_crossing_rate": 0.15,
        },
        "transport_change": {"transport_state": "playing"},
        "midi_event": {"midi_event_type": "note_on", "midi_channel": 1},
        "audio_transition": {"direction": "silence_to_audio", "boundary_hash": "deadbeef"},
        "spectral_shift": {"prev_spectral_centroid_hz": 1200.0, "new_spectral_centroid_hz": 2400.0},
    }

    def test_each_event_type_validates_with_its_required_fields(self):
        for event_type, fields in self.VALID.items():
            with self.subTest(event_type):
                valid, error = validate_event({"event_type": event_type, "proof_level": "directly_observed", **fields})
                self.assertTrue(valid, error)

    def test_rejects_missing_required_field(self):
        fields = {k: v for k, v in self.VALID["buffer_hash"].items() if k != "window_hash"}
        valid, error = validate_event({"event_type": "buffer_hash", "proof_level": "directly_observed", **fields})
        self.assertFalse(valid)
        self.assertIn("window_hash", error)

    def test_rejects_unknown_event_type_and_proof_level(self):
        for event in (
            {"event_type": "bogus", "proof_level": "directly_observed"},
            {"event_type": "buffer_hash", "proof_level": "bogus"},
        ):
            with self.subTest(event):
                self.assertFalse(validate_event(event)[0])

    def test_network_rejects_daemon_origin_event_type(self):
        event = {
            "event_type": "sample_file_observed",
            "proof_level": "directly_observed",
            "sha256": "ab" * 32,
            "file_name": "kick.wav",
        }
        self.assertTrue(validate_event(event)[0])
        valid, error = validate_network_event(event)
        self.assertFalse(valid)
        self.assertIn("not accepted from the network", error)

    def test_network_rejects_proof_level_above_cap(self):
        event = {
            "event_type": "buffer_hash",
            "proof_level": "externally_verified",
            "window_hash": "abc123",
            "prev_hash": "genesis",
            "rms_level": 0.042,
            "zero_crossing_rate": 0.15,
        }
        self.assertTrue(validate_event(event)[0])
        valid, error = validate_network_event(event)
        self.assertFalse(valid)
        self.assertIn("exceeds network cap", error)

    def test_network_accepts_plugin_event_at_cap(self):
        event = {
            "event_type": "buffer_hash",
            "proof_level": "directly_observed",
            "window_hash": "abc123",
            "prev_hash": "genesis",
            "rms_level": 0.042,
            "zero_crossing_rate": 0.15,
        }
        valid, error = validate_network_event(event)
        self.assertTrue(valid, error)

    def _host_event(self, **overrides):
        event = {
            "event_type": "host_environment",
            "proof_level": "directly_observed",
            "host_recognised": True,
            "host_name": "Ableton Live",
            "host_executable_name": "Live",
            "wrapper_format": "VST3",
        }
        event.update(overrides)
        return event

    def test_network_accepts_host_environment(self):
        valid, error = validate_network_event(self._host_event())
        self.assertTrue(valid, error)

    def test_host_name_may_be_null_when_the_host_is_unrecognised(self):
        valid, error = validate_network_event(
            self._host_event(host_recognised=False, host_name=None)
        )
        self.assertTrue(valid, error)

    def test_required_host_text_field_may_not_be_null(self):
        valid, error = validate_network_event(self._host_event(wrapper_format=None))
        self.assertFalse(valid)
        self.assertIn("wrapper_format", error)

    def test_host_text_field_length_boundary(self):
        for length, expected in ((127, True), (128, True), (129, False)):
            with self.subTest(length=length):
                valid, _ = validate_network_event(self._host_event(host_name="a" * length))
                self.assertIs(valid, expected)

    def test_host_recognised_must_be_a_boolean(self):
        for value in ("true", 1, None):
            with self.subTest(value=value):
                valid, error = validate_network_event(self._host_event(host_recognised=value))
                self.assertFalse(valid)
                self.assertIn("host_recognised", error)

    def test_host_name_must_be_a_string(self):
        valid, error = validate_network_event(self._host_event(host_name=42))
        self.assertFalse(valid)
        self.assertIn("host_name", error)


class ReceiverTests(unittest.TestCase):
    def test_process_valid_packet(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            evidence_path = Path(tmp_dir) / "events.jsonl"
            receiver = EvidenceReceiver(
                host="127.0.0.1", port=0, evidence_path=evidence_path,
                capture_session_id="capture-test", stem_id="stem-test",
            )
            self.addCleanup(receiver.close)

            event_json = json.dumps({
                "event_type": "buffer_hash",
                "proof_level": "directly_observed",
                "window_hash": "deadbeef",
                "prev_hash": "genesis",
                "rms_level": 0.042,
                "zero_crossing_rate": 0.15,
            })

            result = receiver.process_packet(event_json.encode("utf-8"))

            self.assertIsNotNone(result)
            self.assertEqual(result["event_type"], "buffer_hash")
            self.assertIn("received_at", result)
            self.assertIn("received_at_ms", result)
            self.assertEqual(result["capture_session_id"], "capture-test")
            self.assertEqual(result["stem_id"], "stem-test")

            lines = evidence_path.read_text().splitlines()
            self.assertEqual(len(lines), 1)
            written = json.loads(lines[0])
            self.assertEqual(written["window_hash"], "deadbeef")

    def test_process_invalid_json(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            evidence_path = Path(tmp_dir) / "events.jsonl"
            receiver = EvidenceReceiver(
                host="127.0.0.1", port=0, evidence_path=evidence_path
            )
            self.addCleanup(receiver.close)

            result = receiver.process_packet(b"not json at all")
            self.assertIsNone(result)
            self.assertFalse(evidence_path.exists())

    def test_process_deeply_nested_json_rejected_not_crashed(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            evidence_path = Path(tmp_dir) / "events.jsonl"
            receiver = EvidenceReceiver(
                host="127.0.0.1", port=0, evidence_path=evidence_path
            )
            self.addCleanup(receiver.close)

            # json.loads raises RecursionError at ~10k nesting levels; the
            # boundary must reject the datagram like any other malformed input.
            result = receiver.process_packet(b"[" * 20000)
            self.assertIsNone(result)
            self.assertEqual(receiver.rejected_count, 1)
            self.assertFalse(evidence_path.exists())

    def test_process_invalid_event(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            evidence_path = Path(tmp_dir) / "events.jsonl"
            receiver = EvidenceReceiver(
                host="127.0.0.1", port=0, evidence_path=evidence_path
            )
            self.addCleanup(receiver.close)

            event_json = json.dumps({"event_type": "bogus", "proof_level": "directly_observed"})
            result = receiver.process_packet(event_json.encode("utf-8"))
            self.assertIsNone(result)

    def test_stream_table_evicts_without_locking_out_the_live_stream(self):
        from daemon.evidence_receiver.receiver import MAX_TRACKED_STREAMS

        with tempfile.TemporaryDirectory() as tmp_dir:
            receiver = EvidenceReceiver(
                host="127.0.0.1", port=0,
                evidence_path=Path(tmp_dir) / "events.jsonl",
            )
            self.addCleanup(receiver.close)

            def packet(instance: str) -> bytes:
                return json.dumps({
                    "event_type": "transport_change",
                    "proof_level": "directly_observed",
                    "transport_state": "playing",
                    "plugin_instance_id": instance,
                }).encode()

            # One live subject stream, then a flood of one-per-reinstantiation
            # stale keys far exceeding the cap. The subject must never be refused.
            self.assertIsNotNone(receiver.process_packet(packet("subject")))
            for i in range(MAX_TRACKED_STREAMS * 2):
                self.assertIsNotNone(receiver.process_packet(packet(f"stale-{i}")))
                self.assertIsNotNone(receiver.process_packet(packet("subject")))

            self.assertLessEqual(len(receiver._stream_states), MAX_TRACKED_STREAMS)
            self.assertGreater(receiver.stream_evictions, 0)
            # Recency keeps the actively-sending subject resident.
            self.assertIn(
                ("subject", "unknown_plugin_capture_session"), receiver._stream_states
            )
            summary = receiver.receipt_summary()
            # Dropped streams mean the receiver cannot vouch for a complete view.
            self.assertEqual(summary["status"], "degraded")
            self.assertEqual(summary["apw:proof_level"], "unknown_unobserved")

    def test_stream_cap_boundary_demotes_at_exactly_cap_plus_one(self):
        from daemon.evidence_receiver.receiver import MAX_TRACKED_STREAMS

        with tempfile.TemporaryDirectory() as tmp_dir:
            receiver = EvidenceReceiver(
                host="127.0.0.1", port=0,
                evidence_path=Path(tmp_dir) / "events.jsonl",
            )
            self.addCleanup(receiver.close)

            def send(instance: str) -> None:
                self.assertIsNotNone(receiver.process_packet(json.dumps({
                    "event_type": "transport_change",
                    "proof_level": "directly_observed",
                    "transport_state": "playing",
                    "plugin_instance_id": instance,
                }).encode()))

            def clean_summary() -> dict[str, object]:
                # Isolate the eviction predicate: receipt_summary also degrades
                # on unsent acknowledgements, which process_packet never sends.
                receiver.acknowledgements_sent = receiver.packet_count
                return receiver.receipt_summary()

            for i in range(MAX_TRACKED_STREAMS - 1):
                send(f"stream-{i}")
            self.assertEqual(receiver.stream_evictions, 0)

            send(f"stream-{MAX_TRACKED_STREAMS - 1}")
            self.assertEqual(receiver.stream_evictions, 0)
            summary = clean_summary()
            self.assertEqual(summary["status"], "issued")
            self.assertEqual(summary["apw:proof_level"], "directly_observed")

            send("one-over-cap")
            self.assertEqual(receiver.stream_evictions, 1)
            self.assertEqual(len(receiver._stream_states), MAX_TRACKED_STREAMS)
            summary = clean_summary()
            self.assertEqual(summary["status"], "degraded")
            self.assertEqual(summary["apw:proof_level"], "unknown_unobserved")

    def test_concurrent_streams_and_reader_do_not_race(self):
        # The udp-receiver thread mutates the stream table (move_to_end/popitem)
        # per packet while status/export/diagnostics threads iterate it via
        # receipt_summary(). Drive enough distinct streams to force eviction
        # while a reader hammers the summary.
        from daemon.evidence_receiver.receiver import MAX_TRACKED_STREAMS

        with tempfile.TemporaryDirectory() as tmp_dir:
            receiver = EvidenceReceiver(
                host="127.0.0.1", port=0,
                evidence_path=Path(tmp_dir) / "events.jsonl",
            )
            self.addCleanup(receiver.close)
            errors: list[BaseException] = []
            stop = threading.Event()
            barrier = threading.Barrier(5)

            def sender(sender_id: int) -> None:
                try:
                    barrier.wait()
                    for i in range(140):
                        receiver.process_packet(json.dumps({
                            "event_type": "buffer_hash",
                            "proof_level": "directly_observed",
                            "plugin_instance_id": f"s{sender_id}-{i}",
                            "window_hash": f"w{sender_id}-{i}",
                            "prev_hash": "genesis",
                            "rms_level": 0.1,
                            "zero_crossing_rate": 0.1,
                        }).encode())
                except BaseException as exc:  # pragma: no cover
                    errors.append(exc)

            def reader() -> None:
                try:
                    barrier.wait()
                    while not stop.is_set():
                        summary = receiver.receipt_summary()
                        self.assertIn("status", summary)
                        self.assertIsInstance(summary["streams"], list)
                except BaseException as exc:  # pragma: no cover
                    errors.append(exc)

            senders = [threading.Thread(target=sender, args=(s,)) for s in range(4)]
            reader_thread = threading.Thread(target=reader)
            reader_thread.start()
            for t in senders:
                t.start()
            for t in senders:
                t.join()
            stop.set()
            reader_thread.join()

            self.assertEqual(errors, [], f"receiver raced: {errors[:3]}")
            self.assertLessEqual(len(receiver._stream_states), MAX_TRACKED_STREAMS)
            self.assertGreater(receiver.stream_evictions, 0)

    def test_non_genesis_chain_start_is_flagged(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            receiver = EvidenceReceiver(
                host="127.0.0.1", port=0,
                evidence_path=Path(tmp_dir) / "events.jsonl",
            )
            self.addCleanup(receiver.close)

            event, ack = receiver.process_packet_with_ack(json.dumps({
                "event_type": "buffer_hash",
                "proof_level": "directly_observed",
                "window_hash": "abc",
                "prev_hash": "SPLICED_UNKNOWN",
                "rms_level": 0.1,
                "zero_crossing_rate": 0.1,
            }).encode())
            self.assertIsNotNone(event)
            self.assertEqual(ack["receipt_state"], "accepted_chain_unknown")
            self.assertEqual(receiver.hash_chain_break_count, 1)


class CorrelationTests(unittest.TestCase):
    def test_matching_fingerprint(self):
        correlator = SampleCorrelator(tolerance_rms=0.1, tolerance_zcr=0.15)
        correlator.register_sample("abc123", {"rms": 0.05, "zero_crossing_rate": 0.2})

        matches = correlator.check_correlation(
            {"rms_level": 0.06, "zero_crossing_rate": 0.22}
        )
        self.assertEqual(len(matches), 1)
        self.assertEqual(matches[0]["sample_sha256"], "abc123")
        self.assertGreater(matches[0]["confidence"], 0.5)

    def test_no_match_when_features_differ(self):
        correlator = SampleCorrelator(tolerance_rms=0.1, tolerance_zcr=0.15)
        correlator.register_sample("abc123", {"rms": 0.5, "zero_crossing_rate": 0.8})

        matches = correlator.check_correlation(
            {"rms_level": 0.01, "zero_crossing_rate": 0.1}
        )
        self.assertEqual(len(matches), 0)

    def test_multiple_samples_matched(self):
        correlator = SampleCorrelator(tolerance_rms=0.1, tolerance_zcr=0.15)
        correlator.register_sample("aaa", {"rms": 0.05, "zero_crossing_rate": 0.2})
        correlator.register_sample("bbb", {"rms": 0.06, "zero_crossing_rate": 0.21})

        matches = correlator.check_correlation(
            {"rms_level": 0.055, "zero_crossing_rate": 0.205}
        )
        self.assertEqual(len(matches), 2)

    def test_handles_none_fingerprint(self):
        correlator = SampleCorrelator()
        correlator.register_sample("xxx", {"rms": None, "zero_crossing_rate": None})

        matches = correlator.check_correlation(
            {"rms_level": 0.05, "zero_crossing_rate": 0.2}
        )
        self.assertEqual(len(matches), 0)

    def test_handles_non_numeric_stream_features(self):
        correlator = SampleCorrelator()
        correlator.register_sample("xxx", {"rms": 0.05, "zero_crossing_rate": 0.2})

        matches = correlator.check_correlation(
            {"rms_level": "not_a_number", "zero_crossing_rate": 0.2}
        )
        self.assertEqual(len(matches), 0)


if __name__ == "__main__":
    unittest.main()


class NetworkCapMessageTests(unittest.TestCase):
    def test_cap_is_named_by_value_not_enum_repr(self):
        from daemon.evidence_receiver.taxonomy import validate_network_event

        ok, message = validate_network_event(
            {"event_type": "host_environment", "proof_level": "externally_verified"}
        )
        self.assertFalse(ok)
        self.assertNotIn("ProofLevel.", message)
