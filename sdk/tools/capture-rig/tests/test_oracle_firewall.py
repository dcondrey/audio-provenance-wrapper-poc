"""The firewall between alignment (bookkeeping, uses ground truth) and detection (must be blind).

This is the single most important test in the tool. Spec 4.1 names the oracle best-of-search as the
reason no published physical watermark number means what it appears to mean: every one of them
aligns by sweeping offsets and keeping whichever scores best AGAINST THE KNOWN PAYLOAD. This rig
exists to produce a number that is not that, and the only durable way to say so is a test an
innocent-looking import would fail.
"""

from __future__ import annotations

import subprocess
import sys

PROBE = """
import sys
import capture_rig.detect
import capture_rig.detect.runner
import capture_rig.report
import capture_rig.cli
leaked = sorted(m for m in sys.modules if m.startswith("capture_rig.alignment"))
print(",".join(leaked))
"""


def _probe(source: str) -> list[str]:
    completed = subprocess.run(
        [sys.executable, "-c", source], capture_output=True, text=True, check=True, timeout=120
    )
    return [name for name in completed.stdout.strip().split(",") if name]


def test_detection_path_never_imports_alignment():
    assert _probe(PROBE) == []


def test_the_probe_would_notice_if_alignment_were_imported():
    """Guards the guard: a firewall test that cannot fail is not a firewall."""
    leaked = _probe("import capture_rig.alignment\n" + PROBE)
    assert leaked and leaked[0].startswith("capture_rig.alignment")
