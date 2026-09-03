"""Config validation, matrix expansion, and the resume contract."""

from __future__ import annotations

import json

import numpy as np
import pytest

from capture_rig.audio import write_wav_atomic
from capture_rig.campaign import expand, estimate_seconds
from capture_rig.config import ConfigError, load_campaign
from capture_rig.record import condition_record, utc_now
from capture_rig.state import STATUS_COMPLETE, STATUS_CORRUPT, STATUS_MISSING, CaptureStore

from .fixtures import SAMPLE_RATE, build_campaign_tree


def _loaded(tmp_path):
    campaign = load_campaign(build_campaign_tree(tmp_path))
    return campaign, CaptureStore(campaign.output_dir), expand(campaign)


def test_expansion_covers_the_declared_grid_with_unique_task_ids(tmp_path):
    campaign, _, tasks = _loaded(tmp_path)
    assert len({t.task_id for t in tasks}) == len(tasks)

    sweeps = [t for t in tasks if t.kind == "sweep"]
    # 3 rooms x 2 speakers x 1 live microphone... the ingest microphone is in the grid too, because
    # a phone is a capture device the campaign owes even though this process cannot drive it.
    assert len(sweeps) == 3 * 2 * 2 * 1 * 1

    corpus = [t for t in tasks if t.kind == "corpus"]
    assert len(corpus) == 3 * 2 * 2 * 2 * 1 * 2 * 2
    assert {t.arm for t in corpus} == {"marked", "unmarked"}
    assert all(t.clip_path is not None and t.clip_path.exists() for t in corpus)


def test_channel_names_follow_the_bench_physical_convention(tmp_path):
    _, _, tasks = _loaded(tmp_path)
    corpus = next(t for t in tasks if t.kind == "corpus")
    assert corpus.channel_id == f"physical_{corpus.room_id}_{corpus.speaker_id}_{corpus.microphone_id}_" + (
        "0p5m" if corpus.distance_m == 0.5 else "1m"
    )
    noise = [t for t in tasks if t.kind == "noise"]
    assert all(t.channel_id is None for t in noise)


def test_time_estimate_is_reported_per_stage(tmp_path):
    campaign, _, tasks = _loaded(tmp_path)
    estimate = estimate_seconds(campaign, tasks)
    assert set(estimate["per_stage"]) == {"rir_sweep", "corpus"}
    assert estimate["total_seconds"] > 0
    assert estimate["per_stage"]["rir_sweep"]["seconds"] > 0


@pytest.mark.parametrize(
    "mutation,fragment",
    [
        ("schema: wrong/1", "schema must be"),
        ("id: Treated_Small", "must match"),
        ("mode: live\n    description: fixture phone", "declares no input_device"),
        ("arms: [marked]", "false-positive arm"),
        ("silence_seconds: 0.1", "shorter than sweep.ir_seconds"),
        ("sample_rate: 22050", "below 32 kHz"),
    ],
)
def test_a_campaign_that_cannot_be_run_is_refused_at_load(tmp_path, mutation, fragment):
    campaign_path = build_campaign_tree(tmp_path)
    text = campaign_path.read_text(encoding="utf-8")
    replacements = {
        "schema: wrong/1": ("schema: audio-provenance-capture-rig-campaign/1", "schema: wrong/1"),
        "id: Treated_Small": ("id: treated_small", "id: Treated_Small"),
        "mode: live\n    description: fixture phone": (
            "mode: ingest\n    description: fixture phone",
            "mode: live\n    description: fixture phone",
        ),
        "arms: [marked]": ("arms: [marked, unmarked]", "arms: [marked]"),
        "silence_seconds: 0.1": ("silence_seconds: 3.0", "silence_seconds: 0.1"),
        "sample_rate: 22050": ("sample_rate: 48000", "sample_rate: 22050"),
    }
    old, new = replacements[mutation]
    campaign_path.write_text(text.replace(old, new), encoding="utf-8")
    with pytest.raises(ConfigError, match=fragment):
        load_campaign(campaign_path)


def test_a_corpus_stage_asking_for_more_clips_than_exist_is_refused(tmp_path):
    campaign_path = build_campaign_tree(tmp_path, clips=1)
    with pytest.raises(ConfigError, match="asks for 2 clips"):
        expand(load_campaign(campaign_path))


def _commit(campaign, store, task, frames=SAMPLE_RATE):
    audio = np.random.default_rng(1).standard_normal(frames) * 0.05
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
    return store.commit(task, audio, SAMPLE_RATE, record)


def test_resume_re_queues_a_truncated_capture_and_keeps_a_complete_one(tmp_path):
    campaign, store, tasks = _loaded(tmp_path)
    corpus = [t for t in tasks if t.kind == "corpus"][:3]

    assert store.status(corpus[0]).status == STATUS_MISSING
    for task in corpus:
        _commit(campaign, store, task)
    assert store.pending(corpus) == []
    assert all(store.status(t, verify_digest=True).status == STATUS_COMPLETE for t in corpus)

    # A crash mid-write leaves a short WAV whose sidecar already claims the full length.
    truncated = corpus[1]
    audio = np.random.default_rng(2).standard_normal(SAMPLE_RATE // 4) * 0.05
    write_wav_atomic(store.wav_path(truncated), audio, SAMPLE_RATE)

    status = store.status(truncated)
    assert status.status == STATUS_CORRUPT
    assert "truncated" in (status.reason or "")
    assert [t.task_id for t in store.pending(corpus)] == [truncated.task_id]


def test_resume_detects_a_capture_whose_samples_changed_under_it(tmp_path):
    campaign, store, tasks = _loaded(tmp_path)
    task = next(t for t in tasks if t.kind == "corpus")
    _commit(campaign, store, task)

    sidecar = store.sidecar_path(task)
    record = json.loads(sidecar.read_text(encoding="utf-8"))
    record["capture"]["pcm_sha256"] = "0" * 64
    sidecar.write_text(json.dumps(record), encoding="utf-8")

    assert store.status(task).status == STATUS_COMPLETE
    assert store.status(task, verify_digest=True).status == STATUS_CORRUPT


def test_a_capture_record_reports_null_spl_without_a_meter_reading(tmp_path):
    campaign, store, tasks = _loaded(tmp_path)
    with_meter = next(t for t in tasks if t.kind == "corpus" and t.room_id == "treated_small")
    without_meter = next(t for t in tasks if t.kind == "corpus" and t.room_id == "office")

    _commit(campaign, store, with_meter)
    _commit(campaign, store, without_meter)

    calibrated = store.load_record(with_meter)["capture"]["levels"]
    assert calibrated["spl_dba"] is not None
    assert calibrated["spl_source"] == "derived_from_operator_meter_calibration"

    uncalibrated = store.load_record(without_meter)["capture"]["levels"]
    assert uncalibrated["spl_dba"] is None
    assert "no absolute reference" in uncalibrated["spl_absent_reason"]


def test_a_campaign_that_declares_no_spl_level_reports_none_and_never_zero(tmp_path):
    """An SPL target of 0.0 dBA is not a level, it is a fabrication in the one field with no null path."""
    campaign_path = build_campaign_tree(tmp_path)
    text = campaign_path.read_text(encoding="utf-8")
    campaign_path.write_text(text.replace("spl_levels_dba: [75]\n", ""), encoding="utf-8")

    campaign = load_campaign(campaign_path)
    tasks = expand(campaign)
    corpus = [t for t in tasks if t.kind == "corpus"]
    assert corpus and all(t.spl_dba is None for t in corpus)
    assert all("nospl" in t.relative_path.as_posix() for t in corpus)

    store = CaptureStore(campaign.output_dir)
    _commit(campaign, store, corpus[0])
    assert store.load_record(corpus[0])["condition"]["spl_target_dba"] is None
