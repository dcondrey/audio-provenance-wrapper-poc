import unittest

from daemon.schema import validate_manifest_invariants
from daemon.manifest_builder.builder import (
    ExportEvidence,
    IngredientEvidence,
    ManifestBuilder,
    StemEvidence,
)


class ManifestBuilderTests(unittest.TestCase):
    def test_evidence_is_placed_with_its_declared_proof_level(self):
        builder = ManifestBuilder(session_id="s1")
        self.assertIn("hidden_plugin_state", builder.build()["apw:unobserved"])
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
        builder.set_export(ExportEvidence(
            file_path="/tmp/out.wav", file_name="out.wav", sha256="deadbeef", format="wav",
            file_size_bytes=1000000, duration_seconds=30.0, exported_at="2026-05-26T00:00:00Z",
        ))
        builder.add_ingredient(IngredientEvidence(
            file_name="kick.wav", sha256="aabbcc", proof_level="directly_observed", correlation_confidence=0.85,
            audio_fingerprint={"rms": 0.3, "zero_crossing_rate": 0.2},
        ))
        manifest = builder.build()
        self.assertEqual(manifest["session_id"], "s1")
        self.assertEqual(manifest["observed_stems"][0]["stem_id"], "stem-1")
        self.assertEqual(manifest["observed_stems"][0]["source"]["apw:proof_level"], "user_declared")
        self.assertEqual(manifest["export"]["sha256"], "deadbeef")
        self.assertEqual(manifest["export"]["apw:proof_level"], "directly_observed")
        self.assertEqual([i["file_name"] for i in manifest["ingredients"]], ["kick.wav"])

    def test_composite_edits_map_to_c2pa_actions(self):
        builder = ManifestBuilder(session_id="s1")
        builder.set_export(ExportEvidence(
            file_path="/tmp/out.wav", file_name="out.wav", sha256="dead", format="wav",
            file_size_bytes=100, duration_seconds=1.0, exported_at="2026-05-26T00:00:00Z",
        ))
        builder.add_composite_edit({"edit_type": "clip_delete", "confidence": 0.8, "timestamp_ms": 500})
        assertions = {a["label"]: a for a in builder.build()["c2pa_mapping"]["assertions"]}
        self.assertLessEqual({"c2pa.hash.data", "c2pa.actions", "apw.unobserved"}, set(assertions))
        self.assertEqual(assertions["c2pa.actions"]["data"]["actions"][0]["action"], "c2pa.removed")

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

    def test_host_environment_names_the_host_on_the_fight_card(self):
        builder = ManifestBuilder(session_id="s1")
        builder.set_host_environment({
            "status": "observed",
            "host_recognised": True,
            "host_name": "Ableton Live",
            "host_executable_name": "Live",
            "wrapper_format": "VST3",
            "basis": "The plug-in wrapper named the host application that loaded it.",
            "apw:proof_level": "directly_observed",
        })

        manifest = builder.build()

        self.assertEqual(manifest["host_environment"]["host_name"], "Ableton Live")
        self.assertFalse(validate_manifest_invariants(manifest))
        claim = next(
            c for c in manifest["claim_summary"] if c["claim"] == "host_application"
        )
        self.assertEqual(claim["value"], "Ableton Live")
        self.assertEqual(claim["apw:proof_level"], "directly_observed")

    def test_an_unnamed_host_never_reads_as_an_observation(self):
        builder = ManifestBuilder(session_id="s1")
        builder.set_host_environment({
            "status": "host_unrecognised",
            "host_recognised": False,
            "host_name": None,
            "host_executable_name": "SomeDaw",
            "wrapper_format": "AudioUnit",
            "basis": "The plug-in wrapper did not recognise the host application.",
            "apw:proof_level": "unknown_unobserved",
        })

        manifest = builder.build()

        self.assertFalse(validate_manifest_invariants(manifest))
        claim = next(
            c for c in manifest["claim_summary"] if c["claim"] == "host_application"
        )
        self.assertEqual(claim["value"], "unknown")
        self.assertEqual(claim["apw:proof_level"], "unknown_unobserved")

    def test_schema_rejects_a_host_named_without_recognition(self):
        manifest = ManifestBuilder(session_id="s1").build()
        manifest["host_environment"] = {
            "status": "host_unrecognised",
            "host_recognised": True,
            "host_name": "Unknown",
            "apw:proof_level": "directly_observed",
        }

        errors = validate_manifest_invariants(manifest)

        self.assertIn("an unidentified host_environment must remain unknown_unobserved", errors)
        self.assertIn("an unidentified host_environment must not name a host", errors)
        self.assertIn(
            "an unidentified host_environment must not report the host as recognised", errors
        )

    def test_a_manifest_without_a_host_environment_stays_valid(self):
        manifest = ManifestBuilder(session_id="s1").build()

        self.assertNotIn("host_environment", manifest)
        self.assertFalse(validate_manifest_invariants(manifest))
        claim = next(
            c for c in manifest["claim_summary"] if c["claim"] == "host_application"
        )
        self.assertEqual(claim["value"], "unknown")
        self.assertEqual(claim["apw:proof_level"], "unknown_unobserved")


if __name__ == "__main__":
    unittest.main()
