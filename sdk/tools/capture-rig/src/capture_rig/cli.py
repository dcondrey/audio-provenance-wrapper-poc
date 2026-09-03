"""`capture-rig` command line.

The `align` subcommand imports `capture_rig.alignment` lazily, inside its own handler. Every other
subcommand therefore runs in a process where the alignment code is not even loaded, which is the
coarsest and most reliable form of the firewall in `capture_rig.alignment`'s docstring.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from .acoustics import drr_db, octave_band_rt60, rt60_t30
from .audio import AudioError, read_wav, to_mono, write_wav_atomic
from .calibration import CalibrationError, analyse_tone, calibration_from_tone, calibration_tone
from .campaign import CaptureTask, estimate_seconds, expand
from .config import ConfigError, load_campaign
from .detect import load_detector
from .detect.adapter import DetectorError
from .detect.runner import load_payload_map, run_trials
from .devices import DeviceError, list_devices
from .execute import ExecutionError, ingest_capture, load_room_context, run_campaign
from .record import utc_now
from .report import build_report, render_text
from .rooms import (
    RoomMeasurementError,
    analyse_sweep_capture,
    evaluate_k0,
    sweep_pair_from_record,
    write_impulse_response,
    write_rir_manifest,
)
from .state import STATUS_COMPLETE, CaptureStore, write_json_atomic

EXIT_OK = 0
EXIT_ERROR = 1
EXIT_KILL = 2


def _campaign_and_store(args):
    campaign = load_campaign(args.campaign)
    return campaign, CaptureStore(campaign.output_dir)


def _tasks(campaign, args) -> list[CaptureTask]:
    stages = tuple(args.stage) if getattr(args, "stage", None) else None
    return expand(campaign, stages)


def cmd_devices(args) -> int:
    devices = list_devices()
    if args.json:
        print(json.dumps([d.to_dict() for d in devices], indent=2))
        return EXIT_OK
    print(f"{'idx':>3}  {'in':>3} {'out':>3}  {'rate':>7}  host api      name")
    for device in devices:
        marks = ("*" if device.is_default_input else " ") + ("*" if device.is_default_output else " ")
        print(
            f"{device.index:>3}{marks} {device.max_input_channels:>3} {device.max_output_channels:>3}  "
            f"{device.default_samplerate:>7.0f}  {device.host_api:<12}  {device.name}"
        )
    print("\n'*' in the first column is the system default input, in the second the default output.")
    return EXIT_OK


def cmd_plan(args) -> int:
    campaign, store = _campaign_and_store(args)
    tasks = _tasks(campaign, args)
    survey = store.survey(tasks, verify_digest=args.verify_digests)
    counts: dict[str, int] = {}
    for cell in survey:
        counts[cell.status] = counts.get(cell.status, 0) + 1
    estimate = estimate_seconds(campaign, tasks)
    payload = {
        "campaign": campaign.to_dict(),
        "tasks": len(tasks),
        "status_counts": counts,
        "time_estimate": estimate,
        "corrupt": [c.__dict__ for c in survey if c.status not in (STATUS_COMPLETE, "missing")],
    }
    if args.json:
        print(json.dumps(payload, indent=2))
        return EXIT_OK
    print(f"campaign {campaign.campaign_id}: {len(tasks)} captures across {len(campaign.stages)} stage(s)")
    for status, count in sorted(counts.items()):
        print(f"  {status:<10} {count}")
    for stage_name, entry in estimate["per_stage"].items():
        print(f"  stage {stage_name:<16} {entry['captures']:>6} captures  {entry['seconds'] / 3600:6.2f} h playback")
    print(f"  TOTAL {estimate['total_hours']:.2f} h of playback and capture")
    print(f"  {estimate['note']}")
    for cell in payload["corrupt"]:
        print(f"  RE-QUEUED {cell['task_id']}: {cell['reason']}")
    return EXIT_OK


def cmd_calibrate(args) -> int:
    from .playback import play_and_record

    campaign, store = _campaign_and_store(args)
    room = campaign.room(args.room)
    speaker = campaign.speaker(args.speaker)
    microphone = campaign.microphone(args.microphone)
    if microphone.mode != "live" or microphone.input_device is None:
        raise CalibrationError(
            f"microphone {microphone.id!r} is mode {microphone.mode!r}; calibrate an ingest device with "
            "its own recorder and enter the meter reading into the campaign file by hand"
        )
    if speaker.output_device is None:
        raise ConfigError(f"speaker {speaker.id!r} declares no output_device")

    tone = calibration_tone(campaign.sample_rate, args.seconds, amplitude_dbfs=args.amplitude_dbfs)
    captured = play_and_record(
        tone, campaign.sample_rate, speaker.output_device, microphone.input_device, campaign.capture_channels
    )
    analysis = analyse_tone(captured, campaign.sample_rate)
    calibration, reason = calibration_from_tone(analysis, args.meter_dba, args.meter)

    record = {
        "schema": "audio-provenance-capture-rig-calibration/1",
        "generated_at": utc_now(),
        "campaign_id": campaign.campaign_id,
        "room_id": room.id,
        "speaker_id": speaker.id,
        "microphone_id": microphone.id,
        "playback_amplitude_dbfs": args.amplitude_dbfs,
        "tone_hz": 1000.0,
        "analysis": analysis.to_dict(),
        "spl_calibration": calibration.to_dict() if calibration else None,
        "spl_calibration_absent_reason": reason,
    }
    destination = store.root / "calibration" / f"{room.id}__{speaker.id}__{microphone.id}.json"
    write_json_atomic(destination, record)

    print(json.dumps(record["analysis"], indent=2))
    print(f"\nwritten to {destination}")
    if calibration is not None:
        print("\nPaste this under the room in the campaign file:\n")
        print("    spl_calibration:")
        for key, value in calibration.to_dict().items():
            print(f"      {key}: {value!r}" if isinstance(value, str) else f"      {key}: {value}")
    else:
        print(f"\nNO SPL ANCHOR: {reason}")
    return EXIT_OK if analysis.ok else EXIT_ERROR


def cmd_run(args) -> int:
    campaign, store = _campaign_and_store(args)
    tasks = _tasks(campaign, args)

    def progress(kind: str, message: str) -> None:
        print(f"[{kind}] {message}", flush=True)

    summary = run_campaign(campaign, store, tasks, progress, dry_run=args.dry_run, stop_on_error=args.stop_on_error)
    print(json.dumps({k: v for k, v in summary.items() if k != "measurements"}, indent=2))
    return EXIT_OK if summary["failed"] == 0 else EXIT_ERROR


def cmd_ingest(args) -> int:
    campaign, store = _campaign_and_store(args)
    tasks = {t.task_id: t for t in expand(campaign)}
    task = tasks.get(args.task)
    if task is None:
        raise ExecutionError(
            f"unknown task id {args.task!r}; run `capture-rig plan --json` to list the campaign's task ids"
        )
    alignment = None
    if args.alignment:
        record = json.loads(Path(args.alignment).read_text(encoding="utf-8"))
        target = str(Path(args.file).resolve())
        for clip_id, segment in (record.get("written_segments") or {}).items():
            if str(Path(segment["path"]).resolve()) == target:
                alignment = {"clip_id": clip_id, **{k: v for k, v in segment.items() if k != "path"}}
                break
        if alignment is None:
            raise ExecutionError(
                f"{args.alignment} records no written segment at {args.file}. Ingesting without the "
                "alignment evidence would leave a mis-registered --task invisible in the record."
            )
        if alignment["clip_id"] != task.clip_id:
            raise ExecutionError(
                f"{args.file} was segmented as clip {alignment['clip_id']!r} but --task registers it "
                f"as {task.clip_id!r}. One of the two is wrong, and scoring would use the task's "
                "payload forever."
            )
    context = load_room_context(store)
    path = ingest_capture(campaign, store, task, Path(args.file), context, note=args.note,
                          alignment=alignment)
    print(f"ingested {args.file} -> {path}")
    return EXIT_OK


def _sweep_measurements(campaign, store):
    for task in expand(campaign):
        if task.kind != "sweep" or store.status(task).status != STATUS_COMPLETE:
            continue
        audio, rate = read_wav(store.wav_path(task))
        if rate != campaign.sample_rate:
            raise RoomMeasurementError(
                f"{store.wav_path(task)}: recorded at {rate} Hz, campaign is {campaign.sample_rate} Hz"
            )
        pair = sweep_pair_from_record(store.load_record(task))
        yield task, analyse_sweep_capture(
            audio, pair, task, campaign.sweep.ir_seconds, campaign.sweep.pre_seconds
        )


def cmd_analyse_sweeps(args) -> int:
    campaign, store = _campaign_and_store(args)
    measurements = []
    for task, measurement in _sweep_measurements(campaign, store):
        write_impulse_response(store.root, task, measurement)
        measurements.append(measurement)
        print(
            f"[ok] {task.task_id}  RT60={measurement.rt60.get('seconds')}  "
            f"trustworthy={measurement.rt60.get('trustworthy')}  DRR={measurement.drr}  "
            f"THD={measurement.response.thd_percent}"
        )
    if not measurements:
        print("no complete sweep captures found")
        return EXIT_ERROR
    manifest = write_rir_manifest(store.root, campaign.sample_rate)
    print(f"\n{len(measurements)} impulse response(s); manifest at {manifest}")
    return EXIT_OK


def cmd_k0(args) -> int:
    campaign, store = _campaign_and_store(args)
    measurements = [measurement for _, measurement in _sweep_measurements(campaign, store)]
    verdict = evaluate_k0(measurements, campaign.rooms)
    destination = store.root / "k0.json"
    write_json_atomic(destination, verdict)
    print(json.dumps(verdict, indent=2))
    if verdict["verdict"] == "kill":
        return EXIT_KILL
    return EXIT_OK if verdict["verdict"] == "pass" else EXIT_ERROR


def cmd_analyse_ir(args) -> int:
    audio, rate = read_wav(args.ir)
    mono = to_mono(audio)
    print(
        json.dumps(
            {
                "path": str(args.ir),
                "sample_rate": rate,
                "rt60": rt60_t30(mono, rate).to_dict(),
                "rt60_octave_bands": octave_band_rt60(mono, rate),
                "drr_db": drr_db(mono, rate),
            },
            indent=2,
        )
    )
    return EXIT_OK


def cmd_align(args) -> int:
    from .alignment import segment_capture

    capture, rate = read_wav(args.capture)
    references = []
    for entry in args.clip:
        if "=" not in entry:
            raise ConfigError(f"--clip expects <clip_id>=<path>, got {entry!r}")
        clip_id, path = entry.split("=", 1)
        data, clip_rate = read_wav(path)
        if clip_rate != rate:
            raise ConfigError(f"{path}: {clip_rate} Hz does not match the capture's {rate} Hz")
        references.append((clip_id, to_mono(data)))
    mono = to_mono(capture)
    segments = segment_capture(mono, references, rate)
    written: dict[str, dict] = {}
    if args.write_segments:
        # Cutting the segments out is what makes a phone's single continuous take usable: one file per
        # clip, each then registered with `capture-rig ingest`. The cut files are DATASET material and
        # carry the same warning as the alignment record itself.
        directory = Path(args.write_segments)
        for segment in segments:
            if not segment.accepted:
                continue
            start = max(segment.start_sample, 0)
            stop = min(segment.end_sample, mono.size)
            if stop <= start:
                continue
            destination = write_wav_atomic(directory / f"{segment.clip_id}.wav", mono[start:stop], rate)
            written[segment.clip_id] = {
                "path": str(destination),
                "peak_to_sidelobe": segment.peak_to_sidelobe,
                "start_sample": segment.start_sample,
            }
    payload = {
        "schema": "audio-provenance-capture-rig-alignment/1",
        "capture": str(args.capture),
        "sample_rate": rate,
        "segments": [s.to_dict() for s in segments],
        "written_segments": written,
        "warning": (
            "Alignment is DATASET BOOKKEEPING. It uses the source clip, which is ground truth, and "
            "must never be fed to a detector. See capture_rig/alignment/__init__.py."
        ),
    }
    if args.out:
        write_json_atomic(args.out, payload)
    print(json.dumps(payload, indent=2))
    return EXIT_OK if all(s.accepted for s in segments) else EXIT_ERROR


def cmd_report(args) -> int:
    campaign, store = _campaign_and_store(args)
    tasks = _tasks(campaign, args)
    thresholds = json.loads(Path(args.thresholds).read_text(encoding="utf-8"))
    payloads = load_payload_map(args.payloads)
    detector = load_detector(args.detector, campaign.sample_rate)

    conditions: dict[str, dict] = {}
    for task in tasks:
        if task.kind != "corpus" or store.status(task).status != STATUS_COMPLETE:
            continue
        record = store.load_record(task)
        condition = dict(record.get("condition", {}))
        condition["spl_dba"] = record.get("capture", {}).get("levels", {}).get("spl_dba")
        condition["capture_mode"] = record.get("capture", {}).get("mode")
        conditions[task.task_id] = condition

    results, skipped = run_trials(store, tasks, detector, thresholds, payloads)
    report = build_report(
        campaign, results, conditions, args.detector, thresholds, skipped, include_trials=not args.no_trial_rows
    )
    out_dir = Path(args.out) if args.out else store.root
    write_json_atomic(out_dir / "capture-rig-report.json", report)
    text = render_text(report)
    (out_dir / "capture-rig-report.txt").write_text(text, encoding="utf-8")
    print(text)
    verdicts = {name: c["verdict"] for name, c in report["kill_criteria"].items()}
    if "kill" in verdicts.values():
        return EXIT_KILL
    return EXIT_OK


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="capture-rig", description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("devices", help="list CoreAudio devices PortAudio can open")
    p.add_argument("--json", action="store_true")
    p.set_defaults(func=cmd_devices)

    def with_campaign(sp):
        sp.add_argument("--campaign", required=True, help="campaign YAML")
        sp.add_argument("--stage", action="append", help="restrict to this stage (repeatable)")
        return sp

    p = with_campaign(sub.add_parser("plan", help="expand the matrix, show what is done and what it costs"))
    p.add_argument("--json", action="store_true")
    p.add_argument("--verify-digests", action="store_true", help="also re-hash every capture (slow)")
    p.set_defaults(func=cmd_plan)

    p = sub.add_parser("calibrate", help="play a 1 kHz tone and verify playback SPL and capture gain")
    p.add_argument("--campaign", required=True)
    p.add_argument("--room", required=True)
    p.add_argument("--speaker", required=True)
    p.add_argument("--microphone", required=True)
    p.add_argument("--seconds", type=float, default=5.0)
    p.add_argument("--amplitude-dbfs", type=float, default=-12.0)
    p.add_argument("--meter-dba", type=float, default=None, help="sound level meter reading at the microphone")
    p.add_argument("--meter", default=None, help="meter make and model, recorded with the reading")
    p.set_defaults(func=cmd_calibrate)

    p = with_campaign(sub.add_parser("run", help="take every pending capture, resuming"))
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--stop-on-error", action="store_true")
    p.set_defaults(func=cmd_run)

    p = sub.add_parser("ingest", help="register a recording made on a phone or other non-CoreAudio device")
    p.add_argument("--campaign", required=True)
    p.add_argument("--task", required=True, help="task id from `capture-rig plan --json`")
    p.add_argument("--file", required=True)
    p.add_argument("--note", default="")
    p.add_argument("--alignment", default=None,
                   help="alignment.json from `capture-rig align --write-segments`; records how "
                        "confidently this segment was located and refuses a --task that disagrees")
    p.set_defaults(func=cmd_ingest)

    p = sub.add_parser("analyse-sweeps", help="recompute impulse responses and the RIR manifest offline")
    p.add_argument("--campaign", required=True)
    p.set_defaults(func=cmd_analyse_sweeps)

    p = sub.add_parser("k0", help="evaluate kill criterion K0 from the measured impulse responses")
    p.add_argument("--campaign", required=True)
    p.set_defaults(func=cmd_k0)

    p = sub.add_parser("analyse-ir", help="RT60, octave-band RT60 and DRR of one impulse response file")
    p.add_argument("--ir", required=True)
    p.set_defaults(func=cmd_analyse_ir)

    p = sub.add_parser("align", help="locate known clips inside a capture (dataset bookkeeping only)")
    p.add_argument("--capture", required=True)
    p.add_argument("--clip", action="append", required=True, metavar="ID=PATH")
    p.add_argument("--out", default=None)
    p.add_argument("--write-segments", default=None, metavar="DIR",
                   help="cut each accepted segment out as <clip_id>.wav, ready for `capture-rig ingest`")
    p.set_defaults(func=cmd_align)

    p = with_campaign(sub.add_parser("report", help="run a detector BLIND over the captures and report"))
    p.add_argument("--detector", required=True, help="null | python:<module>:<factory> | exec:<command>")
    p.add_argument("--thresholds", required=True, help="threshold record frozen BEFORE this run")
    p.add_argument("--payloads", required=True, help="clip id -> payload hex, read only when scoring")
    p.add_argument("--out", default=None)
    p.add_argument("--no-trial-rows", action="store_true")
    p.set_defaults(func=cmd_report)

    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return int(args.func(args))
    except (
        AudioError,
        CalibrationError,
        ConfigError,
        DetectorError,
        DeviceError,
        ExecutionError,
        RoomMeasurementError,
    ) as exc:
        print(f"error: {type(exc).__name__}: {exc}", file=sys.stderr)
        return EXIT_ERROR
    except (OSError, ValueError) as exc:
        print(f"error: {type(exc).__name__}: {exc}", file=sys.stderr)
        return EXIT_ERROR


if __name__ == "__main__":
    raise SystemExit(main())
