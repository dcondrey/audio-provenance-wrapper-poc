import json
import tempfile
import unittest
from pathlib import Path

from daemon.manifest_builder.builder import (
    ExportEvidence,
    IngredientEvidence,
    ManifestBuilder,
    StemEvidence,
)


class ManifestBuilderTests(unittest.TestCase):
    def test_minimal_manifest(self):
        builder = ManifestBuilder(session_id="test-session")
        manifest = builder.build()
        self.assertEqual(manifest["session_id"], "test-session")
        self.assertEqual(manifest["apw_version"], "0.9.0")
        self.assertIn("apw:unobserved", manifest)
        self.assertIn("c2pa_mapping", manifest)

    def test_stem_included(self):
        builder = ManifestBuilder(session_id="s1")
        builder.add_stem(StemEvidence(
            stem_id="stem-1",
            hash_chain_root="abc123",
            hash_chain_length=100,
            first_observed_ms=1000,
            last_observed_ms=2000,
            sample_rate_hz=44100,
            channel_count=2,
            source_category="audio_interface_recording",
            proof_level="directly_observed",
            source_category_proof_level="user_declared",
        ))
        manifest = builder.build()
        self.assertEqual(len(manifest["observed_stems"]), 1)
        self.assertEqual(manifest["observed_stems"][0]["stem_id"], "stem-1")
        self.assertEqual(
            manifest["observed_stems"][0]["source"]["apw:proof_level"],
            "user_declared",
        )

    def test_export_included(self):
        builder = ManifestBuilder(session_id="s1")
        builder.set_export(ExportEvidence(
            file_path="/tmp/out.wav",
            file_name="out.wav",
            sha256="deadbeef",
            format="wav",
            file_size_bytes=1000000,
            duration_seconds=30.0,
            exported_at="2026-05-26T00:00:00Z",
        ))
        manifest = builder.build()
        self.assertEqual(manifest["export"]["sha256"], "deadbeef")
        self.assertEqual(manifest["export"]["apw:proof_level"], "directly_observed")

    def test_ingredients_included(self):
        builder = ManifestBuilder(session_id="s1")
        builder.add_ingredient(IngredientEvidence(
            file_name="kick.wav",
            sha256="aabbcc",
            proof_level="directly_observed",
            correlation_confidence=0.85,
            audio_fingerprint={"rms": 0.3, "zero_crossing_rate": 0.2},
        ))
        manifest = builder.build()
        self.assertEqual(len(manifest["ingredients"]), 1)
        self.assertEqual(manifest["ingredients"][0]["file_name"], "kick.wav")

    def test_c2pa_assertions_generated(self):
        builder = ManifestBuilder(session_id="s1")
        builder.set_export(ExportEvidence(
            file_path="/tmp/out.wav", file_name="out.wav", sha256="dead",
            format="wav", file_size_bytes=100, duration_seconds=1.0,
            exported_at="2026-05-26T00:00:00Z",
        ))
        builder.add_composite_edit({
            "edit_type": "clip_paste",
            "confidence": 0.9,
            "timestamp_ms": 1000,
        })
        manifest = builder.build()
        labels = [a["label"] for a in manifest["c2pa_mapping"]["assertions"]]
        self.assertIn("c2pa.hash.data", labels)
        self.assertIn("c2pa.actions", labels)
        self.assertIn("apw.unobserved", labels)

    def test_unobserved_always_present(self):
        builder = ManifestBuilder(session_id="s1")
        manifest = builder.build()
        self.assertIn("hidden_plugin_state", manifest["apw:unobserved"])

    def test_session_cooccurrence_does_not_establish_association(self):
        builder = ManifestBuilder(session_id="s1")
        builder.add_stem(StemEvidence(
            stem_id="stem-1", hash_chain_root="abc123", hash_chain_length=2,
            first_observed_ms=1000, last_observed_ms=2000, sample_rate_hz=48000,
            channel_count=2, source_category="unknown", proof_level="directly_observed",
        ))
        builder.set_export(ExportEvidence(
            file_path="/tmp/out.wav", file_name="out.wav", sha256="deadbeef",
            format="wav", file_size_bytes=100, duration_seconds=1.0,
            exported_at="2026-05-26T00:00:00Z",
        ))

        manifest = builder.build()

        association = manifest["stem_export_association"]
        self.assertEqual(association["status"], "not_established")
        self.assertEqual(association["apw:proof_level"], "unknown_unobserved")
        full_provenance = next(
            claim for claim in manifest["claim_summary"]
            if claim["claim"] == "full_ableton_provenance"
        )
        self.assertFalse(full_provenance["value"])
        self.assertEqual(full_provenance["apw:proof_level"], "unknown_unobserved")

    def test_write_json(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            path = Path(tmp_dir) / "manifest.json"
            builder = ManifestBuilder(session_id="s1")
            builder.write_json(path)
            self.assertTrue(path.exists())
            data = json.loads(path.read_text())
            self.assertEqual(data["session_id"], "s1")

    def test_composite_edit_maps_to_c2pa_action(self):
        builder = ManifestBuilder(session_id="s1")
        builder.add_composite_edit({"edit_type": "clip_delete", "confidence": 0.8, "timestamp_ms": 500})
        manifest = builder.build()
        actions_assertion = next(
            a for a in manifest["c2pa_mapping"]["assertions"] if a["label"] == "c2pa.actions"
        )
        self.assertEqual(actions_assertion["data"]["actions"][0]["action"], "c2pa.removed")


if __name__ == "__main__":
    unittest.main()
