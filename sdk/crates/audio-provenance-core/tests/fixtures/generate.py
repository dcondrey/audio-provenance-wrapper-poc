"""Regenerate the apw-json-sort-v1 cross-language fixtures.

Run with the POC virtualenv interpreter, which is the reference implementation:

    /Volumes/A/audio-provenance/.venv/bin/python generate.py
"""

from __future__ import annotations

import json
import math
import random
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
RECIPE = (
    'json.dumps(v, sort_keys=True, separators=(",",":"), '
    'ensure_ascii=False, allow_nan=False).encode("utf-8")'
)


def canonical(value: object) -> bytes:
    return json.dumps(
        value,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
        allow_nan=False,
    ).encode("utf-8")


def case(name: str, value: object) -> dict[str, str]:
    text = json.dumps(value, ensure_ascii=True, allow_nan=False)
    reparsed = json.loads(text)
    return {"name": name, "input": text, "python_hex": canonical(reparsed).hex()}


def nested_array(depth: int) -> object:
    value: object = []
    for _ in range(depth - 1):
        value = [value]
    return value


def nested_object(depth: int) -> object:
    value: object = {}
    for _ in range(depth - 1):
        value = {"n": value}
    return value


def nested_array_with_scalar(scalar_depth: int) -> object:
    value: object = 1
    for _ in range(scalar_depth):
        value = [value]
    return value


def parity_cases() -> list[dict[str, str]]:
    controls = "".join(chr(i) for i in range(0x20))
    cases = [
        case("empty_object", {}),
        case("empty_array", []),
        case("null_true_false", [None, True, False]),
        case("out_of_order_keys", {"z": 1, "a": 2, "M": 3, "_": 4, "0": 5, "": 6, "~": 7}),
        case(
            "codepoint_boundary_keys",
            {
                "~": 1,
                "": 2,
                "": 3,
                "߿": 4,
                "ࠀ": 5,
                "퟿": 6,
                "": 7,
                "￿": 8,
                "\U00010000": 9,
                "\U0001f600": 10,
                "\U0010ffff": 11,
            },
        ),
        case(
            "astral_and_combining_keys",
            {
                "é": "combining acute",
                "é": "precomposed acute",
                "\U0001d11e": "g clef",
                "\U0001f1e6\U0001f1e8": "regional indicators",
                "à́̂": "stacked marks",
                "́e": "leading combining mark",
            },
        ),
        case(
            "prefix_ordering_keys",
            {"a": 1, "aa": 2, "a ": 3, "ab": 4, "a\U0001f600": 5, "aé": 6},
        ),
        case("all_c0_controls", {"controls": controls}),
        case("c0_controls_in_key", {controls: "value"}),
        case(
            "unescaped_non_ascii",
            {
                "emoji": "\U0001f3b5 \U0001f9ea \U0001f600",
                "del": "",
                "line_separator": "a b",
                "paragraph_separator": "a b",
                "bom": "﻿",
                "rtl": "العربية",
                "cjk": "日本語",
                "nbsp": " ",
                "surrogate_pair_edge": "\U00010000\U0010ffff",
            },
        ),
        case(
            "solidus_and_escapes",
            {"path": "a/b/c", "escaped": 'quote " backslash \\', "mixed": "\t\n\r\b\f"},
        ),
        case("very_long_key", {"k" * 4096: "v" * 4096, "k" * 4095: 1}),
        case("integers", [0, -0, 1, -1, 2**53, -(2**53), 2**63 - 1, -(2**63)]),
        case("unsigned_upper_bound", {"u64_max": 2**64 - 1}),
        case(
            "integral_floats",
            [1.0, -0.0, 0.0, -1.0, 100.0, 1e15, 1e16, 1e21, 1e300, 1e-300],
        ),
        case("float_sum", {"sum": 0.1 + 0.2, "a": 0.1, "b": 0.2}),
        case(
            "float_threshold_neighbours",
            [
                1e-4,
                9.999e-5,
                1e-5,
                1e-7,
                9999999999999998.0,
                1e16,
                1.0000000000000002e16,
                5e-324,
                2.2250738585072014e-308,
                1.7976931348623157e308,
            ],
        ),
        case("nested_mixed", {"a": [{"b": [1, 2.5, "c", None]}, []], "": {}}),
    ]
    for depth in (1, 2, 63, 64):
        cases.append(case("nested_array_depth_%d" % depth, nested_array(depth)))
        cases.append(case("nested_object_depth_%d" % depth, nested_object(depth)))
    cases.append(case("scalar_at_depth_63", nested_array_with_scalar(63)))
    return cases


def depth_cases() -> list[dict[str, str]]:
    """CPython's canonicaliser has no depth bound; audio-provenance-core rejects these on purpose."""
    out: list[dict[str, str]] = []
    for depth in (65, 70, 100):
        for label, builder in (("array", nested_array), ("object", nested_object)):
            value = builder(depth)
            out.append(
                {
                    "name": "nested_%s_depth_%d" % (label, depth),
                    "input": json.dumps(value),
                    "python_hex": canonical(value).hex(),
                }
            )
    value = nested_array_with_scalar(64)
    out.append(
        {
            "name": "scalar_at_depth_64",
            "input": json.dumps(value),
            "python_hex": canonical(value).hex(),
        }
    )
    return out


def float_cases() -> list[dict[str, str]]:
    fixed = [
        0.0, -0.0, 1.0, -1.0, 0.1, 0.2, 0.1 + 0.2, 2.5, -2.5, 3.0, 100.0,
        1e-1, 1e-2, 1e-3, 1e-4, 9.999e-5, 1e-5, 1e-6, 1e-7, 1e-10,
        1e15, 9999999999999998.0, 1e16, 1.0000000000000002e16, 1e17, 1e21, 1e22,
        1e300, 1e-300, 5e-324, 1e-323, 2.2250738585072014e-308,
        2.225073858507201e-308, 1.7976931348623157e308, -1.7976931348623157e308,
        math.pi, math.e, -math.pi, 1234567890.123456, 4503599627370496.0,
        9007199254740992.0, 9007199254740994.0, 0.3, 1.5e-10, 123456789012345680.0,
    ]
    neighbours: list[float] = []
    for anchor in (1e16, 1e-4, 1e-5, 1.0, 1e300, 1e-300, 0.1):
        neighbours.append(math.nextafter(anchor, -math.inf))
        neighbours.append(anchor)
        neighbours.append(math.nextafter(anchor, math.inf))

    rng = random.Random(0x6E0C07E)
    randoms: list[float] = []
    while len(randoms) < 4000:
        value = struct.unpack("<d", struct.pack("<Q", rng.getrandbits(64)))[0]
        if math.isfinite(value):
            randoms.append(value)

    seen: set[bytes] = set()
    out: list[dict[str, str]] = []
    for value in fixed + neighbours + randoms:
        packed = struct.pack("<d", value)
        if packed in seen:
            continue
        seen.add(packed)
        out.append({"input": json.dumps(value), "python": json.dumps(value)})
    return out


def divergence_cases() -> list[dict[str, str]]:
    """Documented, asserted divergences from CPython. See the FIXME in canonical.rs."""
    return [
        {
            "name": "integer_beyond_u64",
            "reason": (
                "Python integers are arbitrary precision. serde_json::Value stores an integer "
                "outside i64/u64 range as f64, so precision is lost by the parser before "
                "canonicalisation sees the value."
            ),
            "input": json.dumps(2**64),
            "python": json.dumps(2**64),
            "rust": "1.8446744073709552e+19",
        },
        {
            "name": "negative_integer_beyond_i64",
            "reason": "Same cause as integer_beyond_u64, on the negative side.",
            "input": json.dumps(-(2**63) - 1),
            "python": json.dumps(-(2**63) - 1),
            "rust": "-9.223372036854776e+18",
        },
    ]


def window_hash_cases() -> list[dict[str, object]]:
    """Pins window_hash against the POC's C++ and synthetic_rehearsal.py chained hash."""
    vectors = [
        ("genesis", []),
        ("genesis", [0.0]),
        ("genesis", [1.0, -1.0, 0.5, -0.5]),
        ("genesis", [float("1e-30"), 3.4028234663852886e38, -3.4028234663852886e38]),
        ("a" * 64, [0.25, 0.125]),
        (
            "0" * 64,
            [math.sin(i / 7.0) for i in range(4096)],
        ),
    ]
    import hashlib

    out: list[dict[str, object]] = []
    for prev, samples in vectors:
        floats = [struct.unpack("<f", struct.pack("<f", s))[0] for s in samples]
        digest = hashlib.sha256(
            prev.encode() + struct.pack("<%df" % len(floats), *floats)
        ).hexdigest()
        out.append({"prev": prev, "samples": floats, "sha256": digest})
    return out


def main() -> int:
    payloads = {
        "canonical_cases.json": {"recipe": RECIPE, "cases": parity_cases()},
        "depth_cases.json": {"recipe": RECIPE, "limit": 64, "cases": depth_cases()},
        "float_cases.json": {"recipe": RECIPE, "cases": float_cases()},
        "divergences.json": {"recipe": RECIPE, "cases": divergence_cases()},
        "window_hash_cases.json": {"genesis": "genesis", "cases": window_hash_cases()},
    }
    for name, payload in payloads.items():
        (HERE / name).write_text(json.dumps(payload, indent=1, ensure_ascii=True) + "\n")
        print("wrote %s" % name)
    return 0


if __name__ == "__main__":
    sys.exit(main())
