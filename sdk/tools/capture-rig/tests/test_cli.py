"""The command line, over the paths that run without audio hardware."""

from __future__ import annotations

import json

import numpy as np
import pytest
from scipy.signal import fftconvolve

from capture_rig.audio import read_wav, write_wav_atomic
from capture_rig.campaign import expand
from capture_rig.cli import EXIT_ERROR, EXIT_KILL, EXIT_OK, main
from capture_rig.config import load_campaign
from capture_rig.detect.runner import PAYLOAD_MAP_SCHEMA
from capture_rig.record import condition_record, utc_now
from capture_rig.rooms import analyse_sweep_capture, write_impulse_response
from capture_rig.signals import exponential_sweep, sweep_stimulus
from capture_rig.state import CaptureStore

from .fixtures import SAMPLE_RATE, build_campaign_tree
from .synthetic import decaying_ir

PAYLOAD = "109ac35e0011a7"


def _campaign(tmp_path):
    path = build_campaign_tree(tmp_path)
    campaign = load_campaign(path)
    return path, campaign, CaptureStore(campaign.output_dir)


def _fill_corpus(campaign, store):
    rng = np.random.default_rng(3)
    tasks = [t for t in expand(campaign) if t.kind == "corpus"]
    for task in tasks:
        audio = rng.standard_normal(SAMPLE_RATE) * 0.05
        record = condition_record(
            campaign, task, audio, SAMPLE_RATE,
            capture_mode="live", started_at=utc_now(), finished_at=utc_now(),
            devices={"host_api": "fixture", "output": None, "input": None},
        )
        store.commit(task, audio, SAMPLE_RATE, record)
    return tasks


def _fill_sweeps(campaign, store, rt60_by_room):
    pair = exponential_sweep(SAMPLE_RATE, 10.0, 20.0, 20000.0)
    for task in expand(campaign):
        if task.kind != "sweep":
            continue
        ir = decaying_ir(SAMPLE_RATE, rt60_by_room[task.room_id], tail_amplitude=0.12, length_seconds=2.0)
        recording = fftconvolve(sweep_stimulus(pair, 2.5, -6.0), ir)
        record = condition_record(
            campaign, task, recording, SAMPLE_RATE,
            capture_mode="live", started_at=utc_now(), finished_at=utc_now(),
            devices={"host_api": "fixture", "output": None, "input": None},
            extra={"stimulus": pair.to_dict()},
        )
        store.commit(task, recording, SAMPLE_RATE, record)
        write_impulse_response(
            store.root, task,
            analyse_sweep_capture(recording, pair, task, ir_seconds=1.5, pre_seconds=0.01),
        )


def test_plan_reports_the_matrix_and_its_cost(tmp_path, capsys):
    path, campaign, store = _campaign(tmp_path)
    assert main(["plan", "--campaign", str(path), "--json"]) == EXIT_OK
    payload = json.loads(capsys.readouterr().out)
    assert payload["tasks"] == len(expand(campaign))
    assert payload["status_counts"]["missing"] == payload["tasks"]
    assert payload["time_estimate"]["total_hours"] > 0


def test_analyse_sweeps_writes_impulse_responses_and_a_manifest(tmp_path, capsys):
    path, campaign, store = _campaign(tmp_path)
    _fill_sweeps(campaign, store, {"treated_small": 0.25, "office": 0.45, "live_hard": 0.80})
    assert main(["analyse-sweeps", "--campaign", str(path)]) == EXIT_OK
    manifest = store.root / "impulse_responses" / "manifest.jsonl"
    assert manifest.exists()
    entries = [json.loads(line) for line in manifest.read_text(encoding="utf-8").splitlines()]
    assert entries and all(e["licence"] == "audio-provenance-owned" for e in entries)


def test_k0_reads_the_measured_impulse_responses_and_emits_a_verdict(tmp_path, capsys):
    """The plumbing: sweeps on disk become impulse responses become a K0 record.

    RT60 recovery accuracy lives in test_sweep_math.py and the verdict logic in test_k0.py. What is
    asserted here is that the three rooms come out ordered as they were built and that the record
    names its conventions, which is what makes the number quotable.
    """
    path, campaign, store = _campaign(tmp_path)
    _fill_sweeps(campaign, store, {"treated_small": 0.25, "office": 0.45, "live_hard": 0.80})
    assert main(["k0", "--campaign", str(path)]) in (EXIT_OK, EXIT_KILL)
    verdict = json.loads(capsys.readouterr().out)

    rooms = verdict["rooms"]
    assert set(rooms) == {"treated_small", "office", "live_hard"}
    measured = {name: rooms[name]["median_rt60_seconds"] for name in rooms}
    assert measured["treated_small"] < measured["office"] < measured["live_hard"]
    assert measured["treated_small"] == pytest.approx(0.25, rel=0.25)
    assert verdict["rt60_convention"].startswith("schroeder_t30")
    assert verdict["drr_convention"] == "ace_direct_window_2p5ms"
    assert (store.root / "k0.json").exists()


def test_k0_kills_when_the_treated_room_is_too_live(tmp_path, capsys):
    path, campaign, store = _campaign(tmp_path)
    _fill_sweeps(campaign, store, {"treated_small": 0.95, "office": 0.45, "live_hard": 0.80})
    assert main(["k0", "--campaign", str(path)]) == EXIT_KILL
    verdict = json.loads(capsys.readouterr().out)
    assert verdict["verdict"] == "kill"
    assert any("above the 0.6 s limit" in reason for reason in verdict["reasons"])


def test_analysing_a_sweep_against_the_wrong_stimulus_is_refused(tmp_path, capsys):
    """Editing the campaign's sweep block after capture must not silently reanalyse with it."""
    path, campaign, store = _campaign(tmp_path)
    _fill_sweeps(campaign, store, {"treated_small": 0.25, "office": 0.45, "live_hard": 0.80})
    task = next(t for t in expand(campaign) if t.kind == "sweep")
    record = store.load_record(task)
    del record["stimulus"]
    (store.sidecar_path(task)).write_text(json.dumps(record), encoding="utf-8")

    assert main(["k0", "--campaign", str(path)]) == EXIT_ERROR
    assert "carries no `stimulus` block" in capsys.readouterr().err


def test_report_runs_a_detector_blind_and_writes_both_forms(tmp_path, capsys):
    path, campaign, store = _campaign(tmp_path)
    _fill_corpus(campaign, store)
    thresholds = tmp_path / "thresholds.json"
    thresholds.write_text(json.dumps({"presence_threshold": 0.9, "calibration_trials": 5000}), encoding="utf-8")
    payloads = tmp_path / "payloads.json"
    payloads.write_text(
        json.dumps({"schema": PAYLOAD_MAP_SCHEMA, "payloads": {"clip000": PAYLOAD, "clip001": PAYLOAD}}),
        encoding="utf-8",
    )

    code = main([
        "report", "--campaign", str(path), "--detector", "null",
        "--thresholds", str(thresholds), "--payloads", str(payloads), "--out", str(tmp_path),
    ])
    assert code == EXIT_OK
    report = json.loads((tmp_path / "capture-rig-report.json").read_text(encoding="utf-8"))
    assert report["totals"]["overall_exact_recovery_rate"] == 0.0
    assert report["blindness"]["detector_inputs"] == ["raw capture audio", "frozen threshold record"]
    assert (tmp_path / "capture-rig-report.txt").read_text(encoding="utf-8").startswith("audio-provenance-capture-rig/1")


def test_ingest_registers_a_phone_recording_as_a_first_class_trial(tmp_path, capsys):
    path, campaign, store = _campaign(tmp_path)
    task = next(t for t in expand(campaign) if t.kind == "corpus" and t.microphone_id == "phone")
    phone_file = tmp_path / "phone.wav"
    write_wav_atomic(phone_file, np.random.default_rng(1).standard_normal(SAMPLE_RATE) * 0.1, SAMPLE_RATE)

    assert main(["ingest", "--campaign", str(path), "--task", task.task_id, "--file", str(phone_file),
                 "--note", "iPhone on desk"]) == EXIT_OK
    record = store.load_record(task)
    assert record["capture"]["mode"] == "ingest"
    assert record["ingest"]["note"] == "iPhone on desk"
    assert record["channel"] == task.channel_id


def test_ingest_refuses_a_recording_at_the_wrong_sample_rate(tmp_path, capsys):
    path, campaign, store = _campaign(tmp_path)
    task = next(t for t in expand(campaign) if t.kind == "corpus" and t.microphone_id == "phone")
    wrong = tmp_path / "wrong.wav"
    write_wav_atomic(wrong, np.zeros(44100), 44100)
    assert main(["ingest", "--campaign", str(path), "--task", task.task_id, "--file", str(wrong)]) == EXIT_ERROR
    assert "does not match the campaign" in capsys.readouterr().err


def test_align_locates_clips_and_stamps_them_as_bookkeeping(tmp_path, capsys):
    rng = np.random.default_rng(2)
    clip = rng.standard_normal(SAMPLE_RATE // 2)
    clip_path = tmp_path / "clip.wav"
    write_wav_atomic(clip_path, clip, SAMPLE_RATE)
    capture_path = tmp_path / "capture.wav"
    write_wav_atomic(capture_path, np.concatenate([np.zeros(5000), clip * 0.5, np.zeros(5000)]), SAMPLE_RATE)

    segments_dir = tmp_path / "segments"
    assert main(["align", "--capture", str(capture_path), "--clip", f"c0={clip_path}",
                 "--write-segments", str(segments_dir)]) == EXIT_OK
    payload = json.loads(capsys.readouterr().out)
    assert payload["segments"][0]["accepted"]
    assert abs(payload["segments"][0]["start_sample"] - 5000) <= 2
    assert "never be fed to a detector" in payload["warning"]

    cut = segments_dir / "c0.wav"
    assert cut.exists() and payload["written_segments"]["c0"]["path"] == str(cut)
    assert payload["written_segments"]["c0"]["peak_to_sidelobe"] > 3.0
    recovered, rate = read_wav(cut)
    assert rate == SAMPLE_RATE
    assert recovered.size == clip.size


def test_analyse_ir_reports_rt60_drr_and_the_conventions(tmp_path, capsys):
    ir_path = tmp_path / "ir.wav"
    write_wav_atomic(ir_path, decaying_ir(SAMPLE_RATE, 0.4, length_seconds=1.5), SAMPLE_RATE, subtype="FLOAT")
    assert main(["analyse-ir", "--ir", str(ir_path)]) == EXIT_OK
    payload = json.loads(capsys.readouterr().out)
    assert payload["rt60"]["seconds"] == pytest.approx(0.4, rel=0.10)
    assert payload["rt60"]["convention"].startswith("schroeder_t30")
    assert payload["drr_db"] is not None


def test_ingest_refuses_a_segment_registered_against_the_wrong_clip(tmp_path, capsys):
    """A mistyped --task would score a phone capture against another clip's payload forever."""
    path, campaign, store = _campaign(tmp_path)
    tasks = [t for t in expand(campaign) if t.kind == "corpus" and t.microphone_id == "phone"]
    task, other = tasks[0], next(t for t in tasks if t.clip_id != tasks[0].clip_id)

    segment = tmp_path / "segments" / f"{task.clip_id}.wav"
    write_wav_atomic(segment, np.random.default_rng(1).standard_normal(SAMPLE_RATE) * 0.1, SAMPLE_RATE)
    alignment = tmp_path / "alignment.json"
    alignment.write_text(
        json.dumps({"written_segments": {task.clip_id: {"path": str(segment), "peak_to_sidelobe": 42.0,
                                                        "start_sample": 12}}}),
        encoding="utf-8",
    )

    assert main(["ingest", "--campaign", str(path), "--task", other.task_id,
                 "--file", str(segment), "--alignment", str(alignment)]) == EXIT_ERROR
    assert "One of the two is wrong" in capsys.readouterr().err

    assert main(["ingest", "--campaign", str(path), "--task", task.task_id,
                 "--file", str(segment), "--alignment", str(alignment)]) == EXIT_OK
    recorded = store.load_record(task)["ingest"]["alignment"]
    assert recorded["clip_id"] == task.clip_id
    assert recorded["peak_to_sidelobe"] == 42.0
