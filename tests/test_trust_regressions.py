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


class VerifierTrustBoundaryTests(unittest.TestCase):
    """Each case here is a demonstrated forgery the verifier used to accept."""

    @classmethod
    def setUpClass(cls) -> None:
        import math
        import struct
        import wave

        from daemon.__main__ import Daemon
        from daemon.hardware_attestation.provider import SoftwareProvider

        cls._tmp = tempfile.TemporaryDirectory()
        root = Path(cls._tmp.name)
        exports = root / "exports"
        exports.mkdir()
        (root / "samples").mkdir()
        cls.root = root
        cls.store = root / "provenance"
        cls.anchor = cls.store / "ca" / "root_cert.pem"
        daemon = Daemon(
            udp_port=0,
            evidence_dir=root / "evidence",
            sample_dir=root / "samples",
            export_dir=exports,
            manifest_dir=root / "manifests",
            hardware_provider=SoftwareProvider(root / "signing-key.bin"),
            provenance_store=cls.store,
            generate_html_report=False,
        )
        # Real received evidence, so the bound-prefix and coverage re-derivation
        # checks have something to run against.
        previous = "genesis"
        for sequence in range(1, 4):
            event = {
                "event_type": "buffer_hash", "proof_level": "directly_observed",
                "plugin_instance_id": "plug-1", "plugin_capture_session_id": "s1",
                "event_sequence": sequence, "window_hash": f"window-{sequence}",
                "prev_hash": previous, "rms_level": 0.2, "zero_crossing_rate": 0.1,
                "sample_rate_hz": 44100, "window_size_samples": 4096, "channel_count": 1,
                "timestamp_ms": 1000 + sequence,
            }
            previous = event["window_hash"]
            daemon._record_plugin_event(daemon.receiver.process_packet(
                json.dumps(event).encode()), "audio_buffer")

        export = exports / "take.wav"
        with wave.open(str(export), "wb") as handle:
            handle.setnchannels(1)
            handle.setsampwidth(2)
            handle.setframerate(44100)
            handle.writeframes(b"".join(
                struct.pack("<h", int(9000 * math.sin(i / 25))) for i in range(44100)
            ))
        cls.manifest_path = daemon._generate_manifest(export)
        cls.signing_key = daemon._signing_key_path
        cls.public_key = daemon._portable_signer.public_key_path
        daemon.receiver.sock.close()
        cls.addClassCleanup(cls._tmp.cleanup)

    def _verify(self, manifest_path=None, **kwargs):
        from daemon.verify import verify_manifest

        kwargs.setdefault("signing_key_path", self.signing_key)
        kwargs.setdefault("public_key_path", self.public_key)
        kwargs.setdefault("trust_anchor_path", self.anchor)
        return verify_manifest(manifest_path or self.manifest_path, **kwargs)

    def _forge(self, name: str, mutate) -> Path:
        data = json.loads(self.manifest_path.read_text())
        mutate(data)
        path = Path(self._tmp.name) / name
        path.write_text(json.dumps(data, indent=2))
        return path

    def test_baseline_manifest_verifies_with_every_check_run(self):
        result = self._verify()
        self.assertEqual(result.unchecked, [])
        self.assertEqual(result.outcome, "verified")

    def test_a_package_without_its_signed_asset_is_not_verified(self):
        """Deleting the artifacts from a package used to still print PASS.

        Every "I could not check this" path emits a warning, and the outcome fell
        through to "verified", so the founder-facing top line was the strongest
        word the tool has for a package whose audio was never checked.
        """
        recipient = Path(self._tmp.name) / "recipient"
        recipient.mkdir()
        target = recipient / self.manifest_path.name
        target.write_text(self.manifest_path.read_text())
        result = self._verify(target)
        self.assertEqual(result.errors, [])
        self.assertEqual(result.outcome, "incomplete")
        self.assertIn("c2pa_claim", result.unchecked)
        self.assertIn("c2pa_asset_unavailable", {f.code for f in result.warnings})

    def test_a_manifest_with_nothing_checkable_is_not_verified(self):
        """A validly signed document in which literally nothing was verified."""
        from daemon.signing import Ed25519Signer

        root = Path(self._tmp.name) / "hollow"
        root.mkdir()
        signer = Ed25519Signer(root / "private.key", root / "public.key")
        manifest = {
            "apw_version": "0.9.0",
            "schema": "audio-provenance-manifest-v0",
            "session_id": "s",
            "capture_session": {"apw:proof_level": "directly_observed"},
            "created_at": "now",
            "observed_stems": [],
            "claim_summary": [],
            "stem_export_association": {
                "status": "unavailable", "apw:proof_level": "unknown_unobserved",
            },
            "observation_coverage": {
                "status": "complete_observed_path",
                "counters": {},
                "apw:proof_level": "inferred",
            },
            "daemon_receipt_acknowledgement": {
                "status": "issued", "apw:proof_level": "directly_observed",
            },
            "apw:unobserved": [],
            "c2pa_mapping": {"assertions": [{"label": "apw.unobserved"}]},
            "export": {
                "file_path": "/nonexistent/masterpiece.wav",
                "file_name": "masterpiece.wav",
                "sha256": "e" * 64,
                "apw:proof_level": "directly_observed",
            },
        }
        # complete_observed_path counters would fail the schema; the point of the
        # case is the outcome, not the coverage grade.
        manifest["observation_coverage"]["status"] = "unknown_coverage"
        manifest["observation_coverage"]["apw:proof_level"] = "unknown_unobserved"
        manifest["portable_signature"] = signer.sign_manifest(manifest)
        path = root / "hollow_manifest.json"
        path.write_text(json.dumps(manifest, indent=2))

        result = self._verify(path, public_key_path=root / "public.key")
        self.assertEqual(result.errors, [])
        self.assertEqual(result.outcome, "incomplete")
        self.assertEqual(
            sorted(result.unchecked), ["c2pa_claim", "evidence_binding", "export_binding"]
        )

    def test_c2pa_trust_anchor_is_held_locally_not_read_from_the_manifest(self):
        """A forger supplying their own root inside the manifest must not gain trust."""
        from daemon.provenance.local_reference import LocalReferenceProvider

        foreign_store = Path(self._tmp.name) / "foreign"
        LocalReferenceProvider(store_dir=foreign_store)
        foreign = self._verify(trust_anchor_path=foreign_store / "ca" / "root_cert.pem")
        self.assertIn(
            "c2pa_trust_anchor_untrusted", {f.code for f in foreign.findings}
        )
        self.assertEqual(foreign.outcome, "untrusted")

        unanchored = self._verify(trust_anchor_path=Path("/nonexistent/root.pem"))
        self.assertEqual(unanchored.errors, [])
        self.assertIn("c2pa_claim", unanchored.unchecked)
        self.assertNotEqual(unanchored.outcome, "verified")

    def test_portable_signature_cannot_supply_the_key_that_verifies_it(self):
        import hashlib

        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

        from daemon.common import canonical_json_bytes

        def resign(data):
            data["core_principle"] = "Altered demo claim."
            unsigned = {
                key: value for key, value in data.items()
                if key not in {"portable_signature", "manifest_signature"}
            }
            key = Ed25519PrivateKey.generate()
            content = canonical_json_bytes(unsigned)
            data["portable_signature"] = {
                **data["portable_signature"],
                "public_key_hex": key.public_key().public_bytes_raw().hex(),
                "signature_hex": key.sign(content).hex(),
                "signed_content_hash": hashlib.sha256(content).hexdigest(),
            }
            body = {k: v for k, v in data.items() if k != "manifest_signature"}
            data["manifest_signature"]["signed_content_hash"] = hashlib.sha256(
                json.dumps(body, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest()

        result = self._verify(self._forge("resigned.json", resign), signing_key_path=None)
        self.assertIn("portable_signature_invalid", {f.code for f in result.errors})
        self.assertEqual(result.outcome, "changed")

    def test_manifest_signature_cannot_switch_off_its_own_verification(self):
        """trust_scope and an empty seal both used to buy silence."""
        swapped = self._forge(
            "swapped.json",
            lambda data: data["manifest_signature"].update(
                {"trust_scope": "hardware_provider", "algorithm": "hardware_provider"}
            ),
        )
        self.assertIn(
            "signature_algorithm_unrecognised", {f.code for f in self._verify(swapped).errors}
        )

        def hollow(data):
            data["manifest_signature"] = {
                "algorithm": "hmac-sha256-local", "apw:proof_level": "unknown_unobserved",
            }

        self.assertIn(
            "signature_incomplete",
            {f.code for f in self._verify(self._forge("hollow.json", hollow)).errors},
        )

    def test_evidence_binding_cannot_steer_the_verifier_out_of_its_directory(self):
        """The bound file names are untrusted and are hashed pre-authentication."""
        secret = self.root / "secret.key"
        secret.write_bytes(b"\x01" * 32)

        def traverse(data):
            data["evidence_binding"]["evidence_directory"] = str(self.root / "evidence")
            data["evidence_binding"]["evidence_files"] = {
                "../secret.key": {"sha256": "0" * 64, "byte_length": 32},
            }
            data["evidence_binding"]["evidence_file_hashes"] = {"../secret.key": "0" * 64}

        result = self._verify(self._forge("traversal.json", traverse), signing_key_path=None)
        self.assertIn("evidence_binding_invalid", {f.code for f in result.errors})
        self.assertFalse(
            any("secret.key" in finding.message for finding in result.findings
                if finding.code == "evidence_hash_valid"),
            "the verifier confirmed a digest for a path outside the bound directory",
        )


class PluginTelemetryBoundaryTests(unittest.TestCase):
    def test_host_bypass_is_visible_and_cannot_grade_complete(self):
        from daemon.__main__ import Daemon

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            daemon = Daemon(
                udp_port=0,
                evidence_dir=root / "evidence",
                sample_dir=root / "samples",
                manifest_dir=root / "manifests",
                provenance_store=root / "provenance",
                generate_html_report=False,
            )
            self.addCleanup(daemon.receiver.sock.close)
            daemon._record_plugin_event({
                "event_type": "buffer_hash",
                "plugin_instance_id": "plug-1",
                "telemetry": {
                    "buffers_submitted": 1, "samples_submitted": 4096,
                    "windows_hashed": 1, "fifo_samples_dropped": 0,
                    "fifo_windows_dropped": 0, "midi_events_dropped": 0,
                    "bypassed_buffers": 2, "bypassed_samples": 1024,
                    "events_prepared": 1, "udp_sends_attempted": 1,
                    "udp_sends_failed": 0,
                },
            }, "audio_buffer")

            coverage = daemon._derive_coverage(1)
            self.assertEqual(coverage["status"], "partial_observed_path")
            self.assertEqual(coverage["counters"]["bypassed_buffers"], 2)

    def test_cumulative_counters_cannot_be_walked_backwards(self):
        """A spoofed datagram must not upgrade a lossy session's coverage.

        plugin_instance_id travels in cleartext in every outbound event, so any
        local process can reuse it; the discriminating invariant is that a
        cumulative counter never decreases.
        """
        from daemon.__main__ import Daemon

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            daemon = Daemon(
                udp_port=0,
                evidence_dir=root / "evidence",
                sample_dir=root / "samples",
                manifest_dir=root / "manifests",
                provenance_store=root / "provenance",
                generate_html_report=False,
            )
            self.addCleanup(daemon.receiver.sock.close)
            honest = {
                "event_type": "buffer_hash",
                "plugin_instance_id": "plug-1",
                "telemetry": {
                    "buffers_submitted": 40, "samples_submitted": 40, "windows_hashed": 1,
                    "fifo_samples_dropped": 8192, "fifo_windows_dropped": 2,
                    "midi_events_dropped": 0, "bypassed_buffers": 0,
                    "bypassed_samples": 0, "events_prepared": 1,
                    "udp_sends_attempted": 1, "udp_sends_failed": 0,
                },
            }
            daemon._record_plugin_event(honest, "audio_buffer")
            self.assertEqual(daemon._derive_coverage(1)["status"], "partial_observed_path")

            spoofed = {
                **honest,
                "telemetry": {
                    **honest["telemetry"], "fifo_samples_dropped": 0, "fifo_windows_dropped": 0,
                },
            }
            daemon._record_plugin_event(spoofed, "audio_buffer")
            coverage = daemon._derive_coverage(2)
            self.assertEqual(coverage["status"], "partial_observed_path")
            self.assertEqual(coverage["counters"]["fifo_samples_dropped"], 8192)
            self.assertEqual(coverage["counters"]["plugin_telemetry_regressions"], 2)

    def test_nested_telemetry_is_validated_at_the_udp_boundary(self):
        from daemon.evidence_receiver.taxonomy import validate_network_event

        base = {
            "event_type": "buffer_hash", "proof_level": "directly_observed",
            "window_hash": "h", "prev_hash": "genesis",
            "rms_level": 0.2, "zero_crossing_rate": 0.1,
        }
        self.assertTrue(validate_network_event({**base, "telemetry": {"windows_hashed": 4}})[0])
        for poisoned in (
            {"fifo_samples_dropped": -999999},
            {"arbitrary_attacker_key": 2**70},
            {"flag": True},
            {"windows_hashed": "many"},
        ):
            valid, reason = validate_network_event({**base, "telemetry": poisoned})
            self.assertFalse(valid, f"{poisoned} was accepted: {reason}")
        # A non-finite anywhere in the event, not only in the numeric whitelist.
        self.assertFalse(validate_network_event({**base, "junk": float("nan")})[0])
        self.assertFalse(validate_network_event({**base, "timestamp_ms": "soon"})[0])


class ProvenanceRegistryTrustTests(unittest.TestCase):
    def test_a_registry_record_must_authenticate_itself(self):
        """registry.jsonl is a plain append-only file anyone can write to.

        verify(asset, None) is the documented "I only have the audio" path; it
        used to report VERIFIED, with a reason claiming a signature validated,
        for a record whose signature was never checked.
        """
        import math
        import struct
        import wave

        from daemon.common import canonical_json_bytes, sha256_file
        from daemon.provenance import VerificationState
        from daemon.provenance.local_reference import LocalReferenceProvider

        def write_wav(path, frequency):
            with wave.open(str(path), "wb") as handle:
                handle.setnchannels(1)
                handle.setsampwidth(2)
                handle.setframerate(44100)
                handle.writeframes(b"".join(
                    struct.pack("<h", int(9000 * math.sin(2 * math.pi * frequency * i / 44100)))
                    for i in range(44100 * 6)
                ))
            return path

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            provider = LocalReferenceProvider(store_dir=root / "store")
            asset = write_wav(root / "asset.wav", 220.0)
            mark = provider.embed_mark(asset, {"take": 1})
            provider.register({"content_sha256": sha256_file(asset), "mark_id": mark["mark_id"]})
            self.assertEqual(
                provider.verify(asset, None)["state"], VerificationState.VERIFIED.value
            )

            # Append a record carrying a genuine chain but a fabricated subject.
            genuine = json.loads(provider.registry_path.read_text().splitlines()[-1])
            other = write_wav(root / "other.wav", 880.0)
            forged = {**genuine, "content_sha256": sha256_file(other)}
            with provider.registry_path.open("ab") as handle:
                handle.write(canonical_json_bytes(forged) + b"\n")
            result = provider.verify(other, None)
            self.assertEqual(
                result["state"], VerificationState.MARK_FOUND_CLAIM_NOT_TRUSTED.value
            )
            self.assertIn("receipt signature", result["reason"])


class C2paClassificationTests(unittest.TestCase):
    def test_an_untrusted_signer_outranks_a_broken_hard_binding(self):
        """"Registered but changed" claims the signer WAS trusted.

        Anyone can self-sign an asset and flip a byte; reporting that as a
        registration this system recognises is stronger than the truth.
        """
        from daemon.c2pa_engine.verifier import (
            MARK_FOUND_CLAIM_NOT_TRUSTED,
            REGISTERED_BUT_CHANGED,
            _classify,
        )

        failures = ("signingCredential.untrusted", "assertion.dataHash.mismatch")
        state, detail = _classify("Invalid", failures, trust_evaluated=False, credential_trusted=False)
        self.assertEqual(state, MARK_FOUND_CLAIM_NOT_TRUSTED)
        self.assertIn("assertion.dataHash.mismatch", detail)

        state, _ = _classify("Invalid", failures, trust_evaluated=True, credential_trusted=False)
        self.assertEqual(state, MARK_FOUND_CLAIM_NOT_TRUSTED)

        state, detail = _classify(
            "Invalid", ("assertion.dataHash.mismatch",),
            trust_evaluated=True, credential_trusted=True,
        )
        self.assertEqual(state, REGISTERED_BUT_CHANGED)

        # A trusted credential with a non-hash failure must not be described as
        # having failed to chain to an anchor.
        state, detail = _classify(
            "Trusted", ("assertion.somethingElse",), trust_evaluated=True, credential_trusted=True
        )
        self.assertEqual(state, MARK_FOUND_CLAIM_NOT_TRUSTED)
        self.assertNotIn("did not chain", detail)


class CoverageSchemaAgreementTests(unittest.TestCase):
    def test_a_stray_datagram_does_not_make_the_daemon_emit_an_invalid_manifest(self):
        """derive_coverage and the schema must assert the same ACK rule.

        The daemon acknowledges every received packet, rejections included, so a
        rule of one ACK per accepted *event* is violated by a single stray
        datagram from any local process. The manifest still graded
        complete_observed_path and the daemon's own verifier then rejected it as
        schema_invalid, turning a clean capture into outcome "untrusted".
        """
        import socket

        from daemon.__main__ import Daemon
        from daemon.manifest_builder.builder import ManifestBuilder
        from daemon.schema import validate_manifest_invariants

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            daemon = Daemon(
                udp_port=0,
                evidence_dir=root / "evidence",
                sample_dir=root / "samples",
                manifest_dir=root / "manifests",
                provenance_store=root / "provenance",
                generate_html_report=False,
            )
            self.addCleanup(daemon.receiver.sock.close)
            reply_sink = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            reply_sink.bind(("127.0.0.1", 0))
            self.addCleanup(reply_sink.close)
            address = reply_sink.getsockname()

            previous = "genesis"
            for sequence in range(1, 4):
                packet = json.dumps({
                    "event_type": "buffer_hash", "proof_level": "directly_observed",
                    "plugin_instance_id": "plug-1", "plugin_capture_session_id": "s1",
                    "event_sequence": sequence, "window_hash": f"window-{sequence}",
                    "prev_hash": previous, "rms_level": 0.2, "zero_crossing_rate": 0.1,
                    "sample_rate_hz": 44100, "window_size_samples": 4096, "channel_count": 1,
                    "timestamp_ms": 1000 + sequence,
                    "telemetry": {
                        "buffers_submitted": sequence, "samples_submitted": sequence * 4096,
                        "windows_hashed": sequence, "fifo_samples_dropped": 0,
                        "fifo_windows_dropped": 0, "midi_events_dropped": 0,
                        "bypassed_buffers": 0, "bypassed_samples": 0,
                        "events_prepared": sequence, "udp_sends_attempted": sequence,
                        "udp_sends_failed": 0,
                    },
                }).encode()
                previous = f"window-{sequence}"
                event, ack = daemon.receiver.process_packet_with_ack(packet)
                daemon.receiver.send_acknowledgement(address, ack)
                daemon._record_plugin_event(event, "audio_buffer")

            clean = daemon._derive_coverage(3)
            self.assertEqual(clean["status"], "complete_observed_path")

            # One unauthenticated datagram from any local process.
            _rejected, ack = daemon.receiver.process_packet_with_ack(b"{}")
            daemon.receiver.send_acknowledgement(address, ack)
            diagnostics = daemon.receiver.diagnostics()
            self.assertEqual(diagnostics["packets_received"], diagnostics["events_received"] + 1)

            coverage = daemon._derive_coverage(3)
            self.assertEqual(coverage["status"], "complete_observed_path")

            builder = ManifestBuilder(session_id=daemon.session_id)
            builder.coverage = coverage
            manifest = builder.build()
            manifest["daemon_receipt_acknowledgement"] = daemon.receiver.receipt_summary()
            self.assertEqual(validate_manifest_invariants(manifest), [])
