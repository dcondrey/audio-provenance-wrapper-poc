from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from daemon.sdk_adapter import (
    CaptureAdapterInvocation,
    ensure_development_key,
    run_capture_adapter,
)


class CaptureSdkAdapterTests(unittest.TestCase):
    def test_development_key_is_bounded_and_reused(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            key = Path(temporary) / "development.key"
            ensure_development_key(key)
            first = key.read_bytes()
            self.assertEqual(len(first), 32)
            ensure_development_key(key)
            self.assertEqual(key.read_bytes(), first)

    @mock.patch("daemon.sdk_adapter.subprocess.run")
    def test_python_delegates_to_the_same_rust_cli_command(self, run: mock.Mock) -> None:
        result = {
            "development_only": True,
            "identity": "not_established",
            "sign": {"record_id": "ab" * 32},
            "verification": {"status": "untrusted"},
        }
        run.return_value = subprocess.CompletedProcess(
            args=[], returncode=0, stdout=json.dumps(result).encode(), stderr=b""
        )
        invocation = CaptureAdapterInvocation(
            export=Path("export.wav"),
            capture_manifest=Path("capture.json"),
            handoff=Path("handoff.json"),
            evidence_bundle=Path("bundle.zip"),
            key=Path("development.key"),
            receipt=Path("receipt.json"),
            sidecar=Path("record.json"),
            evidence_bundle_sha256="12" * 32,
            registry="local",
        )

        self.assertEqual(run_capture_adapter(invocation, cli="audio-provenance"), result)
        command = run.call_args.args[0]
        self.assertEqual(command[:4], ["audio-provenance", "--json", "--registry", "local"])
        self.assertIn("capture-adapt", command)
        self.assertIn("--evidence-bundle-sha256", command)
        self.assertIn("--sidecar", command)

    def test_output_modes_are_mutually_exclusive(self) -> None:
        with self.assertRaises(ValueError):
            CaptureAdapterInvocation(
                export=Path("export.wav"),
                capture_manifest=Path("capture.json"),
                handoff=Path("handoff.json"),
                evidence_bundle=Path("bundle.zip"),
                key=Path("development.key"),
                receipt=Path("receipt.json"),
                embedded_output=Path("embedded.wav"),
                sidecar=Path("record.json"),
            )


if __name__ == "__main__":
    unittest.main()
