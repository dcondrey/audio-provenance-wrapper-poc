import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from daemon.__main__ import parse_args as parse_daemon_args
from daemon.report import render_html_report, write_html_report
from daemon.sample_watcher.watcher import parse_args as parse_sample_args


class CommandLineTests(unittest.TestCase):
    def test_daemon_reads_real_process_arguments(self):
        with patch.object(sys, "argv", [
            "daemon",
            "--export-dir", "/tmp/demo-exports",
            "--source-category", "imported_sample",
            "--stem-id", "vocals",
        ]):
            args = parse_daemon_args()

        self.assertEqual(args.export_dir, Path("/tmp/demo-exports"))
        self.assertEqual(args.source_category, "imported_sample")
        self.assertEqual(args.stem_id, "vocals")

    def test_sample_watcher_reads_real_process_arguments(self):
        with patch.object(sys, "argv", [
            "sample-watcher", "--once", "--watch-dir", "/tmp/demo-samples",
        ]):
            args = parse_sample_args()

        self.assertTrue(args.once)
        self.assertEqual(args.watch_dir, Path("/tmp/demo-samples"))


class HtmlReportTests(unittest.TestCase):
    def test_report_renders_truthful_claims_and_escapes_values(self):
        manifest = {
            "session_id": "capture-<demo>",
            "created_at": "2026-08-27T00:00:00Z",
            "export": {"file_name": "mix<&>.wav", "sha256": "a" * 64},
            "observed_stems": [{
                "stem_id": "stem-1",
                "hash_chain_root": "b" * 64,
                "hash_chain_length": 12,
                "sample_rate_hz": 48000,
                "channel_count": 2,
                "apw:proof_level": "directly_observed",
            }],
            "claim_summary": [{
                "claim": "observed_stem_linked_to_export",
                "value": True,
                "evidence": "same local capture session",
                "apw:proof_level": "inferred",
            }],
            "stem_export_association": {
                "status": "inferred_match",
                "basis": "Bounded routed-feature comparison",
                "apw:proof_level": "inferred",
            },
            "apw:unobserved": ["bypassed_routing"],
        }

        report = render_html_report(manifest)

        self.assertIn("Audio Provenance Fight Card", report)
        self.assertIn("Inferred", report)
        self.assertIn("bypassed routing", report)
        self.assertNotIn("mix<&>.wav", report)
        self.assertIn("mix&lt;&amp;&gt;.wav", report)

    def test_report_writes_to_disk(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "report.html"
            write_html_report({"session_id": "test"}, path)
            self.assertTrue(path.is_file())
            self.assertIn("Evidence fight card", path.read_text())


if __name__ == "__main__":
    unittest.main()
