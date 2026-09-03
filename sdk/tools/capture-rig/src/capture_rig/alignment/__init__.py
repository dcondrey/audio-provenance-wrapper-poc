"""Alignment and segmentation: WHICH REGION of a capture corresponds to WHICH source clip.

THIS IS DATASET BOOKKEEPING AND IT MUST NEVER TOUCH DETECTION.

Every published physical watermark result obtains its alignment by sweeping offsets and keeping the
one that scores best against the KNOWN payload (spec section 4.1). That is an oracle: a deployed
detector has no ground truth to score against. This package uses the source clip, which is ground
truth, so anything downstream of it is oracle-contaminated by construction.

The rule, enforced rather than described: `capture_rig.detect` and `capture_rig.report` do not
import this package, transitively or otherwise, and
`tests/test_oracle_firewall.py::test_detection_path_never_imports_alignment` fails the build if that
ever changes. A doc paragraph can be broken by an edit that looks innocent; an import-graph
assertion cannot.
"""

from .locate import Segment, gcc_phat, locate_clip, segment_capture

__all__ = ["Segment", "gcc_phat", "locate_clip", "segment_capture"]
