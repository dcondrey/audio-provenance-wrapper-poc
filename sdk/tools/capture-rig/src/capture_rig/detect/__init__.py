"""The BLIND detection path.

A detector here receives the raw capture and a threshold record frozen before the run, and nothing
else: no source clip, no payload, no alignment, no arm label, no condition. That is the whole point
of the campaign. Spec 4.1 names the oracle best-of-search as the reason no published physical number
means what it appears to mean, and a harness that reproduced it would produce the same worthless
number.

`capture_rig.alignment` is deliberately absent from this package's import graph and
`tests/test_oracle_firewall.py` fails the build if it ever appears.
"""

from .adapter import Detection, Detector, DetectorError, load_detector
from .runner import TrialResult, run_trials

__all__ = ["Detection", "Detector", "DetectorError", "load_detector", "TrialResult", "run_trials"]
