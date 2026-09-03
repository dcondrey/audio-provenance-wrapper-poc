"""Blind trial execution, report arithmetic, and the kill criteria the report exists to evaluate."""

from __future__ import annotations

import sys

import numpy as np
import pytest

from capture_rig.campaign import expand
from capture_rig.config import load_campaign
from capture_rig.detect.adapter import Detection, DetectorError, NullDetector, load_detector
from capture_rig.detect.runner import bit_error_rate, run_trials
from capture_rig.record import condition_record, utc_now
from capture_rig.report import build_report, evaluate_kill_criteria, render_text
from capture_rig.state import CaptureStore

from .fixtures import SAMPLE_RATE, build_campaign_tree

PAYLOAD = "109ac35e0011a7"
WRONG_PAYLOAD = "109ac35e0011a6"


class RecordingDetector:
    """Records exactly what it was handed, and answers from a fixed script."""

    def __init__(self, answers: dict[int, Detection] | None = None, default: Detection | None = None):
        self.calls: list[tuple[np.ndarray, dict]] = []
        self.answers = answers or {}
        self.default = default or Detection()

    def detect(self, audio, thresholds):
        index = len(self.calls)
        self.calls.append((audio, thresholds))
        return self.answers.get(index, self.default)


def _built(tmp_path):
    campaign = load_campaign(build_campaign_tree(tmp_path))
    store = CaptureStore(campaign.output_dir)
    tasks = [t for t in expand(campaign) if t.kind == "corpus"]
    rng = np.random.default_rng(9)
    for task in tasks:
        audio = rng.standard_normal(int(30.0 * SAMPLE_RATE / 30)) * 0.05
        record = condition_record(
            campaign,
            task,
            audio,
            SAMPLE_RATE,
            capture_mode="live",
            started_at=utc_now(),
            finished_at=utc_now(),
            devices={"host_api": "fixture", "output": None, "input": None},
        )
        store.commit(task, audio, SAMPLE_RATE, record)
    return campaign, store, tasks


def test_the_detector_receives_only_audio_and_thresholds(tmp_path):
    """The blindness contract, asserted on the call itself rather than described in a docstring."""
    campaign, store, tasks = _built(tmp_path)
    detector = RecordingDetector()
    thresholds = {"presence_threshold": 0.87, "calibration_trials": 5000}

    results, skipped = run_trials(store, tasks, detector, thresholds, {"clip000": PAYLOAD, "clip001": PAYLOAD})

    assert skipped == []
    assert len(detector.calls) == len(tasks)
    for audio, handed in detector.calls:
        assert isinstance(audio, np.ndarray) and audio.ndim == 1
        assert handed == thresholds
    assert {r.arm for r in results} == {"marked", "unmarked"}
    assert all(r.detection.payload_hex is None for r in results)


def test_scoring_happens_after_detection_and_only_marks_the_marked_arm(tmp_path):
    campaign, store, tasks = _built(tmp_path)
    detector = RecordingDetector(default=Detection(payload_hex=PAYLOAD, presence=True, confidence=0.7))
    results, _ = run_trials(store, tasks, detector, {}, {"clip000": PAYLOAD, "clip001": WRONG_PAYLOAD})

    marked = {r.clip_id: r for r in results if r.arm == "marked"}
    assert marked["clip000"].exact and marked["clip000"].bit_error_rate == 0.0
    assert not marked["clip001"].exact and marked["clip001"].bit_error_rate == pytest.approx(1 / 56)

    for result in (r for r in results if r.arm == "unmarked"):
        assert result.expected_payload_hex is None
        assert not result.exact
        assert result.detection.payload_hex == PAYLOAD  # it accepted; that is a false positive


def test_the_null_detector_yields_zero_rates_and_a_three_over_n_bound(tmp_path):
    """The pipeline's own control: the report must not manufacture detections from nothing."""
    campaign, store, tasks = _built(tmp_path)
    results, skipped = run_trials(store, tasks, NullDetector(), {}, {"clip000": PAYLOAD, "clip001": PAYLOAD})
    conditions = {t.task_id: store.load_record(t)["condition"] for t in tasks}
    report = build_report(campaign, results, conditions, "null", {}, skipped)

    assert report["schema"] == "audio-provenance-capture-rig/1"
    assert report["totals"]["overall_exact_recovery_rate"] == 0.0
    assert report["totals"]["overall_false_positive_rate"] == 0.0
    unmarked = report["totals"]["unmarked_trials"]
    assert report["totals"]["false_positive_upper_bound_95"] == pytest.approx(3.0 / unmarked)
    for row in report["rows"]:
        assert row["family"] == "measured_physical"
        assert row["simulated_physical_path"] is False
        assert row["channel"].startswith("physical_")
    assert "capture-rig" in render_text(report).splitlines()[0]


def test_a_false_accept_removes_the_three_over_n_bound_entirely(tmp_path):
    campaign, store, tasks = _built(tmp_path)
    detector = RecordingDetector(default=Detection(payload_hex=PAYLOAD))
    results, skipped = run_trials(store, tasks, detector, {}, {"clip000": PAYLOAD, "clip001": PAYLOAD})
    report = build_report(campaign, results, {}, "fixture", {}, skipped)

    assert report["totals"]["false_positive_accepts"] > 0
    assert report["totals"]["false_positive_upper_bound_95"] is None
    assert report["kill_criteria"]["K4"]["verdict"] in ("kill", "unevaluated")


def _row(channel: str, room: str, speaker: str, mic: str, distance: float, exact: int, trials: int,
         fp_accepts: int = 0, fp_trials: int = 200, presence: int | None = None) -> dict:
    return {
        "channel": channel,
        "family": "measured_physical",
        "params": {"room": room, "speaker": speaker, "microphone": mic, "distance_m": distance},
        "simulated_physical_path": False,
        "trials": trials,
        "exact_recoveries": exact,
        "payloads_returned": exact,
        "presence_detections": trials if presence is None else presence,
        "exact_recovery_rate": exact / trials if trials else None,
        "presence_rate": (trials if presence is None else presence) / trials if trials else None,
        "mean_capture_seconds": 30.0,
        "false_positive": {"trials": fp_trials, "accepts": fp_accepts},
        "presence_false_positive": {"trials": fp_trials, "accepts": 0},
    }


def _k4_grid(exact_per_row: int, trials_per_row: int = 100, fp_accepts: int = 0) -> list[dict]:
    rows = []
    for room in ("r1", "r2", "r3"):
        for speaker in ("s1", "s2"):
            for mic in ("m1", "m2"):
                rows.append(
                    _row(f"physical_{room}_{speaker}_{mic}_1m", room, speaker, mic, 1.0,
                         exact_per_row, trials_per_row, fp_accepts, 100)
                )
    return rows


def test_k4_passes_only_with_full_coverage_a_rate_over_half_and_no_false_accept():
    passing = evaluate_kill_criteria(_k4_grid(60))["K4"]
    assert passing["verdict"] == "pass"
    assert passing["coverage_gaps"] == []
    assert passing["exact_recovery_rate"] == pytest.approx(0.60)

    failing = evaluate_kill_criteria(_k4_grid(40))["K4"]
    assert failing["verdict"] == "kill"
    assert "below 0.5" in failing["reasons"][0]

    accepted = evaluate_kill_criteria(_k4_grid(60, fp_accepts=1))["K4"]
    assert accepted["verdict"] == "kill"
    assert any("false accept" in reason for reason in accepted["reasons"])


def test_k4_reports_a_coverage_gap_rather_than_a_verdict_it_cannot_support():
    thin = [r for r in _k4_grid(18, trials_per_row=20) if r["params"]["room"] == "r1"]
    criterion = evaluate_kill_criteria(thin)["K4"]
    assert criterion["verdict"] == "unevaluated"
    assert any("rooms" in gap for gap in criterion["coverage_gaps"])
    assert any("marked trials" in gap for gap in criterion["coverage_gaps"])


def test_k6_reads_the_largest_distance_that_meets_the_k4_threshold():
    rows = [
        _row("physical_r1_s1_m1_0p15m", "r1", "s1", "m1", 0.15, 225, 250),
        _row("physical_r1_s1_m1_0p5m", "r1", "s1", "m1", 0.5, 175, 250),
        _row("physical_r1_s1_m1_1m", "r1", "s1", "m1", 1.0, 50, 250),
    ]
    assert evaluate_kill_criteria(rows)["K6"]["largest_passing_distance_m"] == 0.5
    assert evaluate_kill_criteria(rows)["K6"]["verdict"] == "pass"

    close_only = [_row("physical_r1_s1_m1_0p15m", "r1", "s1", "m1", 0.15, 225, 250)]
    assert evaluate_kill_criteria(close_only)["K6"]["verdict"] == "kill"

    thin = [_row("physical_r1_s1_m1_1m", "r1", "s1", "m1", 1.0, 40, 50)]
    assert evaluate_kill_criteria(thin)["K6"]["verdict"] == "unevaluated"


def test_k5b_fails_on_any_presence_firing_over_unmarked_audio():
    rows = _k4_grid(90)
    for row in rows:
        row["presence_false_positive"]["accepts"] = 1
    criterion = evaluate_kill_criteria(rows)["K5b"]
    assert criterion["verdict"] == "kill"
    assert "CONFIRMS" in criterion["note"]


def test_bit_error_rate_refuses_payloads_of_different_widths():
    assert bit_error_rate("00", "ff") == 1.0
    assert bit_error_rate("0000", "ff") is None


def test_subprocess_detector_hands_over_a_filename_carrying_no_condition(tmp_path):
    """An exec detector must not be able to read the arm or the clip out of the path it is given."""
    script = tmp_path / "echo_detector.py"
    script.write_text(
        "import argparse, json, os, sys\n"
        "p = argparse.ArgumentParser(); p.add_argument('--audio'); p.add_argument('--thresholds')\n"
        "a = p.parse_args()\n"
        "t = json.load(open(a.thresholds))\n"
        "print(json.dumps({'presence': True, 'presence_score': t['presence_threshold'],\n"
        "                  'findings': [os.path.basename(a.audio)]}))\n",
        encoding="utf-8",
    )
    detector = load_detector(f"exec:{sys.executable} {script}", SAMPLE_RATE)
    detection = detector.detect(np.zeros(SAMPLE_RATE, dtype=np.float64), {"presence_threshold": 0.9})
    assert detection.presence is True
    assert detection.presence_score == pytest.approx(0.9)
    assert detection.findings == ("trial.wav",)


def test_a_detector_returning_a_non_hex_payload_is_refused():
    with pytest.raises(DetectorError, match="not hexadecimal"):
        Detection.from_dict({"payload_hex": "zzzz"})
    with pytest.raises(DetectorError, match="unrecognised detector spec"):
        load_detector("magic:thing", SAMPLE_RATE)
