import json
import tempfile
import unittest
from pathlib import Path

from daemon.correlation_engine.engine import CorrelationEngine, LayerEvent
from daemon.evidence_receiver.receiver import EvidenceReceiver
from daemon.schema import validate_manifest_invariants


class BoundedCorrelationTests(unittest.TestCase):
    def test_mixed_clocks_remain_bounded_and_source_match_is_deduplicated(self):
        with tempfile.TemporaryDirectory() as tmp:
            evidence = Path(tmp) / "composite.jsonl"
            engine = CorrelationEngine(
                window_ms=2_000,
                evidence_path=evidence,
                max_buffer_events=32,
            )
            sample = LayerEvent(
                "sample_watcher", "sample_file_observed", 1_000,
                {"file_name": "kick.wav", "daemon_event_id": "sample-1", "timestamp_ms": 1_700_000_000_000},
            )
            engine.ingest(sample)
            engine.ingest(LayerEvent("audio_buffer", "buffer_hash", 1_010, {"daemon_event_id": "audio-1"}))
            for index in range(200):
                engine.ingest(LayerEvent(
                    "audio_buffer", "buffer_hash", 1_011 + index,
                    {"daemon_event_id": f"audio-{index + 2}", "timestamp_ms": 10_000 + index},
                ))
            engine.ingest(LayerEvent(
                "project_differ", "project_diff", 1_700_000_000_000,
                {"daemon_event_id": "epoch-source", "clips_added": 1, "clips_removed": 0},
            ))
            self.assertLessEqual(engine.buffer_size, 32)
            records = [json.loads(line) for line in evidence.read_text().splitlines()]
            sample_matches = [record for record in records if record["edit_type"] == "sample_import_confirmed"]
            self.assertEqual(len(sample_matches), 1)
            self.assertGreater(engine.duplicate_suppressions, 0)

    def test_one_action_emits_one_composite_edit_as_supporting_evidence_grows(self):
        with tempfile.TemporaryDirectory() as tmp:
            evidence = Path(tmp) / "composite.jsonl"
            engine = CorrelationEngine(window_ms=2_000, evidence_path=evidence)
            engine.ingest(LayerEvent(
                "transport", "transport_change", 1_000,
                {"daemon_event_id": "transport-1", "transport_state": "recording"},
            ))
            for index in range(5):
                engine.ingest(LayerEvent(
                    "audio_buffer", "audio_transition", 1_100 + index * 100,
                    {"daemon_event_id": f"transition-{index}", "direction": "silence_to_audio"},
                ))
            records = [json.loads(line) for line in evidence.read_text().splitlines()]
            started = [r for r in records if r["edit_type"] == "recording_started"]
            self.assertEqual(len(started), 1)
            self.assertGreater(engine.duplicate_suppressions, 0)

    def test_continuous_content_change_is_bounded_per_window_not_per_event(self):
        with tempfile.TemporaryDirectory() as tmp:
            evidence = Path(tmp) / "composite.jsonl"
            engine = CorrelationEngine(window_ms=2_000, evidence_path=evidence)
            for index in range(50):
                engine.ingest(LayerEvent(
                    "audio_buffer", "spectral_shift", index * 100,
                    {"daemon_event_id": f"shift-{index}"},
                ))
            records = [json.loads(line) for line in evidence.read_text().splitlines()]
            changed = [r for r in records if r["edit_type"] == "content_changed"]
            self.assertGreaterEqual(len(changed), 1)
            self.assertLessEqual(len(changed), 3)
            self.assertGreater(engine.duplicate_suppressions, 0)

    def test_long_session_evidence_is_linear_not_candidate_sized(self):
        with tempfile.TemporaryDirectory() as tmp:
            evidence = Path(tmp) / "composite.jsonl"
            engine = CorrelationEngine(evidence_path=evidence, max_buffer_events=64)
            for index in range(5_000):
                engine.ingest(LayerEvent(
                    "audio_buffer", "buffer_hash", index,
                    {"daemon_event_id": f"window-{index}"},
                ))
            self.assertLessEqual(engine.buffer_size, 64)
            self.assertFalse(evidence.exists())


class NetworkNumericFieldTests(unittest.TestCase):
    def test_non_numeric_wire_fields_are_rejected_at_the_boundary(self):
        from daemon.evidence_receiver.taxonomy import validate_network_event

        base = {
            "event_type": "buffer_hash",
            "proof_level": "directly_observed",
            "window_hash": "h", "prev_hash": "genesis",
            "rms_level": 0.2, "zero_crossing_rate": 0.1,
        }
        valid, _ = validate_network_event(base)
        self.assertTrue(valid)
        valid, _ = validate_network_event({**base, "window_size_samples": 4096, "sample_rate_hz": 44100})
        self.assertTrue(valid)
        for poisoned in (
            {**base, "window_size_samples": "x"},
            {**base, "sample_rate_hz": "44100hz"},
            {**base, "rms_level": "loud"},
            {**base, "energy_envelope": ["a", "b", "c", "d"]},
            # json.loads accepts the non-standard NaN/Infinity literals
            {**base, "window_size_samples": float("nan")},
            {**base, "sample_rate_hz": float("inf")},
            {**base, "energy_envelope": [0.1, float("nan"), 0.2, 0.3]},
        ):
            valid, reason = validate_network_event(poisoned)
            self.assertFalse(valid, reason)


class VerifierInputHardeningTests(unittest.TestCase):
    def test_hostile_verifier_inputs_yield_findings_not_exceptions(self):
        from daemon.bundle import verify_evidence_bundle
        from daemon.verify import verify_manifest

        with tempfile.TemporaryDirectory() as tmp:
            deep = Path(tmp) / "deep_manifest.json"
            deep.write_text('{"apw_version":"0.9.0","x":' + "[" * 3000 + "]" * 3000 + "}")
            result = verify_manifest(deep)
            self.assertEqual(result.outcome, "untrusted")

            index = Path(tmp) / "bundle_index.json"
            index.write_text("null")
            errors = verify_evidence_bundle(index, Path(tmp) / "bundle.zip")
            self.assertEqual(errors, ["bundle index must be a JSON object"])


class TrustInvariantTests(unittest.TestCase):
    def test_sequence_gap_is_counted_and_complete_coverage_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            receiver = EvidenceReceiver(port=0, evidence_path=Path(tmp) / "events.jsonl")
            self.addCleanup(receiver.close)
            for sequence in (1, 3):
                receiver.process_packet(json.dumps({
                    "event_type": "buffer_hash",
                    "proof_level": "directly_observed",
                    "plugin_instance_id": "plugin-test",
                    "event_sequence": sequence,
                    "window_hash": f"hash-{sequence}",
                    "prev_hash": "genesis" if sequence == 1 else "hash-1",
                    "rms_level": 0.2,
                    "zero_crossing_rate": 0.1,
                }).encode())
            self.assertEqual(receiver.sequence_gap_count, 1)

            manifest = {
                "apw_version": "0.9.0", "schema": "audio-provenance-manifest-v0",
                "session_id": "s", "capture_session": {"apw:proof_level": "directly_observed"},
                "created_at": "now", "observed_stems": [], "claim_summary": [],
                "stem_export_association": {"status": "unavailable", "apw:proof_level": "unknown_unobserved"},
                "observation_coverage": {
                    "status": "complete_observed_path", "basis": "invalid", "apw:proof_level": "inferred",
                    "counters": {
                        "windows_hashed": 2, "buffer_hash_events_received": 2,
                        "fifo_samples_dropped": 0, "fifo_windows_dropped": 0,
                        "udp_sends_failed": 0, "sequence_gaps": 1, "hash_chain_breaks": 0,
                    },
                },
                "apw:unobserved": [], "c2pa_mapping": {},
            }
            errors = validate_manifest_invariants(manifest)
            self.assertTrue(any("sequence_gaps=0" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
