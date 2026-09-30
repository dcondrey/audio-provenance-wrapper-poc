import gzip
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

from tests.support import load_script

REPO = Path(__file__).resolve().parent.parent
QUICKSTART_PDF = REPO / "packaging" / "assets" / "quickstart.pdf"
QUICKSTART_GENERATOR = REPO / "scripts" / "make_quickstart_pdf.py"
DEMO_SET = REPO / "packaging" / "assets" / "demo-project" / "apw-demo Project" / "apw-demo.als"


def _load_generator():
    return load_script("_make_quickstart_pdf", QUICKSTART_GENERATOR)


class QuickstartPdfTest(unittest.TestCase):
    def test_shipped_pdf_is_the_current_generator_output(self):
        try:
            generator = _load_generator()
        except ImportError as exc:
            self.skipTest(f"quickstart generator dependency missing: {exc}")
        with tempfile.TemporaryDirectory() as tmp:
            fresh = Path(tmp) / "quickstart.pdf"
            generator.render(fresh)
            self.assertEqual(
                fresh.read_bytes(),
                QUICKSTART_PDF.read_bytes(),
                "packaging/assets/quickstart.pdf is stale; re-run scripts/make_quickstart_pdf.py",
            )


class DemoAbletonSetTest(unittest.TestCase):
    def setUp(self):
        self.xml = gzip.decompress(DEMO_SET.read_bytes()).decode("utf-8")

    def test_set_parses(self):
        ET.fromstring(self.xml)

    def test_no_capture_device_is_saved_in_the_set(self):
        for marker in ("Audio Provenance Capture", "Vst3PluginInfo"):
            self.assertNotIn(
                marker,
                self.xml,
                "the demo set must ship with no capture device: one saved into the set is "
                "instantiated before any daemon session exists and grades coverage "
                "partial_observed_path",
            )

    def test_no_home_directory_is_disclosed(self):
        self.assertNotIn("/Users/", self.xml)
