from __future__ import annotations

import math
import struct
import warnings
from pathlib import Path
from typing import Iterator

MAX_FEATURE_WINDOWS = 12_000
MAX_ALIGNMENT_OFFSETS = 801
MAX_COMPARISON_POINTS = 600


def _decode_pcm(raw: bytes, sample_width: int, byte_order: str) -> list[float]:
    if sample_width == 1:
        return [(value - 128) / 128.0 for value in raw]
    if sample_width == 2:
        fmt = ("<" if byte_order == "little" else ">") + f"{len(raw) // 2}h"
        return [value / 32768.0 for value in struct.unpack(fmt, raw)]
    if sample_width == 3:
        values: list[float] = []
        for offset in range(0, len(raw) - 2, 3):
            value = int.from_bytes(
                raw[offset : offset + 3], byte_order, signed=True
            )
            values.append(value / 8_388_608.0)
        return values
    if sample_width == 4:
        fmt = ("<" if byte_order == "little" else ">") + f"{len(raw) // 4}i"
        return [value / 2_147_483_648.0 for value in struct.unpack(fmt, raw)]
    raise ValueError(f"unsupported PCM sample width: {sample_width} bytes")


def _feature(samples: list[float]) -> tuple[float, float]:
    if not samples:
        return 0.0, 0.0
    rms = math.sqrt(sum(value * value for value in samples) / len(samples))
    crossings = sum(
        1
        for index in range(1, len(samples))
        if (samples[index] >= 0.0) != (samples[index - 1] >= 0.0)
    )
    zcr = crossings / (len(samples) - 1) if len(samples) > 1 else 0.0
    return rms, zcr


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
) -> tuple[list[tuple[float, float]], dict[str, object]]:
    handle, byte_order = _open_pcm(path)
    try:
        sample_rate = int(handle.getframerate())
        channels = int(handle.getnchannels())
        sample_width = int(handle.getsampwidth())
        if sample_rate <= 0 or channels <= 0:
            raise ValueError("invalid audio format metadata")
        frames_per_window = max(128, round(sample_rate * target_window_seconds))
        sequence: list[tuple[float, float]] = []
        truncated = False
        for _ in range(max_windows):
            raw = handle.readframes(frames_per_window)
            if not raw:
                break
            decoded = _decode_pcm(raw, sample_width, byte_order)
            mono = [
                sum(decoded[index : index + channels]) / channels
                for index in range(0, len(decoded), channels)
            ]
            sequence.append(_feature(mono))
            if len(raw) < frames_per_window * channels * sample_width:
                break
        else:
            truncated = bool(handle.readframes(1))
        return sequence, {
            "sample_rate_hz": sample_rate,
            "channel_count": channels,
            "sample_width_bytes": sample_width,
            "window_size_frames": frames_per_window,
            "truncated_at_window_limit": truncated,
        }
    finally:
        handle.close()


def _point_similarity(left: tuple[float, float], right: tuple[float, float]) -> float:
    left_db = 20.0 * math.log10(max(left[0], 1e-7))
    right_db = 20.0 * math.log10(max(right[0], 1e-7))
    rms_similarity = max(0.0, 1.0 - abs(left_db - right_db) / 24.0)
    zcr_similarity = max(0.0, 1.0 - abs(left[1] - right[1]) / 0.35)
    return 0.65 * rms_similarity + 0.35 * zcr_similarity


def _offsets(routed_count: int, export_count: int) -> Iterator[int]:
    low = -routed_count + 1
    high = export_count - 1
    count = high - low + 1
    if count <= MAX_ALIGNMENT_OFFSETS:
        yield from range(low, high + 1)
        return
    for index in range(MAX_ALIGNMENT_OFFSETS):
        yield round(low + index * (high - low) / (MAX_ALIGNMENT_OFFSETS - 1))


def compare_feature_sequences(
    routed: list[tuple[float, float]],
    exported: list[tuple[float, float]],
    window_seconds: float,
) -> dict[str, object]:
    if not routed:
        return _unavailable("no routed feature windows were received")
    if not exported:
        return _unavailable("no comparable export feature windows were extracted")

    best: tuple[float, int, int, list[float]] | None = None
    for offset in _offsets(len(routed), len(exported)):
        routed_start = max(0, -offset)
        export_start = max(0, offset)
        overlap = min(len(routed) - routed_start, len(exported) - export_start)
        if overlap < 2:
            continue
        stride = max(1, math.ceil(overlap / MAX_COMPARISON_POINTS))
        scores = [
            _point_similarity(routed[routed_start + index], exported[export_start + index])
            for index in range(0, overlap, stride)
        ]
        mean = sum(scores) / len(scores)
        coverage = overlap / max(1, len(routed))
        objective = mean * (0.75 + 0.25 * min(1.0, coverage))
        if best is None or objective > best[0]:
            best = (objective, offset, overlap, scores)

    if best is None:
        return _unavailable("feature sequences had no usable overlap")

    confidence, offset, overlap, scores = best
    matched_coverage = overlap / max(1, len(routed))
    established = confidence >= 0.72 and matched_coverage >= 0.25
    chart_stride = max(1, math.ceil(len(scores) / 48))
    return {
        "status": "inferred_match" if established else "not_established",
        "method": "windowed_rms_zcr_offset_search_v1",
        "confidence": round(confidence, 4),
        "matched_coverage": round(min(1.0, matched_coverage), 4),
        "matched_windows": overlap,
        "routed_windows": len(routed),
        "export_windows": len(exported),
        "best_offset_windows": offset,
        "best_offset_seconds": round(offset * window_seconds, 4),
        "alignment_similarity": [round(scores[i], 3) for i in range(0, len(scores), chart_stride)],
        "apw:proof_level": "inferred" if established else "unknown_unobserved",
        "limitations": [
            "This conservative feature comparison is not a perceptual watermark or identity system.",
            "Gain, mastering, edits, silence, channel mixing, and unsupported PCM formats can reduce confidence.",
            "A failed or unavailable match does not prove that routed audio is absent from the export.",
        ],
    }


def associate_export(
    export_path: Path,
    routed_events: list[dict[str, object]],
) -> dict[str, object]:
    routed_features: list[tuple[float, float]] = []
    sample_rate = 0
    window_size = 0
    for event in routed_events:
        if event.get("event_type") != "buffer_hash":
            continue
        rms = event.get("rms_level")
        zcr = event.get("zero_crossing_rate")
        if isinstance(rms, (int, float)) and isinstance(zcr, (int, float)):
            routed_features.append((float(rms), float(zcr)))
        sample_rate = int(event.get("sample_rate_hz") or sample_rate)
        window_size = int(event.get("window_size_samples") or window_size)
    if not routed_features or sample_rate <= 0 or window_size <= 0:
        return _unavailable("routed events lack comparable feature, sample-rate, or window metadata")
    window_seconds = window_size / sample_rate
    try:
        export_features, export_details = extract_feature_sequence(export_path, window_seconds)
    except (EOFError, OSError, ValueError) as exc:
        result = _unavailable(str(exc))
        result["method"] = "windowed_rms_zcr_offset_search_v1"
        return result
    result = compare_feature_sequences(routed_features, export_features, window_seconds)
    result["export_feature_extraction"] = export_details
    return result


def _unavailable(reason: str) -> dict[str, object]:
    return {
        "status": "unavailable",
        "method": "windowed_rms_zcr_offset_search_v1",
        "confidence": None,
        "matched_coverage": 0.0,
        "reason": reason,
        "alignment_similarity": [],
        "apw:proof_level": "unknown_unobserved",
        "limitations": [
            "Unavailable comparison is not evidence that routed audio was absent from the export."
        ],
    }
