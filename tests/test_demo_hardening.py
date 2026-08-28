import hashlib
import json
import math
import random
import tempfile
import unittest
import wave
import zipfile
from pathlib import Path

from daemon.audio_association import associate_export, extract_feature_sequence
from daemon.bundle import create_evidence_bundle, verify_evidence_bundle
from daemon.evidence_receiver.receiver import EvidenceReceiver
from daemon.signing import Ed25519Signer


def _write_wav(path: Path, samples: list[float], sample_rate: int = 44_100) -> None:
    import struct

    with wave.open(str(path), "wb") as handle:
        handle.setnchannels(1)
        handle.setsampwidth(2)
        handle.setframerate(sample_rate)
        handle.writeframes(b"".join(
            struct.pack("<h", max(-32768, min(32767, round(sample * 32767))))
            for sample in samples
        ))


def _fixture_samples(window_size: int = 4096, windows: int = 16) -> list[float]:
    samples: list[float] = []
    frequencies = (190.0, 330.0, 510.0, 270.0, 720.0)
    levels = (0.08, 0.31, 0.14, 0.37, 0.19, 0.28)
    for window in range(windows):
        for offset in range(window_size):
            absolute = window * window_size + offset
            quarter = (0.5, 1.0, 0.7, 0.9)[min(3, offset * 4 // window_size)]
            samples.append(
                levels[window % len(levels)]
                * quarter
                * math.sin(2 * math.pi * frequencies[window % len(frequencies)] * absolute / 44_100)
            )
    return samples


def _routed_events(source_path: Path, window_seconds: float = 4096 / 44_100) -> list[dict[str, object]]:
    sequence, _details = extract_feature_sequence(source_path, window_seconds)
    return [
        {
            "event_type": "buffer_hash",
            "rms_level": feature["rms"],
            "zero_crossing_rate": feature["zcr"],
            "crest_factor": feature["crest"],
            "energy_envelope": list(feature["envelope"]),
            "sample_rate_hz": 44_100,
            "window_size_samples": 4096,
        }
        for feature in sequence
    ]


class AcknowledgementStateTests(unittest.TestCase):
    def test_acknowledgement_scopes_sequences_and_reports_gap_duplicate_and_session(self):
        with tempfile.TemporaryDirectory() as tmp:
            receiver = EvidenceReceiver(port=0, evidence_path=Path(tmp) / "events.jsonl")
            self.addCleanup(receiver.close)

            def packet(sequence: int, session: str = "plugin-session-a") -> bytes:
                return json.dumps({
                    "event_type": "buffer_hash",
                    "proof_level": "directly_observed",
                    "plugin_instance_id": "plugin-a",
                    "plugin_capture_session_id": session,
                    "event_sequence": sequence,
                    "window_hash": f"hash-{session}-{sequence}",
                    "prev_hash": "genesis" if sequence == 1 else f"hash-{session}-{sequence - 1}",
                    "rms_level": 0.2,
                    "zero_crossing_rate": 0.1,
                }).encode()

            _event, first = receiver.process_packet_with_ack(packet(1))
            _event, gap = receiver.process_packet_with_ack(packet(3))
            duplicate_event, duplicate = receiver.process_packet_with_ack(packet(3))
            _event, new_session = receiver.process_packet_with_ack(packet(1, "plugin-session-b"))

            self.assertTrue(first["accepted"])
            self.assertEqual(first["highest_contiguous_sequence"], 1)
            self.assertEqual(gap["receipt_state"], "accepted_chain_break")
            self.assertEqual(gap["stream_gaps"], 1)
            self.assertEqual(gap["stream_chain_breaks"], 1)
            self.assertEqual(gap["highest_accepted_sequence"], 3)
            self.assertEqual(gap["highest_contiguous_sequence"], 1)
            self.assertIsNone(duplicate_event)
            self.assertEqual(duplicate["receipt_state"], "rejected_duplicate_or_out_of_order")
            self.assertEqual(new_session["highest_contiguous_sequence"], 1)
            self.assertEqual(len(receiver.receipt_summary()["streams"]), 2)


class RoutedAssociationTests(unittest.TestCase):
    def test_positive_transformed_unrelated_and_unavailable_results_are_conservative(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            samples = _fixture_samples()
            source = root / "source.wav"
            exact = root / "exact.wav"
            transformed = root / "transformed.wav"
            unrelated = root / "unrelated.wav"
            unsupported = root / "unsupported.mp3"
            _write_wav(source, samples)
            _write_wav(exact, samples)
            _write_wav(transformed, [0.0] * 8192 + [sample * 0.43 for sample in samples])
            generator = random.Random(17)
            noise = [generator.uniform(-0.35, 0.35) for _ in range(len(samples))]
            _write_wav(unrelated, noise)
            unsupported.write_bytes(b"not an audio file")
            routed = _routed_events(source)

            positive = associate_export(exact, routed)
            shifted = associate_export(transformed, routed)
            mismatch = associate_export(unrelated, routed)
            insufficient = associate_export(exact, [])
            unavailable = associate_export(unsupported, routed)

            self.assertEqual(positive["status"], "inferred_match")
            self.assertEqual(positive["apw:proof_level"], "inferred")
            self.assertEqual(shifted["status"], "inferred_match")
            self.assertAlmostEqual(shifted["best_offset_seconds"], 8192 / 44_100, places=3)
            self.assertGreater(shifted["matched_window_count"], 0)
            self.assertGreaterEqual(shifted["comparable_window_count"], shifted["matched_window_count"])
            self.assertEqual(mismatch["status"], "not_established")
            self.assertEqual(mismatch["apw:proof_level"], "unknown_unobserved")
            self.assertEqual(insufficient["status"], "unavailable")
            self.assertEqual(unavailable["status"], "unavailable")


class DeterministicBundleTests(unittest.TestCase):
    def test_bundle_index_signature_hashes_and_zip_metadata_are_deterministic(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            evidence_dir = root / "evidence"
            evidence_dir.mkdir()
            evidence = evidence_dir / "events.jsonl"
            evidence.write_text('{"event_type":"buffer_hash"}\n', encoding="utf-8")
            export = root / "export.wav"
            _write_wav(export, _fixture_samples(windows=3))
            manifest = root / "export_manifest.json"
            manifest.write_text(json.dumps({
                "session_id": "bundle-test",
                "created_at": "2026-08-28T00:00:00Z",
                "export": {"file_path": str(export)},
                "evidence_binding": {
                    "evidence_directory": str(evidence_dir),
                    "evidence_files": {
                        evidence.name: {
                            "byte_length": evidence.stat().st_size,
                            "sha256": hashlib.sha256(evidence.read_bytes()).hexdigest(),
                        }
                    },
                },
            }, sort_keys=True), encoding="utf-8")
            report = root / "report.html"
            verification = root / "verification.json"
            handoff = root / "handoff.json"
            report.write_text("<html>report</html>", encoding="utf-8")
            verification.write_text('{"outcome":"verified"}\n', encoding="utf-8")
            handoff.write_text('{"status":"candidate"}\n', encoding="utf-8")
            signer = Ed25519Signer(root / "private.key", root / "public.key")

            outputs = []
            for suffix in ("a", "b"):
                index = root / f"index-{suffix}.json"
                bundle = root / f"bundle-{suffix}.zip"
                create_evidence_bundle(
                    manifest_path=manifest,
                    report_path=report,
                    verification_path=verification,
                    handoff_path=handoff,
                    index_path=index,
                    bundle_path=bundle,
                    signer=signer,
                )
                self.assertEqual(verify_evidence_bundle(index, bundle), [])
                outputs.append((index, bundle))

            self.assertEqual(outputs[0][0].read_bytes(), outputs[1][0].read_bytes())
            self.assertEqual(
                hashlib.sha256(outputs[0][1].read_bytes()).digest(),
                hashlib.sha256(outputs[1][1].read_bytes()).digest(),
            )
            with zipfile.ZipFile(outputs[0][1]) as archive:
                self.assertEqual(archive.namelist(), sorted(archive.namelist()))
                self.assertTrue(all(info.date_time == (1980, 1, 1, 0, 0, 0) for info in archive.infolist()))


if __name__ == "__main__":
    unittest.main()
