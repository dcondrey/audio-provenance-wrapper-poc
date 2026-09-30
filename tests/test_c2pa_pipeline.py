import json
import tempfile
import types
import unittest
from pathlib import Path

from daemon.__main__ import Daemon
from daemon.hardware_attestation.provider import SoftwareProvider
from daemon.report import render_html_report
from daemon.common import sha256_file
from daemon.schema import validate_manifest_invariants
from daemon.verify import VerificationResult, _check_c2pa_claim, verify_manifest
from tests.audio_files import write_wav


def _project_snapshot(sample_refs: tuple[str, ...]) -> types.SimpleNamespace:
    return types.SimpleNamespace(
        transport_bpm=120.0,
        transport_time_signature=(4, 4),
        transport_loop_on=False,
        track_count=0,
        clip_count=0,
        sample_refs=frozenset(sample_refs),
        tracks=[],
    )


class C2paPipelineTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.export_dir = self.root / "exports"
        self.export_dir.mkdir()
        (self.root / "samples").mkdir()
        self.daemon = Daemon(
            udp_port=0,
            evidence_dir=self.root / "evidence",
            sample_dir=self.root / "samples",
            export_dir=self.export_dir,
            manifest_dir=self.root / "manifests",
            hardware_provider=SoftwareProvider(self.root / "signing-key.bin"),
            provenance_store=self.root / "provenance",
            generate_html_report=False,
        )
        self.trust_anchor = self.root / "provenance" / "ca" / "root_cert.pem"
        self.addCleanup(self._tmp.cleanup)
        self.addCleanup(self.daemon.receiver.sock.close)

    def _generate(self, export_path: Path) -> dict:
        manifest_path = self.daemon._generate_manifest(export_path)
        self.manifest_path = manifest_path
        return json.loads(manifest_path.read_text())

    def _verify(self, **kwargs):
        """Verify with the keys and anchor this daemon actually signed with.

        The verifier evaluates C2PA trust against a locally held anchor, never
        against the one recorded inside the manifest, so a test store's root has
        to be supplied the same way the daemon supplies its own.
        """
        return verify_manifest(
            self.manifest_path,
            signing_key_path=self.daemon._signing_key_path,
            public_key_path=self.daemon._portable_signer.public_key_path,
            trust_anchor_path=self.trust_anchor,
            **kwargs,
        )

    def test_export_is_signed_without_rewriting_the_exported_file(self):
        export = self.export_dir / "mixdown.wav"
        write_wav(export)
        original = export.read_bytes()

        manifest = self._generate(export)
        claim = manifest["c2pa_claim"]

        self.assertEqual(claim["status"], "embedded")
        self.assertEqual(claim["validation"]["state"], "verified")
        self.assertEqual(claim["validation"]["failure_codes"], [])
        self.assertEqual(claim["validation"]["trust_anchor_scope"], "self_issued_local_root_only")
        self.assertTrue(claim["source_sha256_matches_export"])
        self.assertEqual(claim["hard_binding"]["source_sha256"], manifest["export"]["sha256"])
        self.assertEqual(claim["signer"]["signer_identity"], "not_established")
        self.assertEqual(claim["signer"]["apw:proof_level"], "user_declared")
        # A self-issued chain never establishes identity, whatever c2pa reports.
        self.assertNotEqual(claim["signer"]["apw:proof_level"], "externally_verified")

        self.assertEqual(export.read_bytes(), original)
        signed = self.daemon.manifest_dir / claim["signed_asset"]["relative_path"]
        self.assertTrue(signed.is_file())
        self.assertNotEqual(signed.resolve(), export.resolve())
        self.assertEqual(validate_manifest_invariants(manifest), [])

        result = self._verify()
        codes = {finding.code for finding in result.findings}
        self.assertIn("c2pa_claim_verified", codes)
        self.assertIn("portable_signature_valid", codes)
        self.assertIn("signed_content_hash_valid", codes)
        self.assertIn("local_signature_valid", codes)

        # The claim must be assembled before both signing steps, so editing it
        # has to break the portable Ed25519 signature and the signed-content
        # hash. A refactor that signs first would leave this manifest valid.
        manifest["c2pa_claim"]["validation"]["state"] = "nothing_found"
        self.manifest_path.write_text(json.dumps(manifest, indent=2))
        resigned = {finding.code for finding in self._verify().findings}
        self.assertIn("portable_signature_invalid", resigned)
        self.assertIn("tampered", resigned)

    def test_tampering_the_signed_asset_is_reported_as_changed(self):
        export = self.export_dir / "mixdown.wav"
        write_wav(export)
        manifest = self._generate(export)
        signed = self.daemon.manifest_dir / manifest["c2pa_claim"]["signed_asset"]["relative_path"]

        payload = bytearray(signed.read_bytes())
        payload[2000] ^= 0xFF
        signed.write_bytes(bytes(payload))

        result = self._verify()
        codes = {finding.code for finding in result.findings}
        self.assertIn("c2pa_asset_hash_mismatch", codes)
        self.assertEqual(result.outcome, "changed")

        # With the recorded digest updated to the tampered bytes, the cheap hash
        # check passes and only the real C2PA hard binding can still catch it.
        manifest["c2pa_claim"]["signed_asset"]["sha256"] = sha256_file(signed)
        rebound = VerificationResult()
        _check_c2pa_claim(
            manifest, self.manifest_path, rebound, self.trust_anchor.read_text()
        )
        rebound_codes = {finding.code for finding in rebound.findings}
        self.assertIn("c2pa_hard_binding_broken", rebound_codes)
        self.assertIn("c2pa_claim_state_changed", rebound_codes)
        self.assertEqual(rebound.outcome, "changed")

    def test_unsignable_export_records_an_honest_unavailable_claim(self):
        export = self.export_dir / "float.wav"
        write_wav(export, float32=True)

        manifest = self._generate(export)
        claim = manifest["c2pa_claim"]

        self.assertEqual(claim["status"], "unavailable")
        self.assertEqual(claim["apw:proof_level"], "unknown_unobserved")
        self.assertIn("32-bit float", claim["reason"])
        self.assertEqual(validate_manifest_invariants(manifest), [])

        row = next(c for c in manifest["claim_summary"] if c["claim"] == "embedded_c2pa_claim")
        self.assertEqual(row["apw:proof_level"], "unknown_unobserved")
        result = self._verify()
        codes = {finding.code for finding in result.findings}
        self.assertIn("c2pa_claim_unavailable", codes)
        self.assertNotIn("c2pa_claim_verified", codes)
        # A session whose export was never signed is not verified, whatever else
        # about it checks out.
        self.assertEqual(result.outcome, "incomplete")

    def test_unobserved_project_sample_becomes_an_ingredient_with_its_hash(self):
        unobserved = self.root / "never_observed.wav"
        write_wav(unobserved, 1000)
        self.daemon._latest_project_snapshot = _project_snapshot(
            (str(unobserved), "Samples/Imported/missing.wav")
        )
        export = self.export_dir / "mixdown.wav"
        write_wav(export)

        manifest = self._generate(export)
        claim = manifest["c2pa_claim"]

        node = next(n for n in claim["ingredients"] if n["title"] == "never_observed.wav")
        self.assertEqual(node["apw:proof_level"], "unknown_unobserved")
        self.assertEqual(node["relationship"], "inputTo")
        self.assertEqual(len(node["sha256"]), 64)

        unresolved = claim["unresolved_ingredient_references"]
        self.assertEqual(len(unresolved), 1)
        self.assertIn("Samples/Imported/missing.wav", unresolved[0]["reference"])
        self.assertEqual(unresolved[0]["apw:proof_level"], "unknown_unobserved")
        self.assertEqual(validate_manifest_invariants(manifest), [])

    def test_report_renders_a_hostile_claim_without_raising(self):
        hostile = {
            "session_id": "s",
            "c2pa_claim": {
                "status": "embedded",
                "validation": "not-an-object",
                "signer": [1],
                "ingredients": "not-a-list",
                "unresolved_ingredient_references": 5,
            },
        }
        html = render_html_report(hostile)
        self.assertIn("Embedded C2PA claim", html)
        self.assertIn("Embedded C2PA claim", render_html_report({}))

    def test_schema_rejects_a_self_issued_signer_presented_as_verified_identity(self):
        base = {
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
                "status": "unknown_coverage", "apw:proof_level": "unknown_unobserved",
            },
            "daemon_receipt_acknowledgement": {
                "status": "unknown", "apw:proof_level": "unknown_unobserved",
            },
            "apw:unobserved": [],
            "c2pa_mapping": {},
            "c2pa_claim": {
                "status": "embedded",
                "source_sha256_matches_export": True,
                "hard_binding": {"algorithm": "sha256"},
                "validation": {
                    "state": "verified",
                    "trust_anchor_scope": "self_issued_local_root_only",
                },
                "signer": {
                    "signer_identity": "not_established",
                    "apw:proof_level": "user_declared",
                },
                "ingredients": [],
                "apw:proof_level": "directly_observed",
            },
        }
        self.assertEqual(validate_manifest_invariants(base), [])

        overclaimed = json.loads(json.dumps(base))
        overclaimed["c2pa_claim"]["signer"]["apw:proof_level"] = "externally_verified"
        overclaimed["c2pa_claim"]["signer"]["signer_identity"] = "verified_creator"
        errors = validate_manifest_invariants(overclaimed)
        self.assertTrue(any("externally_verified" in error for error in errors))
        self.assertTrue(any("not_established" in error for error in errors))

        undigested = json.loads(json.dumps(base))
        undigested["c2pa_claim"]["ingredients"] = [
            {"title": "x", "apw:proof_level": "unknown_unobserved"}
        ]
        self.assertTrue(
            any("must record a digest" in error for error in validate_manifest_invariants(undigested))
        )


if __name__ == "__main__":
    unittest.main()
