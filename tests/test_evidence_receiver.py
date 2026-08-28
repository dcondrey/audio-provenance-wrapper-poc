import json
import tempfile
import unittest
from pathlib import Path

from daemon.evidence_receiver.taxonomy import (
    EventType,
    ProofLevel,
    validate_event,
)
from daemon.evidence_receiver.receiver import EvidenceReceiver
from daemon.evidence_receiver.correlation import SampleCorrelator


class TaxonomyTests(unittest.TestCase):
    def test_valid_buffer_hash_event(self):
        event = {
            "event_type": "buffer_hash",
            "proof_level": "directly_observed",
            "timestamp_ms": 12345,
            "window_hash": "abc123",
            "prev_hash": "genesis",
            "rms_level": 0.042,
            "zero_crossing_rate": 0.15,
        }
        valid, error = validate_event(event)
        self.assertTrue(valid, error)

    def test_rejects_missing_required_field(self):
        event = {
            "event_type": "buffer_hash",
            "proof_level": "directly_observed",
            "timestamp_ms": 12345,
            "prev_hash": "genesis",
            "rms_level": 0.042,
            "zero_crossing_rate": 0.15,
        }
        valid, error = validate_event(event)
        self.assertFalse(valid)
        self.assertIn("window_hash", error)

    def test_rejects_unknown_event_type(self):
        event = {"event_type": "bogus", "proof_level": "directly_observed"}
        valid, _ = validate_event(event)
        self.assertFalse(valid)

    def test_rejects_unknown_proof_level(self):
        event = {"event_type": "buffer_hash", "proof_level": "bogus"}
        valid, _ = validate_event(event)
        self.assertFalse(valid)

    def test_valid_transport_change_event(self):
        event = {
            "event_type": "transport_change",
            "proof_level": "directly_observed",
            "transport_state": "playing",
        }
        valid, error = validate_event(event)
        self.assertTrue(valid, error)

    def test_valid_midi_event(self):
        event = {
            "event_type": "midi_event",
            "proof_level": "directly_observed",
            "midi_event_type": "note_on",
            "midi_channel": 1,
        }
        valid, error = validate_event(event)
        self.assertTrue(valid, error)

    def test_valid_audio_transition_event(self):
        event = {
            "event_type": "audio_transition",
            "proof_level": "directly_observed",
            "direction": "silence_to_audio",
            "boundary_hash": "deadbeef",
        }
        valid, error = validate_event(event)
        self.assertTrue(valid, error)

    def test_valid_spectral_shift_event(self):
        event = {
            "event_type": "spectral_shift",
            "proof_level": "directly_observed",
            "prev_spectral_centroid_hz": 1200.0,
            "new_spectral_centroid_hz": 2400.0,
        }
        valid, error = validate_event(event)
        self.assertTrue(valid, error)

    def test_event_type_enum_values(self):
        self.assertEqual(EventType.BUFFER_HASH.value, "buffer_hash")
        self.assertEqual(EventType.AUDIO_TRANSITION.value, "audio_transition")
        self.assertEqual(EventType.SPECTRAL_SHIFT.value, "spectral_shift")
        self.assertEqual(EventType.INGREDIENT_CORRELATION.value, "ingredient_correlation")

    def test_proof_level_enum_values(self):
        self.assertEqual(ProofLevel.DIRECTLY_OBSERVED.value, "directly_observed")
        self.assertEqual(ProofLevel.INFERRED.value, "inferred")
        self.assertEqual(ProofLevel.UNKNOWN_UNOBSERVED.value, "unknown_unobserved")


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

    def test_event_count_increments(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            evidence_path = Path(tmp_dir) / "events.jsonl"
            receiver = EvidenceReceiver(
                host="127.0.0.1", port=0, evidence_path=evidence_path
            )
            self.addCleanup(receiver.close)

            event_json = json.dumps({
                "event_type": "transport_change",
                "proof_level": "directly_observed",
                "transport_state": "playing",
            }).encode("utf-8")

            receiver.process_packet(event_json)
            receiver.process_packet(event_json)

            self.assertEqual(receiver.event_count, 2)


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
