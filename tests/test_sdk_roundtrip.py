from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from typing import Any

from scripts.synthetic_rehearsal import run


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(64 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _proof_levels(value: Any) -> list[str]:
    levels: list[str] = []
    if isinstance(value, dict):
        level = value.get("apw:proof_level")
        if isinstance(level, str):
            levels.append(level)
        for child in value.values():
            levels.extend(_proof_levels(child))
    elif isinstance(value, list):
        for child in value:
            levels.extend(_proof_levels(child))
    return levels


class SyntheticPluginSdkRoundTripTests(unittest.TestCase):
    def test_real_observations_reach_the_public_sdk_without_proof_promotion(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            manifest_path = run(Path(temporary), sdk_adapter=True)
            session = manifest_path.parent.parent
            artifacts = manifest_path.parent / "artifacts"
            handoff_path = artifacts / "presenter_export_handoff.json"
            bundle_path = artifacts / "presenter_export_evidence_bundle.zip"
            receipt_path = artifacts / "presenter_export_sdk_receipt.json"
            record_path = artifacts / "presenter_export_sdk_record.json"

            capture = json.loads(manifest_path.read_text(encoding="utf-8"))
            handoff = json.loads(handoff_path.read_text(encoding="utf-8"))
            receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
            record = json.loads(record_path.read_text(encoding="utf-8"))

            self.assertEqual(capture["observation_coverage"]["status"], "complete_observed_path")
            self.assertEqual(capture["stem_export_association"]["status"], "inferred_match")
            self.assertEqual(receipt["identity"], "not_established")
            self.assertTrue(receipt["development_only"])
            self.assertEqual(receipt["capture_manifest_sha256"], _sha256(manifest_path))
            self.assertEqual(receipt["capture_handoff_sha256"], _sha256(handoff_path))
            self.assertEqual(receipt["evidence_bundle_sha256"], _sha256(bundle_path))

            copied = [
                claim
                for claim in record["claims"]
                if claim["claim"].startswith("capture_handoff:")
            ]
            self.assertEqual(receipt["proof_objects_preserved"], len(copied))
            self.assertEqual(
                sorted(_proof_levels(handoff)),
                sorted(claim["apw:proof_level"] for claim in copied),
            )
            digest_claims = {
                claim["claim"]: claim["value"]
                for claim in record["claims"]
                if claim["claim"].endswith("_sha256")
            }
            self.assertEqual(
                digest_claims["capture_manifest_sha256"],
                receipt["capture_manifest_sha256"],
            )
            self.assertEqual(
                digest_claims["capture_handoff_sha256"],
                receipt["capture_handoff_sha256"],
            )
            self.assertEqual(
                digest_claims["evidence_bundle_sha256"],
                receipt["evidence_bundle_sha256"],
            )

            verification = receipt["verification"]
            self.assertEqual(verification["status"], "untrusted")
            self.assertEqual(verification["reason"], "trust_anchor_unresolved")
            self.assertTrue(verification["signature"]["valid"])
            self.assertEqual(verification["match_basis"], "hard_exact")
            self.assertEqual(verification["recording_association"]["status"], "exact")
            self.assertFalse(verification["incomplete"])
            self.assertTrue((session / "sdk-development.key").is_file())
            self.assertTrue(record_path.is_file())


if __name__ == "__main__":
    unittest.main()
