"""Regenerates the apw-core canonicalization fixtures from the Python oracle.

The Python implementation in daemon/ is the behavioural specification, so the
expected bytes in tests/fixtures/canonical_cases.json are produced by it and
never hand-written.

    /Volumes/A/audio-provenance/.venv/bin/python \
        rust/apw-core/tests/generate_fixtures.py
"""

from __future__ import annotations

import json
import pathlib
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO_ROOT))

from daemon.common import canonical_json_bytes  # noqa: E402


def canonical_ascii(value: object) -> bytes:
    """daemon/manifest_builder/generator.py:667 and daemon/verify.py:513."""
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")


def pretty(value: object) -> bytes:
    """daemon/manifest_builder/builder.py:442 and generator.py:709."""
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


CASES: list[tuple[str, object]] = [
    ("empty_object", {}),
    ("empty_array", []),
    ("scalars", [None, True, False, "", 0, -0, "0"]),
    (
        "float_thresholds",
        {
            "fixed_low": 1e-4,
            "exp_low": 1e-5,
            "exp_lower": 1e-7,
            "quantum": 3.0517578125e-5,
            "fixed_high": 1e15,
            "fixed_high_edge": 9999999999999998.0,
            "exp_high": 1e16,
            "exp_high_carry": 2e16 + 8,
            "zero": 0.0,
            "negative_zero": -0.0,
            "denormal_min": 5e-324,
            "float_max": 1.7976931348623157e308,
            "tenth": 0.1,
            "negative": -2.5,
            "big_e21": 1e21,
        },
    ),
    (
        "int_versus_float_sample_rate",
        {"int_rate": 44100, "float_rate": 44100.0, "int_one": 1, "float_one": 1.0},
    ),
    (
        "integer_extremes",
        {
            "i64_min": -9223372036854775808,
            "i64_max": 9223372036854775807,
            "u64_max": 18446744073709551615,
            "zero": 0,
            "negative_one": -1,
        },
    ),
    (
        "escapes",
        {
            "control": "\x00\x01\x1f",
            "shortforms": "\b\t\n\x0c\r",
            "quote_backslash_solidus": '"\\/',
            "delete": "\x7f",
            "tab_in_key\t": "value",
        },
    ),
    (
        "non_ascii",
        {
            "cafe": "Café",
            "astral": "\U0001f600",
            "cjk": "音声",
            "combining": "é",
            "surrogate_pair_boundary": "\U00010000\U0010ffff",
        },
    ),
    (
        "key_ordering",
        {
            "éclair": 1,
            "zebra": 2,
            "Zebra": 3,
            "apple": 4,
            "Apple": 5,
            "": 6,
            "apw:proof_level": "directly_observed",
            "\U0001f600": 7,
            "a.b": 8,
            "a b": 9,
        },
    ),
    (
        "nested_manifest_shape",
        {
            "apw_version": "0.9.0",
            "schema": "audio-provenance-manifest-v0",
            "session_id": "sess-é-01",
            "created_at": "2025-08-31T00:26:40.500000Z",
            "observed_stems": [
                {
                    "stem_id": "stem-1",
                    "hash_chain_length": 12,
                    "first_observed_ms": -1,
                    "sample_rate_hz": 44100,
                    "rms": 0.0001220703125,
                    "plugin_instance_ids": [],
                    "apw:proof_level": "directly_observed",
                    "source": {"category": "Café synth", "apw:proof_level": "user_declared"},
                }
            ],
            "export": {
                "file_name": "Bounce – final.wav",
                "duration_seconds": 12.345678901234567,
                "sample_rate_hz": 44100.0,
                "channel_count": 2,
                "apw:proof_level": "directly_observed",
            },
            "apw:unobserved": ["hidden_plugin_state", "bypassed_routing"],
            "observation_coverage": {"status": "unknown_coverage", "counters": {}},
            "empty_list": [],
            "empty_object": {},
            "deep": {"a": {"b": {"c": {"d": [{"e": None}]}}}},
        },
    ),
]


def main() -> None:
    fixtures = []
    for name, value in CASES:
        fixtures.append(
            {
                "name": name,
                "value": value,
                "canonical_utf8": canonical_json_bytes(value).decode("utf-8"),
                "canonical_ascii": canonical_ascii(value).decode("utf-8"),
                "pretty": pretty(value).decode("utf-8"),
            }
        )
    target = pathlib.Path(__file__).resolve().parent / "fixtures" / "canonical_cases.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(
        json.dumps(fixtures, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    print(f"wrote {len(fixtures)} cases to {target}")


if __name__ == "__main__":
    main()
