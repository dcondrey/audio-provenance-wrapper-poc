from __future__ import annotations

import math
import statistics
import struct
import warnings
from pathlib import Path
from typing import Iterator

MAX_FEATURE_WINDOWS = 12_000
MAX_ALIGNMENT_OFFSETS = 801
MAX_COMPARISON_POINTS = 600
ALIGNMENT_CHART_POINTS = 56
# An export must overlap at least this fraction of all routed windows for the
# association to be established; the dashboard derives export guidance from it.
MIN_ROUTED_COVERAGE = 0.25
METHOD = "routed_feature_sequence_offset_search"
METHOD_VERSION = "2.0.0"

Feature = dict[str, object]


def _decode_pcm(raw: bytes, sample_width: int, byte_order: str) -> list[float]:
    if sample_width == 1:
        return [(value - 128) / 128.0 for value in raw]
    if sample_width == 2:
        fmt = ("<" if byte_order == "little" else ">") + f"{len(raw) // 2}h"
        return [value / 32768.0 for value in struct.unpack(fmt, raw)]
    if sample_width == 3:
        values: list[float] = []
        for offset in range(0, len(raw) - 2, 3):
            value = int.from_bytes(raw[offset : offset + 3], byte_order, signed=True)
            values.append(value / 8_388_608.0)
        return values
    if sample_width == 4:
        fmt = ("<" if byte_order == "little" else ">") + f"{len(raw) // 4}i"
        return [value / 2_147_483_648.0 for value in struct.unpack(fmt, raw)]
    raise ValueError(f"unsupported PCM sample width: {sample_width} bytes")


def _feature(samples: list[float]) -> Feature:
    if not samples:
        return {"rms": 0.0, "zcr": 0.0, "crest": 0.0, "envelope": (0.0,) * 4}
    rms = math.sqrt(sum(value * value for value in samples) / len(samples))
    crossings = sum(
        1
        for index in range(1, len(samples))
        if (samples[index] >= 0.0) != (samples[index - 1] >= 0.0)
    )
    zcr = crossings / (len(samples) - 1) if len(samples) > 1 else 0.0
    crest = max(abs(value) for value in samples) / rms if rms > 1e-9 else 0.0
    envelope: list[float] = []
    for segment in range(4):
        start = segment * len(samples) // 4
        end = (segment + 1) * len(samples) // 4
        values = samples[start:end]
        segment_rms = math.sqrt(sum(value * value for value in values) / len(values))
        envelope.append(segment_rms / rms if rms > 1e-9 else 0.0)
    return {"rms": rms, "zcr": zcr, "crest": crest, "envelope": tuple(envelope)}


def _open_pcm(path: Path):
    if path.suffix.lower() == ".wav":
        import wave

        handle = wave.open(str(path), "rb")
        if handle.getcomptype() != "NONE":
            handle.close()
            raise ValueError("compressed WAV is not supported")
        return handle, "little"
    if path.suffix.lower() in {".aif", ".aiff"}:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeprecationWarning)
            import aifc

        handle = aifc.open(str(path), "rb")
        if handle.getcomptype() not in {b"NONE", "NONE"}:
            handle.close()
            raise ValueError("compressed AIFF is not supported")
        return handle, "big"
    raise ValueError("only PCM WAV and AIFF exports are supported")


def extract_feature_sequence(
    path: Path,
    target_window_seconds: float,
    max_windows: int = MAX_FEATURE_WINDOWS,
) -> tuple[list[Feature], dict[str, object]]:
    """Stream bounded, mono-mixed features equivalent to routed plug-in features."""
    handle, byte_order = _open_pcm(path)
    try:
        sample_rate = int(handle.getframerate())
        channels = int(handle.getnchannels())
        sample_width = int(handle.getsampwidth())
        if sample_rate <= 0 or channels <= 0:
            raise ValueError("invalid audio format metadata")
        frames_per_window = max(128, round(sample_rate * target_window_seconds))
        sequence: list[Feature] = []
        truncated = False
        for _ in range(max_windows):
            raw = handle.readframes(frames_per_window)
            frame_count = len(raw) // max(1, channels * sample_width)
            if frame_count < frames_per_window:
                break
            decoded = _decode_pcm(raw, sample_width, byte_order)
            mono = [
                sum(decoded[index : index + channels]) / channels
                for index in range(0, len(decoded), channels)
            ]
            sequence.append(_feature(mono))
        else:
            truncated = bool(handle.readframes(1))
        return sequence, {
            "sample_rate_hz": sample_rate,
            "channel_count": channels,
            "sample_width_bytes": sample_width,
            "window_size_frames": frames_per_window,
            "window_duration_seconds": round(frames_per_window / sample_rate, 8),
            "truncated_at_window_limit": truncated,
            "streaming_extraction": True,
        }
    finally:
        handle.close()


def _number(feature: Feature, key: str) -> float | None:
    value = feature.get(key)
    if isinstance(value, (int, float)) and math.isfinite(float(value)):
        return float(value)
    return None


def _gain_reference(sequence: list[Feature]) -> float:
    levels = [
        20.0 * math.log10(max(rms, 1e-9))
        for feature in sequence
        if (rms := _number(feature, "rms")) is not None and rms > 1e-7
    ]
    return statistics.median(levels) if levels else -180.0


def _point_similarity(
    left: Feature,
    right: Feature,
    left_gain_reference: float,
    right_gain_reference: float,
) -> float:
    weighted: list[tuple[float, float]] = []
    left_rms = _number(left, "rms")
    right_rms = _number(right, "rms")
    if left_rms is not None and right_rms is not None:
        left_silent = left_rms < 1e-7
        right_silent = right_rms < 1e-7
        if left_silent != right_silent:
            weighted.append((0.25, 0.0))
        elif left_silent:
            weighted.append((0.25, 1.0))
        else:
            left_relative_db = 20.0 * math.log10(left_rms) - left_gain_reference
            right_relative_db = 20.0 * math.log10(right_rms) - right_gain_reference
            weighted.append((0.25, max(0.0, 1.0 - abs(left_relative_db - right_relative_db) / 15.0)))

    left_zcr = _number(left, "zcr")
    right_zcr = _number(right, "zcr")
    if left_zcr is not None and right_zcr is not None:
        weighted.append((0.40, max(0.0, 1.0 - abs(left_zcr - right_zcr) / 0.08)))

    left_crest = _number(left, "crest")
    right_crest = _number(right, "crest")
    if left_crest is not None and right_crest is not None and left_crest > 0 and right_crest > 0:
        crest_distance = abs(math.log2(left_crest / right_crest))
        weighted.append((0.10, max(0.0, 1.0 - crest_distance / 1.5)))

    left_envelope = left.get("envelope")
    right_envelope = right.get("envelope")
    if (
        isinstance(left_envelope, (list, tuple))
        and isinstance(right_envelope, (list, tuple))
        and len(left_envelope) == len(right_envelope)
        and len(left_envelope) > 0
    ):
        try:
            distance = sum(
                abs(float(left_value) - float(right_value))
                for left_value, right_value in zip(left_envelope, right_envelope)
            ) / len(left_envelope)
        except (TypeError, ValueError):
            pass
        else:
            weighted.append((0.25, max(0.0, 1.0 - distance / 0.75)))

    if not weighted:
        return 0.0
    weight = sum(item[0] for item in weighted)
    return sum(item_weight * score for item_weight, score in weighted) / weight


def _offsets(routed_count: int, export_count: int) -> Iterator[int]:
    low = -routed_count + 1
    high = export_count - 1
    count = high - low + 1
    if count <= MAX_ALIGNMENT_OFFSETS:
        yield from range(low, high + 1)
        return
    previous: int | None = None
    for index in range(MAX_ALIGNMENT_OFFSETS):
        value = round(low + index * (high - low) / (MAX_ALIGNMENT_OFFSETS - 1))
        if value != previous:
            yield value
            previous = value


def compare_feature_sequences(
    routed: list[Feature],
    exported: list[Feature],
    window_seconds: float,
) -> dict[str, object]:
    if not routed:
        return _unavailable("no routed feature windows were received")
    if not exported:
        return _unavailable("no comparable export feature windows were extracted")

    routed_gain = _gain_reference(routed)
    export_gain = _gain_reference(exported)
    best: tuple[float, float, int, int, list[float]] | None = None
    for offset in _offsets(len(routed), len(exported)):
        routed_start = max(0, -offset)
        export_start = max(0, offset)
        overlap = min(len(routed) - routed_start, len(exported) - export_start)
        if overlap < 3:
            continue
        stride = max(1, math.ceil(overlap / MAX_COMPARISON_POINTS))
        scores = [
            _point_similarity(
                routed[routed_start + index],
                exported[export_start + index],
                routed_gain,
                export_gain,
            )
            for index in range(0, overlap, stride)
        ]
        mean = sum(scores) / len(scores)
        matched_fraction = sum(score >= 0.72 for score in scores) / len(scores)
        routed_coverage = overlap / max(1, len(routed))
        confidence = 0.85 * mean + 0.15 * matched_fraction
        objective = confidence * (0.70 + 0.30 * min(1.0, routed_coverage))
        if best is None or objective > best[0]:
            best = (objective, confidence, offset, overlap, scores)

    if best is None:
        return _unavailable("feature sequences had no usable overlap")

    _objective, confidence, offset, overlap, scores = best
    matched_count = sum(score >= 0.72 for score in scores)
    comparable_count = len(scores)
    matched_coverage = matched_count / comparable_count
    routed_coverage = overlap / max(1, len(routed))
    established = (
        confidence >= 0.74
        and matched_coverage >= 0.60
        and routed_coverage >= MIN_ROUTED_COVERAGE
        and comparable_count >= 3
    )
    chart_stride = max(1, math.ceil(len(scores) / ALIGNMENT_CHART_POINTS))
    chart = [
        {
            "relative_window": index,
            "similarity": round(scores[index], 3),
            "matched": scores[index] >= 0.72,
        }
        for index in range(0, len(scores), chart_stride)
    ]
    return {
        "status": "inferred_match" if established else "not_established",
        "method": METHOD,
        "method_version": METHOD_VERSION,
        "confidence": round(confidence, 4),
        "matched_coverage": round(matched_coverage, 4),
        "routed_coverage": round(min(1.0, routed_coverage), 4),
        "matched_window_count": matched_count,
        "comparable_window_count": comparable_count,
        "overlap_window_count": overlap,
        "routed_window_count": len(routed),
        "export_window_count": len(exported),
        "best_offset_windows": offset,
        "best_offset_seconds": round(offset * window_seconds, 4),
        "alignment_threshold": 0.72,
        "alignment_series": chart,
        "alignment_similarity": [point["similarity"] for point in chart],
        "feature_dimensions": ["relative_rms", "zero_crossing_rate", "crest_factor", "energy_envelope_4"],
        "reason": None if established else "bounded routed/export feature similarity did not meet the inference threshold",
        "apw:proof_level": "inferred" if established else "unknown_unobserved",
        "limitations": [
            "This bounded feature comparison is not a perceptual watermark, identity system, or registry lookup.",
            "Gain normalization tolerates fixed level changes, but mastering, edits, silence, and channel mixing can reduce confidence.",
            "A failed or unavailable match does not prove that routed audio is absent from the export.",
            "Only the retained bounded routed-feature prefix participates in long-session alignment.",
        ],
    }


def associate_export(export_path: Path, routed_events: list[dict[str, object]]) -> dict[str, object]:
    routed_features: list[Feature] = []
    sample_rate = 0
    window_size = 0
    for event in routed_events:
        if event.get("event_type") != "buffer_hash":
            continue
        rms = event.get("rms_level")
        zcr = event.get("zero_crossing_rate")
        if isinstance(rms, (int, float)) and isinstance(zcr, (int, float)):
            feature: Feature = {"rms": float(rms), "zcr": float(zcr)}
            crest = event.get("crest_factor")
            if isinstance(crest, (int, float)):
                feature["crest"] = float(crest)
            envelope = event.get("energy_envelope")
            if isinstance(envelope, list) and len(envelope) == 4:
                feature["envelope"] = tuple(envelope)
            routed_features.append(feature)
        sample_rate = int(event.get("sample_rate_hz") or sample_rate)
        window_size = int(event.get("window_size_samples") or window_size)
    if not routed_features or sample_rate <= 0 or window_size <= 0:
        return _unavailable("routed events lack comparable feature, sample-rate, or window metadata")
    window_seconds = window_size / sample_rate
    try:
        export_features, export_details = extract_feature_sequence(export_path, window_seconds)
    except (EOFError, OSError, ValueError, struct.error) as exc:
        return _unavailable(str(exc))
    result = compare_feature_sequences(routed_features, export_features, window_seconds)
    result["export_feature_extraction"] = export_details
    result["routed_feature_source"] = "accepted buffer_hash events emitted from routed plug-in observations"
    result["bounded_routed_window_limit"] = MAX_FEATURE_WINDOWS
    return result


def _unavailable(reason: str) -> dict[str, object]:
    return {
        "status": "unavailable",
        "method": METHOD,
        "method_version": METHOD_VERSION,
        "confidence": None,
        "matched_coverage": 0.0,
        "routed_coverage": 0.0,
        "matched_window_count": 0,
        "comparable_window_count": 0,
        "reason": reason,
        "alignment_series": [],
        "alignment_similarity": [],
        "apw:proof_level": "unknown_unobserved",
        "limitations": [
            "Unavailable comparison is not evidence that routed audio was absent from the export."
        ],
    }
