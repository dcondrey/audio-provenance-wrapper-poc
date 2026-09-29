"""Golden fixtures shared with the Rust workspace (tests/fixtures/parity/).

The fixtures are produced by tests/fixtures/parity/generate_parity_fixtures.py from
this Python implementation. This test proves they are still what Python produces,
so a change to the oracle cannot drift silently away from what the Rust tests
(rust/apw-core/tests/time_anchor_parity.rs, rust/apw-daemon/tests/forgery_parity.rs)
check against.
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

from daemon import verify as verify_module
from daemon.common import canonical_json_bytes
from daemon.manifest_builder.generator import derive_forgery_analysis
from daemon.signing import verify_ed25519_signature

PARITY = Path(__file__).parent / "fixtures" / "parity"
SCHEMA = Path(__file__).resolve().parent.parent / "docs" / "manifest.schema.json"

pytestmark_skip = sys.version_info < (3, 12)


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _canonical(value: object) -> bytes:
    return canonical_json_bytes(value)


@unittest.skipIf(pytestmark_skip, "the forgery screen's sum() is only compensated on Python 3.12+")
class ForgeryFixtureTests(unittest.TestCase):
    def test_every_case_is_what_the_oracle_produces_and_covers_each_flag(self):
        cases = json.loads((PARITY / "forgery_analysis.json").read_text())
        flags: set[str] = set()
        for case in cases:
            with self.subTest(case["name"]):
                produced = derive_forgery_analysis(case["events"])
                self.assertEqual(_canonical(produced), _canonical(case["expected"]))
                for analyzer in produced["analyzers"].values():
                    flags.update(flag["name"] for flag in analyzer["flags"])
        self.assertEqual(flags, {
            "too_regular_rms", "too_regular_spectrum", "metronomic_transitions", "superhuman_speed",
            "too_regular_iki", "missing_human_pauses", "superhuman_input_speed",
            "no_fatigue_pattern", "wrong_iki_statistics", "chain_break", "duplicate_hashes",
            "timestamp_reversal",
        })

    def test_fixtures_are_current(self):
        generator = _load("parity_generator", PARITY / "generate_parity_fixtures.py")
        self.assertEqual(
            json.dumps(generator.forgery_cases(), ensure_ascii=False),
            json.dumps(json.loads((PARITY / "forgery_analysis.json").read_text()), ensure_ascii=False),
        )


class TimeAnchorFixtureTests(unittest.TestCase):
    def test_vectors_are_current(self):
        generator = _load("parity_generator", PARITY / "generate_parity_fixtures.py")
        self.assertEqual(
            generator.time_anchor_fixture(),
            json.loads((PARITY / "time_anchor.json").read_text()),
        )

    def test_verifier_vectors_cover_every_finding_code(self):
        vectors = json.loads((PARITY / "time_anchor.json").read_text())["verify"]
        codes = {finding["code"] for vector in vectors for finding in vector["expected_findings"]}
        self.assertEqual(codes, {"time_anchor_invalid", "time_anchor_consistent", "time_anchor_unavailable"})


class SessionFixtureTests(unittest.TestCase):
    def test_session_and_validation_vectors_are_current(self):
        generator = _load("parity_generator", PARITY / "generate_parity_fixtures.py")
        for name, produced in (
            ("session_state.json", generator.session_cases()),
            ("network_validation.json", generator.network_validation_cases()),
        ):
            with self.subTest(name):
                self.assertEqual(produced, json.loads((PARITY / name).read_text()))

    def test_host_environment_and_regression_cases_cover_each_branch(self):
        cases = json.loads((PARITY / "session_state.json").read_text())
        statuses = {case["host_environment"]["status"] for case in cases}
        self.assertEqual(statuses, {"unobserved", "observed", "host_inferred", "host_unrecognised", "conflicting_observations"})
        self.assertTrue(any(case["telemetry_regressions"] > 0 for case in cases))


class ProjectFixtureTests(unittest.TestCase):
    """Golden snapshots and parity vectors the Rust project parsers are checked against."""

    def test_vectors_and_goldens_are_current(self):
        import gzip

        generator = _load("parity_project_generator", PARITY / "generate_project_fixtures.py")
        self.assertEqual(
            json.loads(json.dumps(generator.build_payload(), ensure_ascii=False)),
            json.loads((PARITY / "project_parity.json").read_text()),
        )
        projects = PARITY.parent / "projects"
        for name in ("basic", "edited"):
            self.assertEqual(gzip.decompress((projects / "als" / f"{name}.als").read_bytes()), generator.als_document(name))
            self.assertEqual(
                (projects / "reaper" / f"{name}.rpp").read_bytes(),
                (PARITY.parent / "reaper" / f"{name}.rpp").read_bytes(),
            )
        goldens = sorted(projects.rglob("*.golden.json"))
        self.assertGreaterEqual(len(goldens), 10)
        from daemon.project_formats import parse_project
        from daemon.project_formats._snapshot import golden_json

        for golden in goldens:
            source = golden.with_name(golden.name[: -len(".golden.json")])
            self.assertEqual(golden.read_text(), golden_json(parse_project(source)), source.name)

    def test_the_rehearsal_manifest_carries_a_project_and_an_inferred_host(self):
        manifest = json.loads((PARITY / "manifest_rehearsal.json").read_text())["manifest"]
        self.assertEqual(manifest["session_facts"]["apw:proof_level"], "inferred")
        self.assertEqual(manifest["host_environment"]["status"], "host_inferred")
        self.assertEqual(manifest["host_environment"]["apw:proof_level"], "inferred")
        self.assertNotIn("layer_project_differ_not_active", manifest["apw:unobserved"])


class KeyPathTests(unittest.TestCase):
    """The Python side of the key-path diff. The Rust side is
    rust/apw-cli/tests/manifest_key_paths.rs, which runs the real `apw daemon` over
    the same wire events and compares against this same fixture."""

    def test_a_fresh_python_session_has_the_committed_key_paths(self):
        generator = _load("parity_generator", PARITY / "generate_parity_fixtures.py")
        capture = generator.rehearsal_capture()
        fixture = json.loads((PARITY / "manifest_key_paths.json").read_text())
        self.assertEqual(generator.key_paths(capture["manifest"]), fixture["paths"])
        self.assertEqual(capture["events"], json.loads((PARITY / "synthetic_events.json").read_text()))

    def test_committed_manifest_matches_the_key_paths(self):
        generator = _load("parity_generator", PARITY / "generate_parity_fixtures.py")
        manifest = json.loads((PARITY / "manifest_rehearsal.json").read_text())["manifest"]
        fixture = json.loads((PARITY / "manifest_key_paths.json").read_text())
        self.assertEqual(generator.key_paths(manifest), fixture["paths"])
        for path in ("/host_environment/status", "/observation_coverage/counters/plugin_telemetry_regressions",
                     "/manifest_signature/hardware_cosignature/counter_scope",
                     "/manifest_signature/hardware_cosignature/apw:proof_level", "/time_anchor/status",
                     "/forgery_analysis/analyzers/hash_chain/flags", "/time_anchor_opentimestamps/status",
                     "/time_anchor_opentimestamps/proof_hex", "/time_anchor_opentimestamps/calendars[]/url"):
            self.assertIn(path, fixture["paths"])


class RehearsalManifestTests(unittest.TestCase):
    """A real manifest from the Python daemon carrying both new sections."""

    def setUp(self):
        self.fixture = json.loads((PARITY / "manifest_rehearsal.json").read_text())
        self.manifest = self.fixture["manifest"]

    def test_manifest_satisfies_the_published_schema(self):
        minischema = _load("parity_minischema", PARITY / "minischema.py")
        schema = json.loads(SCHEMA.read_text())
        self.assertEqual(minischema.validate(self.manifest, schema), [])
        self.assertIn("forgery_analysis", self.manifest)
        self.assertEqual(self.manifest["time_anchor"]["status"], "anchored")

    def test_schema_validator_rejects_a_stronger_time_anchor_claim(self):
        minischema = _load("parity_minischema", PARITY / "minischema.py")
        schema = json.loads(SCHEMA.read_text())
        broken = json.loads(json.dumps(self.manifest))
        broken["time_anchor"]["status"] = "verified"
        broken["forgery_analysis"]["suspicion_score"] = 1.5
        errors = minischema.validate(broken, schema)
        self.assertTrue(any("/time_anchor/status" in error for error in errors), errors)
        self.assertTrue(any("/forgery_analysis/suspicion_score" in error for error in errors), errors)

    def test_signing_bytes_and_portable_signature(self):
        portable = {k: v for k, v in self.manifest.items() if k not in {"portable_signature", "manifest_signature"}}
        local = {k: v for k, v in self.manifest.items() if k != "manifest_signature"}
        portable_bytes = canonical_json_bytes(portable)
        local_bytes = json.dumps(local, sort_keys=True, separators=(",", ":")).encode()
        self.assertEqual(hashlib.sha256(portable_bytes).hexdigest(), self.fixture["portable_signing_input_sha256"])
        self.assertEqual(len(portable_bytes), self.fixture["portable_signing_input_length"])
        self.assertEqual(hashlib.sha256(local_bytes).hexdigest(), self.fixture["local_signing_input_sha256"])
        self.assertEqual(len(local_bytes), self.fixture["local_signing_input_length"])
        with tempfile.TemporaryDirectory() as directory:
            key = Path(directory) / "public.key"
            key.write_bytes(bytes.fromhex(self.manifest["portable_signature"]["public_key_hex"]))
            ok, message = verify_ed25519_signature(portable, self.manifest["portable_signature"], key)
        self.assertTrue(ok, message)

    def test_time_anchor_findings(self):
        result = verify_module.VerificationResult()
        verify_module._check_time_anchor(self.manifest, result)
        produced = [
            {"severity": str(f.severity.value if hasattr(f.severity, "value") else f.severity),
             "code": f.code, "message": f.message}
            for f in result.findings
        ]
        self.assertEqual(produced, self.fixture["time_anchor_findings"])
        self.assertEqual([f["code"] for f in produced], ["time_anchor_consistent"])


if __name__ == "__main__":
    unittest.main()
