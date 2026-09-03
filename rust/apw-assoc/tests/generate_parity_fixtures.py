#!/usr/bin/env python3
"""Regenerate fixtures/association_oracle.json from daemon/audio_association.py.

Every expected value in the fixture is produced by the Python oracle, never by
hand and never by the Rust port. Run with the repository venv:

    .venv/bin/python3.13 rust/apw-assoc/tests/generate_parity_fixtures.py

The Rust parity test rebuilds the same WAV bytes from the same integer-only
generator, checks their SHA-256 against this fixture, and then compares its own
results field by field.
"""
from __future__ import annotations

import hashlib
import json
import math
import struct
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve()
REPO_ROOT = HERE.parents[3]
sys.path.insert(0, str(REPO_ROOT))

from daemon.audio_association import (  # noqa: E402
    associate_export,
    compare_feature_sequences,
    extract_feature_sequence,
)

FIXTURE = (
    Path(sys.argv[1]).resolve()
    if len(sys.argv) > 1
    else HERE.parent / "fixtures" / "association_oracle.json"
)
SAMPLE_RATE = 44_100
WINDOW_FRAMES = 4096
WINDOW_SECONDS = WINDOW_FRAMES / SAMPLE_RATE

PERIODS = (37, 5, 19, 3, 11, 71)
AMPLITUDES = (2600, 10100, 4600, 12100, 6200, 9200)
QUARTERS = (5, 10, 7, 9)


# --------------------------- integer sample source ---------------------------
def scaled(value: int, numerator: int, denominator: int) -> int:
    magnitude = abs(value) * numerator // denominator
    return -magnitude if value < 0 else magnitude


def waveform(kind: int, position: int, period: int, amplitude: int) -> int:
    if kind == 0:
        return amplitude if position * 2 < period else -amplitude
    if kind == 1:
        return amplitude - (2 * amplitude * abs(2 * position - period)) // period
    if kind == 2:
        return (2 * amplitude * position) // period - amplitude
    if position == 0:
        return 3 * amplitude
    return scaled(amplitude * ((position * 7) % 5 - 2), 1, 8)


def analytic_samples(windows: int, window_frames: int = WINDOW_FRAMES) -> list[int]:
    samples: list[int] = []
    for window in range(windows):
        period = PERIODS[window % len(PERIODS)]
        amplitude = AMPLITUDES[window % len(AMPLITUDES)]
        kind = window % 4
        for offset in range(window_frames):
            absolute = window * window_frames + offset
            quarter = QUARTERS[min(3, offset * 4 // window_frames)]
            value = scaled(waveform(kind, absolute % period, period, amplitude), quarter, 10)
            samples.append(max(-32768, min(32767, value)))
    return samples


def noise_samples(count: int, seed: int) -> list[int]:
    state = seed
    samples: list[int] = []
    for _ in range(count):
        state = (state * 1103515245 + 12345) % (1 << 31)
        samples.append((state % 20001) - 10000)
    return samples


# ------------------------------- WAV assembly -------------------------------
def frame_bytes(value: int, sample_width: int, float_format: bool) -> bytes:
    if float_format:
        return struct.pack("<f", value / 32768.0)
    if sample_width == 1:
        return bytes([max(0, min(255, scaled(value, 1, 256) + 128))])
    if sample_width == 2:
        return struct.pack("<h", value)
    if sample_width == 3:
        return (value * 256).to_bytes(3, "little", signed=True)
    if sample_width == 4:
        return struct.pack("<i", value * 65536)
    if sample_width == 8:
        return struct.pack("<q", value * 65536 * 65536)
    raise ValueError(f"unsupported test sample width {sample_width}")


def wav_bytes(spec: dict) -> bytes:
    samples = build_samples(spec)
    sample_width = spec.get("sample_width", 2)
    channels = spec.get("channels", 1)
    sample_rate = spec.get("sample_rate", SAMPLE_RATE)
    tag = spec.get("format_tag", 1)
    float_format = bool(spec.get("float_format", False))
    subtype = bytes(spec.get("subtype", []))

    data = bytearray()
    for value in samples:
        data += frame_bytes(value, sample_width, float_format)
        if channels == 2:
            data += frame_bytes(scaled(value, 6, 10), sample_width, float_format)

    bits = sample_width * 8
    block_align = channels * sample_width
    fmt = struct.pack(
        "<HHLLHH", tag, channels, sample_rate, sample_rate * block_align, block_align, bits
    )
    if tag == 0xFFFE:
        fmt += struct.pack("<HHL", 22, bits, 3) + subtype
    chunks = b"fmt " + struct.pack("<L", len(fmt)) + fmt
    if spec.get("junk_chunk"):
        junk = b"junk payload!"
        chunks += b"JUNK" + struct.pack("<L", len(junk)) + junk + b"\x00"
    truncate_fmt = spec.get("truncate_fmt_to")
    if truncate_fmt is not None:
        chunks = b"fmt " + struct.pack("<L", truncate_fmt) + fmt[:truncate_fmt]
    body = chunks + b"data" + struct.pack("<L", len(data)) + bytes(data)
    if spec.get("data_before_fmt"):
        body = b"data" + struct.pack("<L", len(data)) + bytes(data) + chunks
    return b"RIFF" + struct.pack("<L", len(body) + 4) + b"WAVE" + body


def build_samples(spec: dict) -> list[int]:
    kind = spec.get("samples", "analytic")
    if kind == "noise":
        return noise_samples(spec["windows"] * WINDOW_FRAMES, spec.get("seed", 17))
    samples = analytic_samples(spec["windows"], spec.get("window_frames", WINDOW_FRAMES))
    gain_numerator = spec.get("gain_numerator", 1)
    gain_denominator = spec.get("gain_denominator", 1)
    if (gain_numerator, gain_denominator) != (1, 1):
        samples = [scaled(value, gain_numerator, gain_denominator) for value in samples]
    lead = spec.get("lead_silence_frames", 0)
    return [0] * lead + samples


# ------------------------------ routed events ------------------------------
def routed_events(samples: list[int], window_frames: int = WINDOW_FRAMES) -> list[dict]:
    """Features computed here, directly from the integer samples, never through
    daemon.audio_association's extractor: deriving both sides from the same
    extractor would compare it with itself."""
    events: list[dict] = []
    for start in range(0, len(samples) - window_frames + 1, window_frames):
        window = [value / 32768.0 for value in samples[start : start + window_frames]]
        rms = math.sqrt(sum(value * value for value in window) / window_frames)
        crossings = sum(
            1
            for previous, current in zip(window, window[1:])
            if (previous >= 0.0) != (current >= 0.0)
        )
        quarter = window_frames // 4
        envelope = [
            math.sqrt(sum(v * v for v in window[index : index + quarter]) / quarter) / rms
            for index in range(0, window_frames, quarter)
        ]
        events.append(
            {
                "event_type": "buffer_hash",
                "rms_level": rms,
                "zero_crossing_rate": crossings / (window_frames - 1),
                "crest_factor": max(abs(value) for value in window) / rms,
                "energy_envelope": envelope,
                "sample_rate_hz": SAMPLE_RATE,
                "window_size_samples": window_frames,
            }
        )
    return events


# --------------------------- synthetic feature rule ---------------------------
def synthetic_feature(seed: int, index: int, gain: float, axes: str) -> dict:
    feature: dict = {
        "rms": (((seed * 7 + index * 13) % 97 + 1) / 128.0) * gain,
        "zcr": ((seed * 5 + index * 29) % 53) / 512.0,
    }
    if "c" in axes:
        feature["crest"] = 1.0 + ((seed + index) % 7) / 8.0
    if "e" in axes:
        feature["envelope"] = [((seed + index + band * 3) % 5) / 4.0 for band in range(4)]
    return feature


def materialise(side: dict) -> list[dict]:
    if "explicit" in side:
        return side["explicit"]
    rule = side["synthetic"]
    return [
        synthetic_feature(rule["seed"], rule["start"] + index, rule["gain"], rule["axes"])
        for index in range(rule["count"])
    ]


# --------------------------------- the cases ---------------------------------
def comparison_cases() -> list[dict]:
    cases: list[dict] = []

    def synthetic(seed, count, start=0, gain=1.0, axes="ce"):
        return {"synthetic": {"seed": seed, "count": count, "start": start, "gain": gain, "axes": axes}}

    cases.append({
        "name": "identical_sequences",
        "window_seconds": WINDOW_SECONDS,
        "routed": synthetic(3, 24),
        "exported": synthetic(3, 24),
    })
    cases.append({
        "name": "dyadic_gain_and_offset",
        "window_seconds": WINDOW_SECONDS,
        "routed": synthetic(3, 24),
        "exported": synthetic(3, 30, start=-6, gain=0.5),
    })
    cases.append({
        "name": "shifted_seed_sequences",
        "window_seconds": WINDOW_SECONDS,
        "routed": synthetic(3, 24),
        "exported": synthetic(41, 24),
    })
    cases.append({
        "name": "offsets_exactly_at_the_bound",
        "window_seconds": WINDOW_SECONDS,
        "routed": synthetic(3, 401, axes=""),
        "exported": synthetic(3, 401, axes=""),
    })
    cases.append({
        "name": "offsets_one_past_the_bound_subsamples",
        "window_seconds": WINDOW_SECONDS,
        "routed": synthetic(3, 401, axes=""),
        "exported": synthetic(3, 402, axes=""),
    })
    cases.append({
        "name": "overlap_601_strides_the_comparison",
        "window_seconds": WINDOW_SECONDS,
        "routed": synthetic(5, 601, axes="e"),
        "exported": synthetic(5, 601, axes="e"),
    })
    cases.append({
        "name": "comparable_56_windows",
        "window_seconds": WINDOW_SECONDS,
        "routed": synthetic(9, 56),
        "exported": synthetic(9, 56),
    })
    cases.append({
        "name": "comparable_57_windows_strides_the_chart",
        "window_seconds": WINDOW_SECONDS,
        "routed": synthetic(9, 57),
        "exported": synthetic(9, 57),
    })

    # Exactly the minimum overlap and comparable count.
    cases.append({
        "name": "overlap_exactly_three",
        "window_seconds": WINDOW_SECONDS,
        "routed": {"explicit": [synthetic_feature(3, index, 1.0, "ce") for index in range(3)]},
        "exported": {"explicit": [synthetic_feature(3, index, 1.0, "ce") for index in range(3)]},
    })
    cases.append({
        "name": "overlap_two_has_no_usable_overlap",
        "window_seconds": WINDOW_SECONDS,
        "routed": {"explicit": [synthetic_feature(3, index, 1.0, "ce") for index in range(2)]},
        "exported": {"explicit": [synthetic_feature(3, index, 1.0, "ce") for index in range(2)]},
    })

    # routed_coverage exactly at MIN_ROUTED_COVERAGE: 3 of 12 routed windows.
    cases.append({
        "name": "routed_coverage_exactly_at_the_floor",
        "window_seconds": WINDOW_SECONDS,
        "routed": {"explicit": [synthetic_feature(11, index, 1.0, "ce") for index in range(12)]},
        "exported": {"explicit": [synthetic_feature(11, index + 9, 1.0, "ce") for index in range(3)]},
    })

    # matched_coverage exactly 0.60: three of five windows agree, two do not.
    matched = [synthetic_feature(13, index, 1.0, "ce") for index in range(5)]
    mismatched = list(matched)
    mismatched[3] = {"rms": 0.5, "zcr": 0.4, "crest": 6.0, "envelope": [1.5, 0.1, 1.5, 0.1]}
    mismatched[4] = {"rms": 0.004, "zcr": 0.02, "crest": 9.0, "envelope": [0.0, 1.9, 0.0, 1.9]}
    cases.append({
        "name": "matched_coverage_three_of_five",
        "window_seconds": WINDOW_SECONDS,
        "routed": {"explicit": matched},
        "exported": {"explicit": mismatched},
    })

    # Every axis at an intermediate distance, so each tolerance is load-bearing:
    # a uniform gain would cancel in the RMS axis, so the level ratio alternates.
    partial_routed = [
        {
            "rms": 0.25 + (index % 5) / 64.0,
            "zcr": 0.10 + (index % 7) / 512.0,
            "crest": 2.0 + (index % 3) / 4.0,
            "envelope": [0.5 + ((index + band) % 4) / 16.0 for band in range(4)],
        }
        for index in range(24)
    ]
    partial_export = [
        {
            "rms": feature["rms"] * (0.75 if index % 2 == 0 else 0.4375),
            "zcr": feature["zcr"] + 0.01 + (index % 3) / 1024.0,
            "crest": feature["crest"] * (1.25 + (index % 4) / 8.0),
            "envelope": [
                value + (0.0625 if band % 2 else -0.0625) * (1 + index % 3)
                for band, value in enumerate(feature["envelope"])
            ],
        }
        for index, feature in enumerate(partial_routed)
    ]
    cases.append({
        "name": "every_axis_at_an_intermediate_distance",
        "window_seconds": WINDOW_SECONDS,
        "routed": {"explicit": partial_routed},
        "exported": {"explicit": partial_export},
    })

    # A fine sweep across the 0.72 alignment threshold on the ZCR axis alone.
    sweep_routed = [{"rms": 0.25, "zcr": 0.20} for _ in range(41)]
    sweep_export = [
        {"rms": 0.25, "zcr": 0.20 + (0.0224 + (index - 20) * 2e-7)} for index in range(41)
    ]
    cases.append({
        "name": "zcr_threshold_sweep",
        "window_seconds": WINDOW_SECONDS,
        "routed": {"explicit": sweep_routed},
        "exported": {"explicit": sweep_export},
    })

    # The decision thresholds, pinned from both sides. Only the ZCR axis is
    # present, so a window's score is exactly 1 - delta / 0.08, and the base
    # rate is spread far enough that no shifted alignment can win.
    def zcr_pair(matched_delta: float, unmatched_delta: float, matched: int, unmatched: int):
        routed = [{"zcr": 0.05 + index * 0.09} for index in range(matched + unmatched)]
        exported = [
            {"zcr": feature["zcr"] + (matched_delta if index < matched else unmatched_delta)}
            for index, feature in enumerate(routed)
        ]
        return routed, exported

    def established(unmatched_delta: float) -> bool:
        routed, exported = zcr_pair(0.0112, unmatched_delta, 3, 2)
        return compare_feature_sequences(routed, exported, WINDOW_SECONDS)["status"] == "inferred_match"

    low, high = 0.0224, 0.05
    assert established(low) and not established(high)
    for _ in range(200):
        middle = (low + high) / 2
        if middle in (low, high):
            break
        if established(middle):
            low = middle
        else:
            high = middle
    for name, delta in (("confidence_just_at_the_floor", low), ("confidence_just_below_the_floor", high)):
        routed, exported = zcr_pair(0.0112, delta, 3, 2)
        cases.append({
            "name": name,
            "window_seconds": WINDOW_SECONDS,
            "routed": {"explicit": routed},
            "exported": {"explicit": exported},
        })

    routed, exported = zcr_pair(0.004, 0.0256, 5, 4)
    cases.append({
        "name": "matched_coverage_five_of_nine",
        "window_seconds": WINDOW_SECONDS,
        "routed": {"explicit": routed},
        "exported": {"explicit": exported},
    })

    # Three offsets score identically; the search keeps the first, so the
    # reported offset is 0 rather than the last tie.
    tied = {"rms": 0.25, "zcr": 0.10, "crest": 2.0, "envelope": [1.0, 1.0, 1.0, 1.0]}
    cases.append({
        "name": "tied_objectives_keep_the_first_offset",
        "window_seconds": WINDOW_SECONDS,
        "routed": {"explicit": [dict(tied) for _ in range(3)]},
        "exported": {"explicit": [dict(tied) for _ in range(5)]},
    })

    # The silence floor is a strict `<`, and both sides are gain-normalised, so
    # a routed window at 1.02e-7 scores 1.0 against an export 10^5 louder.
    quiet = [1e-7, 1.02e-7, 1.05e-7, 1.08e-7, 1.03e-7, 1.06e-7]
    cases.append({
        "name": "levels_just_above_the_silence_floor",
        "window_seconds": WINDOW_SECONDS,
        "routed": {"explicit": [
            {"rms": level, "zcr": 0.05 + index * 0.09} for index, level in enumerate(quiet)
        ]},
        "exported": {"explicit": [
            {"rms": level * 100_000.0, "zcr": 0.05 + index * 0.09}
            for index, level in enumerate(quiet)
        ]},
    })

    # Silence, absent axes, degenerate crest, envelope length disagreement.
    cases.append({
        "name": "silence_and_absent_axes",
        "window_seconds": 0.5,
        "routed": {"explicit": [
            {"rms": 0.0, "zcr": 0.0, "crest": 0.0, "envelope": [0.0, 0.0, 0.0, 0.0]},
            {"rms": 1e-7, "zcr": 0.1, "crest": 2.0, "envelope": [1.0, 1.0, 1.0, 1.0]},
            {"rms": 9.999999999999999e-8, "zcr": 0.1, "crest": 2.0},
            {"rms": 1e-9, "zcr": 0.1},
            {"rms": 0.3, "zcr": 0.1, "crest": 0.0, "envelope": [1.0, 1.0, 1.0, 1.0]},
            {"rms": 0.3, "zcr": 0.1, "crest": 3.0, "envelope": [1.0, 1.0]},
            {"zcr": 0.1},
            {},
        ]},
        "exported": {"explicit": [
            {"rms": 0.0, "zcr": 0.0, "crest": 0.0, "envelope": [0.0, 0.0, 0.0, 0.0]},
            {"rms": 0.2, "zcr": 0.1, "crest": 2.0, "envelope": [1.0, 1.0, 1.0, 1.0]},
            {"rms": 1e-7, "zcr": 0.1, "crest": 2.0},
            {"rms": 1e-9, "zcr": 0.1},
            {"rms": 0.3, "zcr": 0.1, "crest": 4.0, "envelope": [1.0, 1.0, 1.0, 1.0]},
            {"rms": 0.3, "zcr": 0.1, "crest": 3.0, "envelope": [1.0, 1.0, 1.0, 1.0]},
            {"zcr": 0.4},
            {},
        ]},
    })

    # An even count of non-silent windows exercises the median's mean-of-two arm.
    cases.append({
        "name": "even_gain_reference_median",
        "window_seconds": WINDOW_SECONDS,
        "routed": {"explicit": [
            {"rms": 0.1, "zcr": 0.05}, {"rms": 0.2, "zcr": 0.05},
            {"rms": 0.3, "zcr": 0.05}, {"rms": 0.4, "zcr": 0.05},
        ]},
        "exported": {"explicit": [
            {"rms": 0.05, "zcr": 0.05}, {"rms": 0.1, "zcr": 0.05},
            {"rms": 0.15, "zcr": 0.05}, {"rms": 0.2, "zcr": 0.05},
        ]},
    })

    cases.append({
        "name": "empty_routed",
        "window_seconds": WINDOW_SECONDS,
        "routed": {"explicit": []},
        "exported": synthetic(3, 8),
    })
    cases.append({
        "name": "empty_export",
        "window_seconds": WINDOW_SECONDS,
        "routed": synthetic(3, 8),
        "exported": {"explicit": []},
    })
    return cases


def wav_cases() -> list[dict]:
    subtype_pcm = list(bytes.fromhex("0100000000001000800000aa00389b71"))
    subtype_float = list(bytes.fromhex("0300000000001000800000aa00389b71"))
    return [
        {"name": "exact_export", "file": "exact.wav", "spec": {"windows": 16}},
        {"name": "gain_and_offset_export", "file": "transformed.wav",
         "spec": {"windows": 16, "gain_numerator": 43, "gain_denominator": 100,
                  "lead_silence_frames": 8192}},
        {"name": "unrelated_export", "file": "unrelated.wav",
         "spec": {"windows": 16, "samples": "noise", "seed": 17}},
        {"name": "stereo_export", "file": "stereo.wav", "spec": {"windows": 8, "channels": 2}},
        {"name": "eight_bit_export", "file": "eight_bit.wav",
         "spec": {"windows": 8, "sample_width": 1}},
        {"name": "twenty_four_bit_export", "file": "twenty_four.wav",
         "spec": {"windows": 8, "sample_width": 3}},
        {"name": "thirty_two_bit_export", "file": "thirty_two.wav",
         "spec": {"windows": 8, "sample_width": 4}},
        {"name": "junk_chunk_is_skipped", "file": "junk.wav",
         "spec": {"windows": 8, "junk_chunk": True}},
        {"name": "extensible_pcm_export", "file": "extensible_pcm.wav",
         "spec": {"windows": 8, "format_tag": 0xFFFE, "subtype": subtype_pcm}},
        {"name": "extensible_float_is_refused", "file": "extensible_float.wav",
         "spec": {"windows": 8, "sample_width": 4, "format_tag": 0xFFFE,
                  "float_format": True, "subtype": subtype_float}},
        {"name": "float_tag_is_refused", "file": "float.wav",
         "spec": {"windows": 8, "sample_width": 4, "format_tag": 3, "float_format": True}},
        {"name": "sixty_four_bit_width_is_refused", "file": "sixty_four.wav",
         "spec": {"windows": 8, "sample_width": 8}},
        {"name": "zero_sample_rate", "file": "zero_rate.wav",
         "spec": {"windows": 8, "sample_rate": 0}},
        {"name": "data_before_fmt", "file": "data_first.wav",
         "spec": {"windows": 2, "data_before_fmt": True}},
        {"name": "truncated_fmt_chunk", "file": "short_fmt.wav",
         "spec": {"windows": 2, "truncate_fmt_to": 12}},
        {"name": "shorter_than_one_window", "file": "tiny.wav",
         "spec": {"windows": 1, "window_frames": 512}},
        {"name": "not_a_riff_file", "file": "garbage.wav", "raw": "not an audio file"},
        {"name": "riff_header_shorter_than_eight_bytes", "file": "stub.wav", "raw": "RIFF!"},
        {"name": "empty_file", "file": "empty.wav", "raw": ""},
        {"name": "unsupported_suffix", "file": "unsupported.mp3", "raw": "not an audio file"},
        {"name": "missing_file", "file": "absent.wav", "missing": True},
    ]


def round_cases() -> list[list]:
    values = [
        0.0625, -0.0625, 0.03125, 2.5, 0.5, 1.5, -0.5, -2.5, 0.0, -0.0,
        1 / 3, 2 / 3, math.pi, math.e, 0.72, 0.985, 1.005, 12345.678,
        0.1 + 0.2, 1e-5, 1e-7, 3.0517578125e-05, 5e-324, 1.7976931348623157e308,
        0.85 * 0.9 + 0.15 * 1.0, 0.7199999999999999, 0.7200000000000001,
        1.0, 0.9999999999999999, 44100 / 4096, 4096 / 44100, 8192 / 44100,
        123456789.123456789, 2.675, -2.675, 0.15625, 1023.5, 1024.5,
    ]
    cases: list[list] = []
    for value in values:
        for digits in (0, 1, 2, 3, 4, 8):
            cases.append([value, digits, round(value, digits)])
    return cases


# ---------------------------------- driver ----------------------------------
def feature_to_json(feature: dict) -> dict:
    rendered = {}
    for key in ("rms", "zcr", "crest"):
        if key in feature:
            rendered[key] = feature[key]
    if "envelope" in feature:
        rendered["envelope"] = list(feature["envelope"])
    return rendered


def main() -> int:
    fixture: dict = {
        "python": {"executable": sys.executable, "version": sys.version.split()[0]},
        "sample_rate_hz": SAMPLE_RATE,
        "window_frames": WINDOW_FRAMES,
        "window_seconds": WINDOW_SECONDS,
        "round_cases": round_cases(),
        "comparison_cases": comparison_cases(),
        "wav_cases": wav_cases(),
        "extraction_cases": [],
    }

    # Re-read the declared inputs from their serialised form, so every expected
    # value is computed from exactly the bytes the Rust side will parse.
    fixture = json.loads(json.dumps(fixture))

    for case in fixture["comparison_cases"]:
        routed = materialise(case["routed"])
        exported = materialise(case["exported"])
        case["expected"] = compare_feature_sequences(routed, exported, case["window_seconds"])

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        routed = routed_events(analytic_samples(16))
        fixture["routed_events"] = json.loads(json.dumps(routed))
        for case in fixture["wav_cases"]:
            path = root / case["file"]
            if case.get("missing"):
                case["sha256"] = None
            elif "raw" in case:
                path.write_bytes(case["raw"].encode())
                case["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
            else:
                path.write_bytes(wav_bytes(case["spec"]))
                case["sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
            result = associate_export(path, fixture["routed_events"])
            if case.get("missing") and isinstance(result.get("reason"), str):
                result["reason"] = result["reason"].replace(str(root), "<DIR>")
            case["expected"] = result

        for name, file_name, windows, target_seconds, max_windows in (
            ("bounded_extraction", "exact.wav", 16, WINDOW_SECONDS, 4),
            ("full_extraction", "stereo.wav", 8, WINDOW_SECONDS, 12_000),
            ("floored_window_length", "exact.wav", 16, 1e-6, 6),
        ):
            path = root / file_name
            sequence, details = extract_feature_sequence(path, target_seconds, max_windows)
            fixture["extraction_cases"].append({
                "name": name,
                "file": file_name,
                "target_window_seconds": target_seconds,
                "max_windows": max_windows,
                "features": [feature_to_json(feature) for feature in sequence],
                "details": details,
            })

    FIXTURE.parent.mkdir(parents=True, exist_ok=True)
    FIXTURE.write_text(json.dumps(fixture, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {FIXTURE} ({FIXTURE.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
