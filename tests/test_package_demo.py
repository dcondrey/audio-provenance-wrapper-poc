import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


def _load_packager():
    spec = importlib.util.spec_from_file_location(
        "_package_demo", REPO / "scripts" / "package_demo.py"
    )
    module = importlib.util.module_from_spec(spec)
    # @dataclass resolves annotations through sys.modules[cls.__module__].
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


packager = _load_packager()


def _manifest(export_path: Path, digest: str, association: dict) -> dict:
    return {
        "session_id": "synthetic-test",
        "core_principle": "Never claim full DAW provenance.",
        "claim_summary": [],
        "apw:unobserved": [],
        "observation_coverage": {"status": "unobserved", "basis": "no routed events"},
        "daemon_receipt_acknowledgement": {
            "status": "none", "counters": {}, "scope": "test",
        },
        "stem_export_association": association,
        "evidence_binding": {
            "chain_length": 0,
            "last_window_hash": "0" * 64,
            "evidence_directory": "/nonexistent",
            "evidence_files": {},
        },
        "export": {
            "file_path": str(export_path),
            "file_name": export_path.name,
            "sha256": digest,
            "file_size_bytes": 4,
            "apw:proof_level": "directly_observed",
        },
        "presentation": {},
    }


class UnavailableAssociationTest(unittest.TestCase):
    """An export-only session leaves confidence and offset null.

    The runbook's own recovery step produces exactly this manifest, and formatting
    those nulls as floats crashed the documented "package to send" step.
    """

    def test_readme_renders_a_null_association_without_crashing(self):
        association = {
            "status": "unavailable",
            "confidence": None,
            "matched_coverage": 0.0,
            "best_offset_seconds": None,
            "apw:proof_level": "unknown_unobserved",
        }
        data = _manifest(Path("/nonexistent/x.wav"), "a" * 64, association)
        readme = packager.build_readme(
            data,
            {"outcome": "verified", "qualified_scope": "local"},
            "synthetic",
            [],
            None,
            [],
            packager.PackagedPrimaries("x_manifest.json", None, (), (), ()),
        )
        self.assertIn("unavailable (confidence n/a, coverage 0.00, offset n/a)", readme)


class PackagedExportIntegrityTest(unittest.TestCase):
    """The package must never ship audio the manifest does not bind."""

    def test_export_whose_bytes_do_not_match_the_manifest_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            export = root / "take.wav"
            export.write_bytes(b"REAL")
            data = _manifest(export, "b" * 64, {"status": "unavailable", "apw:proof_level": "unknown_unobserved"})
            with self.assertRaises(SystemExit) as caught:
                packager.copy_primaries(data, "take_manifest.json", root / "pkg")
            self.assertIn("Refusing to ship an export", str(caught.exception))

    def test_matching_export_is_copied_and_named_in_the_package(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            export = root / "take.wav"
            export.write_bytes(b"REAL")
            digest = packager.sha256_file(export)
            data = _manifest(export, digest, {"status": "unavailable", "apw:proof_level": "unknown_unobserved"})
            packaged = packager.copy_primaries(data, "take_manifest.json", root / "pkg")
            self.assertEqual(packaged.export_rel, "export/take.wav")
            self.assertEqual((root / "pkg" / "export" / "take.wav").read_bytes(), b"REAL")


class ManifestSelectionTest(unittest.TestCase):
    """A live session with a second export must still be packageable."""

    def test_multiple_manifests_select_the_newest_instead_of_aborting(self):
        with tempfile.TemporaryDirectory() as tmp:
            session = Path(tmp)
            manifests = session / "manifests"
            manifests.mkdir()
            first = manifests / "take_manifest.json"
            second = manifests / "take_v002_manifest.json"
            for path in (first, second):
                path.write_text(json.dumps({}))
            os.utime(first, (1_700_000_000, 1_700_000_000))
            os.utime(second, (1_700_000_100, 1_700_000_100))
            self.assertEqual(packager.find_manifest(session), second)
            self.assertEqual(
                packager.find_manifest(session, Path("take_manifest.json")), first.resolve()
            )


if __name__ == "__main__":
    unittest.main()
