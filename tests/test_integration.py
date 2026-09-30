import json
import socket
import tempfile
import threading
import time
import unittest
from pathlib import Path

from daemon.__main__ import Daemon
from daemon.hardware_attestation.provider import SoftwareProvider
from tests.audio_files import write_wav


class DaemonIntegrationTests(unittest.TestCase):
    """End-to-end: send plugin events via UDP, detect export, generate manifest."""

    def test_full_pipeline(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            evidence_dir = tmp_path / "evidence"
            manifest_dir = tmp_path / "manifests"
            sample_dir = tmp_path / "samples"
            export_dir = tmp_path / "exports"
            sample_dir.mkdir()
            export_dir.mkdir()

            daemon = Daemon(
                udp_port=0,
                evidence_dir=evidence_dir,
                sample_dir=sample_dir,
                export_dir=export_dir,
                manifest_dir=manifest_dir,
                source_category="imported_sample",
                hardware_provider=SoftwareProvider(tmp_path / "signing-key.bin"),
            )
            actual_port = daemon.receiver.sock.getsockname()[1]

            thread = threading.Thread(target=daemon.run, daemon=True)
            thread.start()
            time.sleep(0.3)

            sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                events = [
                    {
                        "event_type": "host_environment",
                        "proof_level": "directly_observed",
                        "timestamp_ms": 900,
                        "sample_position": 0,
                        "host_recognised": True,
                        "host_name": "Ableton Live",
                        "host_executable_name": "Live",
                        "wrapper_format": "VST3",
                    },
                    {
                        "event_type": "transport_change",
                        "proof_level": "directly_observed",
                        "timestamp_ms": 1000,
                        "sample_position": 0,
                        "transport_state": "playing",
                        "is_looping": False,
                        "bpm": 120.0,
                    },
                    {
                        "event_type": "audio_transition",
                        "proof_level": "directly_observed",
                        "timestamp_ms": 1010,
                        "sample_position": 441,
                        "direction": "silence_to_audio",
                        "boundary_hash": "abc123",
                    },
                    {
                        "event_type": "buffer_hash",
                        "proof_level": "directly_observed",
                        "timestamp_ms": 1100,
                        "sample_position": 4096,
                        "window_hash": "hash001",
                        "prev_hash": "genesis",
                        "rms_level": 0.15,
                        "zero_crossing_rate": 0.3,
                        "spectral_centroid_hz": 2000.0,
                        "channel_count": 2,
                        "sample_rate_hz": 44100,
                        "window_size_samples": 4096,
                        "bpm": 120.0,
                        "band_low": 0.4,
                        "band_mid": 0.4,
                        "band_high": 0.2,
                    },
                    {
                        "event_type": "buffer_hash",
                        "proof_level": "directly_observed",
                        "timestamp_ms": 1200,
                        "sample_position": 8192,
                        "window_hash": "hash002",
                        "prev_hash": "hash001",
                        "rms_level": 0.14,
                        "zero_crossing_rate": 0.31,
                        "spectral_centroid_hz": 2050.0,
                        "channel_count": 2,
                        "sample_rate_hz": 44100,
                        "window_size_samples": 4096,
                        "bpm": 120.0,
                        "band_low": 0.4,
                        "band_mid": 0.4,
                        "band_high": 0.2,
                    },
                ]
                for event in events:
                    sock.sendto(json.dumps(event).encode(), ("127.0.0.1", actual_port))
                    time.sleep(0.05)
            finally:
                sock.close()

            plugin_events_path = evidence_dir / "plugin_events.jsonl"
            _wait_for(
                lambda: plugin_events_path.exists()
                and len(plugin_events_path.read_text().splitlines()) >= 5,
                message="5 plugin events in plugin_events.jsonl",
            )
            lines = plugin_events_path.read_text().splitlines()
            self.assertEqual(len(lines), 5)

            export_path = export_dir / "mixdown.wav"
            write_wav(export_path)

            artifact_dir = manifest_dir / "artifacts"
            expected_outputs = [
                manifest_dir / "mixdown_provenance.html",
                artifact_dir / "mixdown_verification.json",
                artifact_dir / "mixdown_bundle_index.json",
                artifact_dir / "mixdown_evidence_bundle.zip",
            ]
            _wait_for(
                lambda: bool(list(manifest_dir.glob("*.json")))
                and all(p.is_file() for p in expected_outputs),
                message="manifest and evidence artifacts for mixdown.wav",
            )

            manifests = list(manifest_dir.glob("*.json"))
            self.assertEqual(len(manifests), 1, f"Expected 1 manifest, found {len(manifests)}")

            manifest = json.loads(manifests[0].read_text())
            self.assertEqual(manifest["apw_version"], "0.9.0")
            self.assertIn("export", manifest)
            self.assertEqual(manifest["export"]["file_name"], "mixdown.wav")
            self.assertIn("apw:unobserved", manifest)
            self.assertIn("c2pa_mapping", manifest)

            stems = manifest.get("observed_stems", [])
            self.assertEqual(len(stems), 1, "Expected 1 observed stem from buffer_hash events")
            self.assertEqual(stems[0]["hash_chain_root"], "hash002")
            self.assertEqual(stems[0]["hash_chain_genesis"], "genesis")
            self.assertEqual(stems[0]["hash_chain_length"], 2)
            self.assertEqual(stems[0]["sample_rate_hz"], 44100)
            self.assertEqual(stems[0]["source_category"], "imported_sample")
            self.assertEqual(stems[0]["source_category_proof_level"], "user_declared")
            host = manifest["host_environment"]
            self.assertEqual(host["status"], "observed")
            self.assertEqual(host["host_name"], "Ableton Live")
            self.assertEqual(host["wrapper_format"], "VST3")
            self.assertEqual(host["apw:proof_level"], "directly_observed")
            host_claim = next(
                claim for claim in manifest["claim_summary"]
                if claim["claim"] == "host_application"
            )
            self.assertEqual(host_claim["value"], "Ableton Live")
            self.assertIn("manifest_signature", manifest)
            self.assertEqual(
                manifest["manifest_signature"]["trust_scope"],
                "local_software_integrity",
            )
            self.assertIn("hardware_binding", manifest)
            self.assertEqual(
                manifest["hardware_binding"]["chain_root_hash"],
                stems[0]["hash_chain_root"],
            )
            self.assertFalse(manifest["hardware_binding"]["hardware_attested"])
            cosignature = manifest["manifest_signature"]["hardware_cosignature"]
            self.assertEqual(cosignature["previous_cosignature_hash"], "genesis")
            self.assertEqual(
                cosignature["content_hash"],
                manifest["manifest_signature"]["signed_content_hash"],
            )
            self.assertTrue((manifest_dir / "mixdown_provenance.html").is_file())
            self.assertTrue((manifest_dir / "artifacts/mixdown_evidence_bundle.zip").is_file())
            self.assertTrue((manifest_dir / "artifacts/mixdown_bundle_index.json").is_file())
            stored_verification = json.loads(
                (manifest_dir / "artifacts/mixdown_verification.json").read_text()
            )
            self.assertFalse(any(
                finding["code"] == "html_report_missing"
                for finding in stored_verification["findings"]
            ))

            from daemon.verify import verify_manifest
            verification = verify_manifest(
                manifests[0],
                signing_key_path=tmp_path / "signing-key.bin",
            )
            self.assertTrue(verification.passed)
            self.assertTrue(any(
                finding.code == "local_signature_valid"
                for finding in verification.findings
            ))

            daemon.stop()
            thread.join(timeout=15)

    def test_sample_detection_feeds_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            evidence_dir = tmp_path / "evidence"
            manifest_dir = tmp_path / "manifests"
            sample_dir = tmp_path / "samples"
            export_dir = tmp_path / "exports"
            sample_dir.mkdir()
            export_dir.mkdir()

            daemon = Daemon(
                udp_port=0,
                evidence_dir=evidence_dir,
                sample_dir=sample_dir,
                export_dir=export_dir,
                manifest_dir=manifest_dir,
                hardware_provider=SoftwareProvider(tmp_path / "signing-key.bin"),
            )

            thread = threading.Thread(target=daemon.run, daemon=True)
            thread.start()
            time.sleep(0.3)

            write_wav(sample_dir / "kick.wav")
            sample_events_path = evidence_dir / "sample_import_events.jsonl"
            _wait_for(
                lambda: sample_events_path.exists()
                and bool(sample_events_path.read_text().strip()),
                message="sample import event for kick.wav",
            )

            write_wav(export_dir / "final.wav")
            final_outputs = [
                manifest_dir / "final_manifest.json",
                manifest_dir / "artifacts" / "final_bundle_index.json",
                manifest_dir / "artifacts" / "final_evidence_bundle.zip",
            ]
            _wait_for(
                lambda: all(p.is_file() for p in final_outputs),
                message="manifest and evidence bundle for final.wav",
            )

            manifests = list(manifest_dir.glob("*.json"))
            self.assertEqual(len(manifests), 1)

            manifest = json.loads(manifests[0].read_text())
            self.assertIn("ingredients", manifest)
            self.assertEqual(len(manifest["ingredients"]), 1)
            self.assertEqual(manifest["ingredients"][0]["file_name"], "kick.wav")

            daemon.stop()
            thread.join(timeout=15)

    def test_overwriting_existing_export_generates_new_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            evidence_dir = tmp_path / "evidence"
            manifest_dir = tmp_path / "manifests"
            sample_dir = tmp_path / "samples"
            export_dir = tmp_path / "exports"
            sample_dir.mkdir()
            export_dir.mkdir()
            export_path = export_dir / "demo.wav"
            write_wav(export_path, 1000)

            daemon = Daemon(
                udp_port=0,
                evidence_dir=evidence_dir,
                sample_dir=sample_dir,
                export_dir=export_dir,
                manifest_dir=manifest_dir,
                hardware_provider=SoftwareProvider(tmp_path / "signing-key.bin"),
            )
            thread = threading.Thread(target=daemon.run, daemon=True)
            thread.start()
            time.sleep(0.4)

            write_wav(export_path, 4000)
            demo_outputs = [
                manifest_dir / "demo_manifest.json",
                manifest_dir / "artifacts" / "demo_bundle_index.json",
                manifest_dir / "artifacts" / "demo_evidence_bundle.zip",
            ]
            _wait_for(
                lambda: all(p.is_file() for p in demo_outputs),
                message="manifest and evidence bundle for overwritten demo.wav",
            )
            self.assertTrue((manifest_dir / "demo_manifest.json").is_file())
            manifest = json.loads((manifest_dir / "demo_manifest.json").read_text())
            # The next manifest in this session must entangle this one.
            self.assertEqual(
                daemon._last_cosignature_hash,
                manifest["manifest_signature"]["hardware_cosignature"]["entangled_hash"],
            )
            self.assertNotEqual(daemon._last_cosignature_hash, "genesis")
            daemon.stop()
            thread.join(timeout=15)


class HostEnvironmentTests(unittest.TestCase):
    """The host identity the daemon signs into a manifest."""

    def _daemon(self, tmp: str) -> Daemon:
        tmp_path = Path(tmp)
        return Daemon(
            udp_port=0,
            evidence_dir=tmp_path / "evidence",
            sample_dir=tmp_path / "samples",
            export_dir=tmp_path / "exports",
            manifest_dir=tmp_path / "manifests",
            hardware_provider=SoftwareProvider(tmp_path / "signing-key.bin"),
        )

    @staticmethod
    def _event(**overrides: object) -> dict[str, object]:
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

    def test_recognised_host_is_named(self):
        with tempfile.TemporaryDirectory() as tmp:
            daemon = self._daemon(tmp)
            daemon._record_plugin_event(self._event(), "session")

            host = daemon._derive_host_environment()
            self.assertEqual(host["status"], "observed")
            self.assertEqual(host["host_name"], "Ableton Live")
            self.assertEqual(host["wrapper_format"], "VST3")
            self.assertEqual(host["apw:proof_level"], "directly_observed")

    def test_repeated_identical_observation_is_not_a_conflict(self):
        with tempfile.TemporaryDirectory() as tmp:
            daemon = self._daemon(tmp)
            for _ in range(3):
                daemon._record_plugin_event(self._event(), "session")

            host = daemon._derive_host_environment()
            self.assertEqual(host["status"], "observed")
            self.assertEqual(host["host_name"], "Ableton Live")

    def test_unrecognised_host_is_not_named(self):
        with tempfile.TemporaryDirectory() as tmp:
            daemon = self._daemon(tmp)
            daemon._record_plugin_event(
                self._event(host_recognised=False, host_name=None), "session"
            )

            host = daemon._derive_host_environment()
            self.assertEqual(host["status"], "host_unrecognised")
            self.assertIsNone(host["host_name"])
            self.assertEqual(host["apw:proof_level"], "unknown_unobserved")
            # Observed regardless of whether the wrapper knew the host.
            self.assertEqual(host["host_executable_name"], "Live")

    def test_a_disagreeing_report_withdraws_the_host(self):
        """The UDP socket cannot authenticate its sender: last-wins would let a
        spoofed datagram rename the host in a signed manifest."""
        with tempfile.TemporaryDirectory() as tmp:
            daemon = self._daemon(tmp)
            daemon._record_plugin_event(self._event(), "session")
            daemon._record_plugin_event(self._event(host_name="Logic Pro"), "session")

            host = daemon._derive_host_environment()
            self.assertEqual(host["status"], "conflicting_observations")
            self.assertIsNone(host["host_name"])
            self.assertFalse(host["host_recognised"])
            self.assertEqual(host["apw:proof_level"], "unknown_unobserved")

    def test_one_host_in_two_plugin_formats_is_not_a_conflict(self):
        """Producers A/B a VST3 against an AU in one session; that is one host."""
        with tempfile.TemporaryDirectory() as tmp:
            daemon = self._daemon(tmp)
            daemon._record_plugin_event(self._event(), "session")
            daemon._record_plugin_event(
                self._event(wrapper_format="AudioUnit"), "session"
            )

            host = daemon._derive_host_environment()
            self.assertEqual(host["status"], "observed")
            self.assertEqual(host["host_name"], "Ableton Live")
            self.assertEqual(host["wrapper_format"], "VST3")

    def test_no_report_reads_as_unobserved(self):
        with tempfile.TemporaryDirectory() as tmp:
            host = self._daemon(tmp)._derive_host_environment()
            self.assertEqual(host["status"], "unobserved")
            self.assertIsNone(host["host_name"])
            self.assertEqual(host["apw:proof_level"], "unknown_unobserved")

    def test_the_constructor_report_survives_session_event_eviction(self):
        with tempfile.TemporaryDirectory() as tmp:
            daemon = self._daemon(tmp)
            daemon._max_session_events = 2
            daemon._record_plugin_event(self._event(), "session")
            for position in range(4):
                daemon._record_plugin_event(
                    {
                        "event_type": "transport_change",
                        "proof_level": "directly_observed",
                        "transport_state": "playing",
                        "sample_position": position,
                    },
                    "transport",
                )

            with daemon._session_lock:
                self.assertNotIn(
                    "host_environment",
                    {event.get("event_type") for event in daemon._session_events},
                )
            self.assertEqual(daemon._derive_host_environment()["host_name"], "Ableton Live")


class VerifyTests(unittest.TestCase):
    def test_valid_manifest_passes(self):
        from daemon.verify import verify_manifest
        from daemon.manifest_builder.builder import ManifestBuilder, StemEvidence, ExportEvidence

        builder = ManifestBuilder(session_id="test")
        builder.add_stem(StemEvidence(
            stem_id="s1", hash_chain_root="abc", hash_chain_length=10,
            first_observed_ms=0, last_observed_ms=1000, sample_rate_hz=44100,
            channel_count=2, source_category="unknown", proof_level="directly_observed",
        ))
        builder.set_export(ExportEvidence(
            file_path="/tmp/x.wav", file_name="x.wav", sha256="dead",
            format="wav", file_size_bytes=100, duration_seconds=1.0,
            exported_at="2026-05-28T00:00:00Z",
        ))
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "m.json"
            builder.write_json(p)
            result = verify_manifest(p)
            self.assertTrue(result.passed)

    def test_empty_manifest_fails(self):
        from daemon.verify import verify_manifest

        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "m.json"
            p.write_text("{}")
            result = verify_manifest(p)
            self.assertFalse(result.passed)

    def test_malformed_manifest_degrades_never_crashes(self):
        from daemon.verify import verify_manifest

        malformed = [
            "null",
            "5",
            "true",
            '"string"',
            "[1, 2, 3]",
            '{"export": "not-a-dict"}',
            '{"observed_stems": 5}',
            '{"observed_stems": ["not-a-dict"]}',
            '{"c2pa_mapping": "nope"}',
            '{"c2pa_mapping": {"assertions": [1, 2]}}',
            '{"evidence_binding": "nope"}',
            json.dumps({
                "evidence_binding": {
                    "evidence_directory": "/tmp",
                    "evidence_files": {"e.jsonl": {"byte_length": "not-a-number", "sha256": "x"}},
                },
            }),
            json.dumps({
                "evidence_binding": {
                    "evidence_directory": "/tmp",
                    "evidence_files": {"e.jsonl": {"byte_length": -5, "sha256": "x"}},
                },
            }),
            '{"manifest_signature": "nope"}',
            '{"session_facts": "nope"}',
            '{"session_facts": {"tracks": 7}}',
        ]
        with tempfile.TemporaryDirectory() as tmp:
            for i, content in enumerate(malformed):
                p = Path(tmp) / f"m{i}.json"
                p.write_text(content)
                result = verify_manifest(p)
                self.assertFalse(result.passed, f"case {i}: {content[:60]}")
                self.assertIn(result.outcome, {"changed", "untrusted"}, f"case {i}")

    def test_status_write_survives_divergent_artifact_roots(self):
        # H-003: relative_to raised ValueError when --manifest-dir did not share
        # a root with the status dir, killing the export-watcher thread.
        with tempfile.TemporaryDirectory() as evidence_root, \
                tempfile.TemporaryDirectory() as manifest_root:
            daemon = Daemon(
                udp_port=0,
                evidence_dir=Path(evidence_root) / "evidence",
                sample_dir=Path(evidence_root) / "samples",
                manifest_dir=Path(manifest_root) / "manifests",
                generate_html_report=False,
            )
            try:
                daemon._last_manifest_path = Path(manifest_root) / "manifests" / "m.json"
                daemon._last_bundle_path = Path(manifest_root) / "manifests" / "b.zip"
                daemon._write_status("idle")
                status = json.loads(
                    (Path(evidence_root) / "status.json").read_text()
                )
                self.assertIn("..", status["links"]["manifest"])
            finally:
                daemon.receiver.close()

    def test_lying_coverage_counters_are_caught_from_bound_evidence(self):
        from daemon.verify import verify_manifest

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            evidence_dir = root / "evidence"
            evidence_dir.mkdir()
            evidence = evidence_dir / "plugin_events.jsonl"
            lines = []
            prev = "genesis"
            for i in range(4):
                window = f"hash-{i:03d}"
                lines.append(json.dumps({
                    "event_type": "buffer_hash",
                    "window_hash": window,
                    "prev_hash": prev,
                    "timestamp_ms": 1000 + i,
                }))
                prev = window
            evidence.write_text("\n".join(lines) + "\n")
            byte_length = evidence.stat().st_size
            import hashlib as _hashlib
            digest = _hashlib.sha256(evidence.read_bytes()).hexdigest()

            manifest = root / "m.json"
            manifest.write_text(json.dumps({
                "session_id": "s",
                "observation_coverage": {
                    "status": "complete_observed_path",
                    "apw:proof_level": "inferred",
                    "counters": {"buffer_hash_events_received": 9},
                },
                "evidence_binding": {
                    "evidence_directory": str(evidence_dir),
                    "evidence_files": {
                        "plugin_events.jsonl": {"byte_length": byte_length, "sha256": digest},
                    },
                    "evidence_file_hashes": {"plugin_events.jsonl": digest},
                    "chain_length": 9,
                    "last_window_hash": "hash-003",
                },
            }))
            result = verify_manifest(manifest)
            codes = {finding.code for finding in result.findings}
            self.assertIn("coverage_counters_mismatch", codes)
            self.assertFalse(result.passed)

            honest = root / "honest.json"
            content = json.loads(manifest.read_text())
            content["observation_coverage"]["counters"]["buffer_hash_events_received"] = 4
            content["evidence_binding"]["chain_length"] = 4
            honest.write_text(json.dumps(content))
            codes = {finding.code for finding in verify_manifest(honest).findings}
            self.assertIn("coverage_counters_rederived", codes)
            self.assertNotIn("coverage_counters_mismatch", codes)

            # Live-stream race: more events landed in the bound file than the
            # counter snapshot claimed. Conservative, not fraud: no error.
            conservative = root / "conservative.json"
            content = json.loads(honest.read_text())
            content["observation_coverage"]["counters"]["buffer_hash_events_received"] = 3
            content["evidence_binding"]["chain_length"] = 3
            content["evidence_binding"]["last_window_hash"] = "hash-002"
            conservative.write_text(json.dumps(content))
            codes = {finding.code for finding in verify_manifest(conservative).findings}
            self.assertNotIn("coverage_counters_mismatch", codes)
            self.assertIn("coverage_counters_rederived", codes)

    def test_chain_length_claimed_over_zero_bound_events_is_caught(self):
        from daemon.verify import verify_manifest
        import hashlib as _hashlib

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            evidence_dir = root / "evidence"
            evidence_dir.mkdir()
            # Bound evidence exists and hashes fine, but contains zero buffer_hash
            # events. A manifest claiming a chain over it must not pass silently.
            evidence = evidence_dir / "plugin_events.jsonl"
            evidence.write_text(
                json.dumps({"event_type": "transport_change", "transport_state": "playing"}) + "\n"
            )
            byte_length = evidence.stat().st_size
            digest = _hashlib.sha256(evidence.read_bytes()).hexdigest()
            manifest = root / "m.json"
            manifest.write_text(json.dumps({
                "session_id": "s",
                "observation_coverage": {
                    "status": "partial_observed_path",
                    "apw:proof_level": "inferred",
                    "counters": {"buffer_hash_events_received": 50},
                },
                "evidence_binding": {
                    "evidence_directory": str(evidence_dir),
                    "evidence_files": {
                        "plugin_events.jsonl": {"byte_length": byte_length, "sha256": digest},
                    },
                    "evidence_file_hashes": {"plugin_events.jsonl": digest},
                    "chain_length": 50,
                },
            }))
            codes = {finding.code for finding in verify_manifest(manifest).findings}
            self.assertIn("coverage_counters_mismatch", codes)

    def test_malformed_evidence_degrades_never_crashes(self):
        from daemon.verify import verify_hash_chain

        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "e.jsonl"
            p.write_text('5\n"str"\nnot json\n')
            result = verify_hash_chain(p)
            self.assertFalse(result.passed)

            binary = Path(tmp) / "b.jsonl"
            binary.write_bytes(b"\xff\xfe\x00garbage")
            result = verify_hash_chain(binary)
            self.assertFalse(result.passed)

    def test_valid_hash_chain_passes(self):
        from daemon.verify import verify_hash_chain

        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "e.jsonl"
            events = [
                {"event_type": "buffer_hash", "window_hash": "aaa", "prev_hash": "genesis", "timestamp_ms": 100},
                {"event_type": "buffer_hash", "window_hash": "bbb", "prev_hash": "aaa", "timestamp_ms": 200},
            ]
            p.write_text("\n".join(json.dumps(e) for e in events))
            result = verify_hash_chain(p)
            self.assertTrue(result.passed)

    def test_broken_hash_chain_fails(self):
        from daemon.verify import verify_hash_chain

        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "e.jsonl"
            events = [
                {"event_type": "buffer_hash", "window_hash": "aaa", "prev_hash": "genesis", "timestamp_ms": 100},
                {"event_type": "buffer_hash", "window_hash": "bbb", "prev_hash": "WRONG", "timestamp_ms": 200},
            ]
            p.write_text("\n".join(json.dumps(e) for e in events))
            result = verify_hash_chain(p)
            self.assertFalse(result.passed)
            self.assertTrue(any(f.code == "chain_break" for f in result.errors))


def _wait_for(condition, timeout: float = 30.0, interval: float = 0.05, message: str = "condition") -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if condition():
            return
        time.sleep(interval)
    raise AssertionError(f"Timed out after {timeout}s waiting for {message}")


if __name__ == "__main__":
    unittest.main()
