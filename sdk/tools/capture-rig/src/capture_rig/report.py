"""The campaign report: a BLIND detection rate with its false-positive bound, per condition.

Row shape deliberately mirrors `crates/audio-provenance-bench`'s so the numbers sit beside its simulated
acoustic rows without translation: `channel`, `family`, `params`, `simulated_physical_path`,
`exact_recovery_rate`, `payload_returned_rate`, `mean_bit_error_rate`, `mean_detect_seconds`,
`false_positive{trials,accepts,rate,errors}` and per-trial rows.

The schema id is NOT the bench's. A different producer emitting `audio-provenance-bench/1` would be a
provenance lie; spec 10.4 says the physical arm is a proposal to the bench's owner, not an edit made
by this tool.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Iterable

import numpy as np

from . import __version__
from .config import Campaign
from .detect.runner import TrialResult
from .record import utc_now

SCHEMA = "audio-provenance-capture-rig/1"
CHANNEL_FAMILY = "measured_physical"
K4_MIN_EXACT_RECOVERY = 0.50
K4_DISTANCE_M = 1.0
K4_MIN_ROOMS = 3
K4_MIN_SPEAKERS = 2
K4_MIN_MICROPHONES = 2
K4_MIN_CLIPS = 200
K4_MIN_DURATION_SECONDS = 30.0
K5B_MIN_PRESENCE_RATE = 0.80
K6_MIN_DISTANCE_M = 0.5
DISTANCE_TOLERANCE_M = 0.05

BLINDNESS_NOTE = (
    "Every detection in this report was produced by a detector that received the raw capture and a "
    "threshold record frozen before the run, and no ground truth. No offset was searched, no window "
    "was selected by its agreement with a known payload, no threshold was fitted on these trials, "
    "and the alignment/segmentation code in capture_rig.alignment is not in this path's import "
    "graph. The published acoustic literature reports best-of-search maxima computed with knowledge "
    "of the true bits; those numbers and these are not the same quantity."
)

FALSE_POSITIVE_NOTE = (
    "The false-positive arm is the UNMARKED physical corpus captured under identical conditions. "
    "Zero accepts over N trials bounds the rate at roughly 3/N with 95% confidence; it does not "
    "establish that the rate is zero. A physical campaign yields hundreds of unmarked captures, not "
    "thousands, so a 1e-3 bound is not physically measurable at this scale: spec 12.2 requires it to "
    "be CALIBRATED on >= 5000 simulated unmarked trials and only CONFIRMED here."
)

DISCLAIMERS = (
    "MEASURED speaker-to-microphone path. Audio was played through a named physical loudspeaker and "
    "captured by a named physical microphone in a named room at a measured distance. This is the "
    "quantity that crates/audio-provenance-bench's acoustic rows explicitly are NOT.",
    "RT60 and DRR in each row's params are MEASURED from a swept-sine impulse response taken in that "
    "cell, not modelled. RT60 is Schroeder T30 extrapolated to 60 dB; DRR is the ACE +/- 2.5 ms "
    "direct-window convention, which is not the image-order split the training tree's synthetic RIRs "
    "use, and the two are not interchangeable.",
    "An SPL or background dBA field reported as null means no operator sound-level-meter reading "
    "anchors that room's captures. It is not an omission to be filled in later from the dBFS.",
    "Every published physical watermark number this can be compared against is an oracle "
    "best-of-search maximum computed with knowledge of the true bits. These are not.",
)

METRIC_DEFINITIONS = {
    "exact_recovery_rate": "exact payload matches divided by marked trials that ran without error. The pass/fail metric.",
    "payload_returned_rate": "marked trials where the detector returned any payload, divided by marked trials that ran without error.",
    "presence_rate": "marked trials where the zero-bit presence tier fired, divided by marked trials that ran without error. This is what kill criterion K5b reads.",
    "mean_bit_error_rate": "mean differing-bit fraction over marked trials where the detector RETURNED a payload. Trials where it declined contribute no bits and are excluded, so a low value alongside a low payload_returned_rate means the detector is silent rather than accurate. Read it next to payload_returned_rate, never alone.",
    "false_positive.rate": "locator accepts divided by UNMARKED trials that ran without error, on the SAME physical channel.",
    "presence_false_positive.rate": "presence-tier firings divided by unmarked trials that ran without error, on the SAME physical channel. The presence tier has no CRC behind it, so this is a purely empirical quantity (spec 3.5).",
    "false_positive_upper_bound_95": "3/N with N the unmarked trial count. Quoted only where accepts is zero; where it is not, the observed rate is the measurement.",
    "mean_detect_seconds": "wall time inside detect() only.",
}


def _rate(numerator: int, denominator: int) -> float | None:
    return numerator / denominator if denominator else None


def _upper_bound(accepts: int, trials: int) -> float | None:
    if trials <= 0 or accepts > 0:
        return None
    return 3.0 / trials


@dataclass(frozen=True)
class ChannelRow:
    channel: str
    params: dict
    marked: list[TrialResult]
    unmarked: list[TrialResult]

    def to_dict(self, include_trials: bool) -> dict:
        ok_marked = [t for t in self.marked if t.error is None]
        ok_unmarked = [t for t in self.unmarked if t.error is None]
        exact = sum(1 for t in ok_marked if t.exact)
        returned = sum(1 for t in ok_marked if t.detection.payload_hex is not None)
        presence = sum(1 for t in ok_marked if t.detection.presence)
        bers = [t.bit_error_rate for t in ok_marked if t.bit_error_rate is not None]
        times = [t.detect_seconds for t in self.marked + self.unmarked if t.error is None]
        fp_accepts = sum(1 for t in ok_unmarked if t.detection.payload_hex is not None)
        presence_fp = sum(1 for t in ok_unmarked if t.detection.presence)
        row = {
            "channel": self.channel,
            "family": CHANNEL_FAMILY,
            "params": self.params,
            "simulated_physical_path": False,
            "trials": len(ok_marked),
            "trial_errors": len(self.marked) - len(ok_marked),
            "exact_recoveries": exact,
            "payloads_returned": returned,
            "presence_detections": presence,
            "exact_recovery_rate": _rate(exact, len(ok_marked)),
            "payload_returned_rate": _rate(returned, len(ok_marked)),
            "presence_rate": _rate(presence, len(ok_marked)),
            "mean_bit_error_rate": float(np.mean(bers)) if bers else None,
            "mean_detect_seconds": float(np.mean(times)) if times else None,
            "p95_detect_seconds": float(np.percentile(times, 95)) if times else None,
            "mean_capture_seconds": (
                float(np.mean([t.duration_seconds for t in ok_marked])) if ok_marked else None
            ),
            "false_positive": {
                "trials": len(ok_unmarked),
                "accepts": fp_accepts,
                "rate": _rate(fp_accepts, len(ok_unmarked)),
                "errors": len(self.unmarked) - len(ok_unmarked),
                "upper_bound_95": _upper_bound(fp_accepts, len(ok_unmarked)),
            },
            "presence_false_positive": {
                "trials": len(ok_unmarked),
                "accepts": presence_fp,
                "rate": _rate(presence_fp, len(ok_unmarked)),
                "upper_bound_95": _upper_bound(presence_fp, len(ok_unmarked)),
            },
        }
        if include_trials:
            row["trial_rows"] = [t.to_dict() for t in self.marked + self.unmarked]
        return row


def _params_for(trials: list[TrialResult], conditions: dict[str, dict]) -> dict:
    sample = trials[0]
    condition = conditions.get(sample.task_id, {})
    return {
        "room": sample.room_id,
        "speaker": sample.speaker_id,
        "microphone": sample.microphone_id,
        "distance_m": sample.distance_m,
        "spl_target_dba": condition.get("spl_target_dba"),
        "spl_dba": condition.get("spl_dba"),
        "measured_rt60_seconds": condition.get("measured_rt60_seconds"),
        "measured_drr_db": condition.get("measured_drr_db"),
        "background_dba": condition.get("background_dba"),
        "capture_mode": condition.get("capture_mode"),
    }


def group_rows(results: Iterable[TrialResult], conditions: dict[str, dict]) -> list[ChannelRow]:
    grouped: dict[str, dict[str, list[TrialResult]]] = {}
    for result in results:
        if result.channel is None:
            continue
        arms = grouped.setdefault(result.channel, {"marked": [], "unmarked": []})
        arms.setdefault(result.arm, []).append(result)
    rows: list[ChannelRow] = []
    for channel in sorted(grouped):
        arms = grouped[channel]
        present = arms["marked"] or arms["unmarked"]
        rows.append(
            ChannelRow(
                channel=channel,
                params=_params_for(present, conditions) if present else {},
                marked=arms["marked"],
                unmarked=arms["unmarked"],
            )
        )
    return rows


def _at_distance(rows: list[dict], distance_m: float) -> list[dict]:
    return [
        r for r in rows
        if r["params"].get("distance_m") is not None
        and abs(r["params"]["distance_m"] - distance_m) <= DISTANCE_TOLERANCE_M
    ]


def _pool(rows: list[dict], key: str) -> tuple[int, int]:
    numerator = sum(r[key] for r in rows)
    denominator = sum(r["trials"] for r in rows)
    return numerator, denominator


def evaluate_kill_criteria(rows: list[dict]) -> dict:
    """K4, K5b and K6 from spec 12.3, computed on the rows above and nothing else."""
    one_metre = _at_distance(rows, K4_DISTANCE_M)
    rooms = {r["params"]["room"] for r in one_metre}
    speakers = {r["params"]["speaker"] for r in one_metre}
    mics = {r["params"]["microphone"] for r in one_metre}
    exact, trials = _pool(one_metre, "exact_recoveries")
    fp_accepts = sum(r["false_positive"]["accepts"] for r in one_metre)
    fp_trials = sum(r["false_positive"]["trials"] for r in one_metre)
    durations = [r["mean_capture_seconds"] for r in one_metre if r["mean_capture_seconds"] is not None]
    shortest = min(durations) if durations else None

    coverage: list[str] = []
    if len(rooms) < K4_MIN_ROOMS:
        coverage.append(f"K4 needs >= {K4_MIN_ROOMS} rooms at 1.0 m, has {len(rooms)}")
    if len(speakers) < K4_MIN_SPEAKERS:
        coverage.append(f"K4 needs >= {K4_MIN_SPEAKERS} speakers at 1.0 m, has {len(speakers)}")
    if len(mics) < K4_MIN_MICROPHONES:
        coverage.append(f"K4 needs >= {K4_MIN_MICROPHONES} microphones at 1.0 m, has {len(mics)}")
    if trials < K4_MIN_CLIPS:
        coverage.append(f"K4 needs >= {K4_MIN_CLIPS} marked trials at 1.0 m, has {trials}")
    if shortest is not None and shortest < K4_MIN_DURATION_SECONDS:
        coverage.append(
            f"K4 is defined at {K4_MIN_DURATION_SECONDS:g} s captures; the shortest 1.0 m row averages {shortest:.1f} s"
        )
    if fp_trials == 0:
        coverage.append("K4 requires an unmarked arm at 1.0 m; none was captured")

    rate = _rate(exact, trials)
    k4_reasons: list[str] = []
    if rate is not None and rate < K4_MIN_EXACT_RECOVERY:
        k4_reasons.append(f"pooled exact_recovery_rate {rate:.3f} at 1.0 m is below {K4_MIN_EXACT_RECOVERY}")
    if fp_accepts > 0:
        k4_reasons.append(f"{fp_accepts} false accept(s) on the unmarked physical corpus; K4 requires zero")

    k4 = {
        "criterion": "K4",
        "tier": "locator",
        "evaluated": not coverage and rate is not None,
        "coverage_gaps": coverage,
        "rooms": sorted(rooms),
        "speakers": sorted(speakers),
        "microphones": sorted(mics),
        "marked_trials": trials,
        "exact_recovery_rate": rate,
        "false_positive": {
            "trials": fp_trials,
            "accepts": fp_accepts,
            "rate": _rate(fp_accepts, fp_trials),
            "upper_bound_95": _upper_bound(fp_accepts, fp_trials),
        },
        "threshold": K4_MIN_EXACT_RECOVERY,
        "verdict": (
            "unevaluated" if (coverage or rate is None) else ("kill" if k4_reasons else "pass")
        ),
        "reasons": k4_reasons,
    }

    presence, presence_trials = _pool(one_metre, "presence_detections")
    presence_fp = sum(r["presence_false_positive"]["accepts"] for r in one_metre)
    presence_rate = _rate(presence, presence_trials)
    k5b_reasons: list[str] = []
    if presence_rate is not None and presence_rate < K5B_MIN_PRESENCE_RATE:
        k5b_reasons.append(f"presence rate {presence_rate:.3f} at 1.0 m is below {K5B_MIN_PRESENCE_RATE}")
    if presence_fp > 0:
        k5b_reasons.append(f"{presence_fp} presence firing(s) on the unmarked physical corpus; K5b requires zero")
    k5b_evaluable = not coverage and presence_rate is not None
    k5b = {
        "criterion": "K5b",
        "tier": "presence",
        "evaluated": k5b_evaluable,
        "coverage_gaps": coverage,
        "presence_rate": presence_rate,
        "marked_trials": presence_trials,
        "presence_false_positive": {
            "trials": fp_trials,
            "accepts": presence_fp,
            "rate": _rate(presence_fp, fp_trials),
            "upper_bound_95": _upper_bound(presence_fp, fp_trials),
        },
        "threshold": K5B_MIN_PRESENCE_RATE,
        "verdict": "unevaluated" if not k5b_evaluable else ("kill" if k5b_reasons else "pass"),
        "reasons": k5b_reasons,
        "note": (
            "K5b CONFIRMS a threshold calibrated elsewhere; it cannot establish a false-positive rate. "
            "Spec 12.2: the 1e-3 figure comes from >= 5000 simulated unmarked trials (K5a), and this "
            "arm bounds the physical rate only at 3/N with N stated above."
        ),
    }

    # Pooled per distance, because K6 asks about a distance and not about one room-speaker-mic cell.
    by_distance: dict[float, tuple[int, int]] = {}
    for row in rows:
        distance = row["params"].get("distance_m")
        if distance is None:
            continue
        exact_at, trials_at = by_distance.get(distance, (0, 0))
        by_distance[distance] = (exact_at + row["exact_recoveries"], trials_at + row["trials"])
    passing_distances = [
        distance
        for distance, (exact_at, trials_at) in by_distance.items()
        if trials_at >= K4_MIN_CLIPS and exact_at / trials_at >= K4_MIN_EXACT_RECOVERY
    ]
    largest = max(passing_distances) if passing_distances else None
    # A kill verdict stops the programme, so it is only reachable once some distance carries enough
    # trials to support one. A thin dataset is unevaluated, never a kill.
    k6_evaluable = any(trials_at >= K4_MIN_CLIPS for _, trials_at in by_distance.values())
    k6 = {
        "criterion": "K6",
        "largest_passing_distance_m": largest,
        "threshold_m": K6_MIN_DISTANCE_M,
        "distances_measured": {str(d): {"exact": e, "trials": t} for d, (e, t) in sorted(by_distance.items())},
        "minimum_trials_per_distance": K4_MIN_CLIPS,
        "evaluated": k6_evaluable,
        "verdict": (
            "unevaluated"
            if not k6_evaluable
            else ("pass" if largest is not None and largest >= K6_MIN_DISTANCE_M else "kill")
        ),
        "reasons": (
            []
            if largest is not None and largest >= K6_MIN_DISTANCE_M
            else [
                f"no distance reaches exact_recovery_rate {K4_MIN_EXACT_RECOVERY}"
                if largest is None
                else f"largest passing distance {largest} m is below {K6_MIN_DISTANCE_M} m"
            ]
        ),
    }
    return {"K4": k4, "K5b": k5b, "K6": k6}


def build_report(
    campaign: Campaign,
    results: list[TrialResult],
    conditions: dict[str, dict],
    detector_spec: str,
    thresholds: dict,
    skipped: list[str],
    include_trials: bool = True,
) -> dict:
    rows = [row.to_dict(include_trials) for row in group_rows(results, conditions)]
    ok = [r for r in results if r.error is None]
    marked = [r for r in ok if r.arm == "marked"]
    unmarked = [r for r in ok if r.arm == "unmarked"]
    fp_accepts = sum(1 for r in unmarked if r.detection.payload_hex is not None)
    exact = sum(1 for r in marked if r.exact)
    return {
        "schema": SCHEMA,
        "generated_at": utc_now(),
        "capture_rig_version": __version__,
        "campaign_id": campaign.campaign_id,
        "working_sample_rate": campaign.sample_rate,
        "detector": {"spec": detector_spec},
        "thresholds": thresholds,
        "rows": rows,
        "totals": {
            "rows_recorded": len(rows),
            "marked_trials": len(marked),
            "unmarked_trials": len(unmarked),
            "trial_errors": sum(1 for r in results if r.error is not None),
            "exact_recoveries": exact,
            "overall_exact_recovery_rate": _rate(exact, len(marked)),
            "false_positive_trials": len(unmarked),
            "false_positive_accepts": fp_accepts,
            "overall_false_positive_rate": _rate(fp_accepts, len(unmarked)),
            "false_positive_upper_bound_95": _upper_bound(fp_accepts, len(unmarked)),
            "captures_missing": len(skipped),
        },
        "kill_criteria": evaluate_kill_criteria(rows),
        "blindness": {
            "note": BLINDNESS_NOTE,
            "detector_inputs": ["raw capture audio", "frozen threshold record"],
            "detector_inputs_withheld": [
                "the true payload",
                "the source clip",
                "the marked/unmarked arm",
                "the room, speaker, microphone, distance or SPL",
                "any alignment or segmentation produced by capture_rig.alignment",
            ],
        },
        "false_positive_note": FALSE_POSITIVE_NOTE,
        "metric_definitions": METRIC_DEFINITIONS,
        "disclaimers": list(DISCLAIMERS),
        "skipped_captures": skipped,
    }


def render_text(report: dict) -> str:
    def rate(value: float | None, width: int = 7) -> str:
        return f"{'n/a':>{width}}" if value is None else f"{value:>{width}.3f}"

    totals = report["totals"]
    bound = totals["false_positive_upper_bound_95"]
    lines = [
        f"{report['schema']}  campaign={report['campaign_id']}  detector={report['detector']['spec']}",
        f"generated {report['generated_at']}",
        "",
        f"{'channel':<58} {'trials':>7} {'exact':>7} {'return':>7} {'presence':>9} {'fp':>6} {'fp_n':>6}",
    ]
    for row in report["rows"]:
        lines.append(
            f"{row['channel']:<58} {row['trials']:>7} {rate(row['exact_recovery_rate'])} "
            f"{rate(row['payload_returned_rate'])} {rate(row['presence_rate'], 9)} "
            f"{row['false_positive']['accepts']:>6} {row['false_positive']['trials']:>6}"
        )
    lines += [
        "",
        f"marked trials {totals['marked_trials']}  exact {totals['exact_recoveries']} "
        f"(rate {rate(totals['overall_exact_recovery_rate']).strip()})",
        f"unmarked trials {totals['unmarked_trials']}  accepts {totals['false_positive_accepts']}  "
        + (
            f"95% upper bound {bound:.5f} (3/N)"
            if bound is not None
            else "95% upper bound n/a: accepts is non-zero, so the observed rate above IS the "
                 "measurement and no zero-accept bound applies"
        ),
        "",
    ]
    for name, criterion in report["kill_criteria"].items():
        lines.append(f"{name}: {criterion['verdict']}")
        for reason in list(criterion.get("reasons", ())) + list(criterion.get("coverage_gaps", ())):
            lines.append(f"    - {reason}")
    lines += ["", report["blindness"]["note"], "", report["false_positive_note"]]
    return "\n".join(lines) + "\n"
