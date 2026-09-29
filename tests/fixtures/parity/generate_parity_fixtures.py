#!/usr/bin/env python3
"""Regenerate the Python/Rust parity fixtures in this directory.

Every expected value is produced by the Python oracle (daemon/), never by hand and
never by the Rust port. Python 3.12+ is required because the forgery screen's
`sum()` is Neumaier-compensated from 3.12, and the Rust port reproduces that.

    ./.venv/bin/python tests/fixtures/parity/generate_parity_fixtures.py

Outputs, checked by tests/test_parity_fixtures.py and by
rust/apw-core/tests/time_anchor_parity.rs and rust/apw-daemon/tests/forgery_parity.rs:

    forgery_analysis.json   derive_forgery_analysis over fixed event streams
    time_anchor.json        RFC 3161 request/response/verify/record vectors
    manifest_rehearsal.json a real signed manifest (forgery_analysis + anchored
                            time_anchor) with its canonical signing-byte digests
"""
from __future__ import annotations

import contextlib
import hashlib
import io
import json
import sys
import tempfile
import unittest.mock
from pathlib import Path

if sys.version_info < (3, 12):
    raise SystemExit("Python 3.12+ is required: earlier sum() is not compensated")

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO))
sys.path.insert(0, str(REPO / "scripts"))

from daemon import verify as verify_module  # noqa: E402
from daemon.common import canonical_json_bytes  # noqa: E402
from daemon.manifest_builder.generator import derive_forgery_analysis  # noqa: E402
from daemon.time_anchor import anchor as anchor_module  # noqa: E402
from daemon.time_anchor import ots as ots_module  # noqa: E402
from daemon.time_anchor.http import HttpResult  # noqa: E402
from daemon.time_anchor.anchor import (  # noqa: E402
    MAX_TSA_RESPONSE_BYTES,
    RFC3161Provider,
    TimeAnchorService,
    TimeProof,
    _der,
    _der_children,
    _der_integer,
    _der_read,
    encode_timestamp_request,
    parse_timestamp_response,
)

FIXTURES = REPO / "tests" / "fixtures"
GENTIME = "20260928123000Z"


def dump(name: str, payload: object) -> None:
    (HERE / name).write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {HERE / name}")


# ------------------------------- forgery analysis -------------------------------
class Lcg:
    """Integer-only generator so the streams never depend on a library's RNG."""

    def __init__(self, seed: int) -> None:
        self.state = seed

    def next(self, bound: int) -> int:
        self.state = (self.state * 6364136223846793005 + 1442695040888963407) % (1 << 64)
        return (self.state >> 33) % bound

    def unit(self) -> float:
        return self.next(1_000_000) / 1_000_000


def buffer_hash(index: int, rms: object, zcr: object, centroid: object, *, chained: bool = True,
                timestamp: object = None, window_hash: object = None, prev_hash: object = None) -> dict:
    return {
        "event_type": "buffer_hash",
        "timestamp_ms": 10_000 + index * 93 if timestamp is None else timestamp,
        "window_hash": f"{index:064x}" if window_hash is None else window_hash,
        "prev_hash": (f"{index - 1:064x}" if index else "genesis") if prev_hash is None and chained
        else prev_hash,
        "rms_level": rms,
        "zero_crossing_rate": zcr,
        "spectral_centroid_hz": centroid,
    }


def transitions(intervals: list[int], start: int = 5_000) -> list[dict]:
    events, now = [], start
    events.append({"event_type": "audio_transition", "timestamp_ms": now})
    for interval in intervals:
        now += interval
        events.append({"event_type": "audio_transition", "timestamp_ms": now})
    return events


def keystrokes(counts_and_ikis: list[tuple[int, float]]) -> list[dict]:
    return [
        {"event_type": "input_keystroke_stats", "count": count, "mean_iki_ms": iki}
        for count, iki in counts_and_ikis
    ]


def forgery_cases() -> list[dict]:
    rng = Lcg(7)
    cases: list[tuple[str, list[dict]]] = []
    cases.append(("empty", []))
    cases.append(("unknown_and_missing_event_types", [
        {"event_type": "midi"}, {"no_type": 1}, {"event_type": 5}, {"event_type": None},
    ]))
    cases.append(("varied_audio_no_flags", [
        buffer_hash(i, 0.05 + 0.4 * rng.unit(), 0.02 + 0.3 * rng.unit(), 500 + 3000 * rng.unit())
        for i in range(80)
    ] + transitions([300 + rng.next(9000) for _ in range(30)])))
    cases.append(("regular_rms_and_spectrum", [
        buffer_hash(i, 0.5 + 0.0001 * (i % 3), 0.3, 2000 + (i % 2) * 5) for i in range(60)
    ]))
    cases.append(("regular_rms_below_min_windows", [buffer_hash(i, 0.5, 0.3, 2000) for i in range(49)]))
    cases.append(("regular_rms_at_min_windows", [buffer_hash(i, 0.5, 0.3, 2000) for i in range(50)]))
    cases.append(("metronomic_transitions", transitions([1000] * 20)))
    cases.append(("metronomic_at_ten_intervals", transitions([1000] * 10)))
    cases.append(("metronomic_at_nine_intervals", transitions([1000] * 9)))
    cases.append(("superhuman_speed_half_even_12_5_percent", transitions([50] * 2 + [700 + 41 * i for i in range(14)])))
    cases.append(("superhuman_speed_half_even_37_5_percent", transitions([50] * 6 + [700 + 41 * i for i in range(10)])))
    cases.append(("superhuman_speed_all", transitions([50] * 20)))
    cases.append(("transition_intervals_out_of_range", transitions([0, -5, 300_000, 299_999] + [400 + 37 * i for i in range(12)])))
    cases.append(("chain_break_duplicate_reversal", [
        buffer_hash(0, 0.2, 0.1, 900),
        buffer_hash(1, 0.3, 0.1, 950),
        buffer_hash(2, 0.4, 0.1, 990, prev_hash="f" * 64),
        buffer_hash(3, 0.1, 0.1, 400, window_hash=f"{2:064x}", timestamp=10_050),
        buffer_hash(4, 0.2, 0.1, 500, timestamp=9_000),
    ]))
    cases.append(("chain_intact", [buffer_hash(i, 0.1 + 0.01 * i, 0.1, 500 + i) for i in range(20)]))
    cases.append(("chain_short_prev_hash_list", [
        buffer_hash(0, 0.2, 0.1, 900), {**buffer_hash(1, 0.3, 0.1, 950), "prev_hash": 7},
        buffer_hash(2, 0.3, 0.1, 950, prev_hash="x"),
    ]))
    cases.append(("input_regular_all_flags", keystrokes([(25, 100.0)] * 8)))
    cases.append(("input_below_min_samples", keystrokes([(99, 120.0)])))
    cases.append(("input_at_min_samples", keystrokes([(100, 120.0)])))
    cases.append(("input_superhuman", keystrokes([(30, 10.0), (30, 400.0), (40, 200.0)])))
    cases.append(("input_varied_human_like", keystrokes(
        [(1 + rng.next(6), float(60 + rng.next(700))) for _ in range(120)]
    )))
    cases.append(("input_fatigue_and_skew", keystrokes(
        [(3, 80.0 + 0.9 * i + (rng.next(50) if i % 5 == 0 else 0)) for i in range(70)]
        + [(4, 900.0 - i) for i in range(6)]
    )))
    cases.append(("input_tiny_speedup", keystrokes([(20, 200.0), (20, 180.0), (20, 190.0), (20, 199.0), (20, 210.0)])))
    cases.append(("malformed_field_types", [
        {"event_type": "buffer_hash", "rms_level": True, "zero_crossing_rate": "0.1",
         "spectral_centroid_hz": None, "timestamp_ms": True, "window_hash": 5, "prev_hash": None},
        {"event_type": "buffer_hash", "rms_level": 10**300, "timestamp_ms": 1.5e300,
         "window_hash": "a", "prev_hash": "a"},
        {"event_type": "buffer_hash", "rms_level": 0.25, "timestamp_ms": 12345.9, "window_hash": "b", "prev_hash": "a"},
        {"event_type": "audio_transition", "timestamp_ms": "1000"},
        {"event_type": "audio_transition", "timestamp_ms": 1000.9},
        {"event_type": "input_keystroke_stats", "count": 3.0, "mean_iki_ms": 100.0},
        {"event_type": "input_keystroke_stats", "count": True, "mean_iki_ms": 100.0},
        {"event_type": "input_keystroke_stats", "count": None, "mean_iki_ms": 100.0},
        {"event_type": "input_keystroke_stats", "count": "4", "mean_iki_ms": 100.0},
        {"event_type": "input_keystroke_stats", "count": -5, "mean_iki_ms": 100.0},
        {"event_type": "input_keystroke_stats", "mean_iki_ms": 100.0},
        {"event_type": "input_keystroke_stats", "count": 3, "mean_iki_ms": "100"},
        {"event_type": "input_keystroke_stats", "count": 3, "mean_iki_ms": False},
    ]))
    cases.append(("keystroke_count_cap", keystrokes([(10**30, 100.0), (10_000, 250.0), (10_001, 90.0)])))
    cases.append(("rounding_stress", [
        buffer_hash(i, 0.1 + (i % 7) * 0.1 + 1e-12 * i, 0.3, 700 + (i * 7919) % 13) for i in range(64)
    ] + keystrokes([(1, 0.1 * ((i * 37) % 11) + 0.05) for i in range(150)])))
    return [{"name": name, "events": events, "expected": derive_forgery_analysis(events)}
            for name, events in cases]


# ------------------------------- RFC 3161 vectors -------------------------------
SHA256_OID_TLV = bytes.fromhex("0609608648016503040201")
SIGNED_DATA_OID_TLV = bytes.fromhex("06092a864886f70d010702")
TST_INFO_OID_TLV = bytes.fromhex("060b2a864886f70d0109100104")
POLICY_OID_TLV = bytes.fromhex("06032a0304")


def synthetic_token(
    data_hash: str,
    nonce: bytes | None,
    gentime: bytes | str = GENTIME,
    *,
    status: int = 0,
    algorithm_oid: bytes = SHA256_OID_TLV,
    with_token: bool = True,
    nonce_is_integer: bool = True,
    extra_before_gentime: bytes = b"",
    tst_tag: int = 0x30,
    serial: int = 0x0102030405,
) -> bytes:
    if isinstance(gentime, str):
        gentime = gentime.encode()
    imprint = _der(0x30, _der(0x30, algorithm_oid + _der(0x05, b"")) + _der(0x04, bytes.fromhex(data_hash)))
    tst_body = (
        _der(0x02, b"\x01") + POLICY_OID_TLV + imprint + _der_integer(serial) + extra_before_gentime
        + _der(0x18, gentime)
    )
    if nonce is not None:
        tst_body += _der_integer(int.from_bytes(nonce, "big")) if nonce_is_integer else _der(0x04, nonce)
    tst_info = _der(tst_tag, tst_body)
    signed_data = _der(0x30, _der(0x02, b"\x03") + _der(0x31, b"") + _der(
        0x30, TST_INFO_OID_TLV + _der(0xA0, _der(0x04, tst_info))))
    token = _der(0x30, SIGNED_DATA_OID_TLV + _der(0xA0, signed_data))
    status_info = _der(0x30, _der(0x02, bytes([status]) if status < 256 else status.to_bytes(2, "big")))
    return _der(0x30, status_info + (token if with_token else b""))


def parse_result(body: bytes) -> dict:
    try:
        parsed = parse_timestamp_response(body)
    except (ValueError, IndexError, UnicodeDecodeError) as error:
        message = str(error)
        exact = message.startswith(("malformed TimeStampResp", "DER ", "unsupported DER", "TSTInfo message"))
        return {"error": message, "message_exact": exact}
    return {"ok": {
        "granted": parsed.granted,
        "status": parsed.status,
        "gentime_ms": parsed.gentime_ms,
        "imprint_hash_hex": parsed.imprint_hash_hex,
        "nonce_hex": None if parsed.nonce is None else format(parsed.nonce, "x"),
    }}


def load_digicert() -> tuple[bytes, str, str]:
    response = bytes.fromhex((FIXTURES / "rfc3161_digicert_response.hex").read_text().strip())
    request = json.loads((FIXTURES / "rfc3161_digicert_request.json").read_text())
    return response, request["data_hash"], request["nonce_hex"]


def time_anchor_fixture() -> dict:
    response, digicert_hash, digicert_nonce = load_digicert()
    digicert_gentime = parse_timestamp_response(response).gentime_ms
    hash_a = hashlib.sha256(b"parity export a").hexdigest()
    nonce_a = bytes.fromhex("00" * 3 + "a1b2c3d4e5f60718293a4b5c6d")
    good = synthetic_token(hash_a, nonce_a)

    parse_bodies: dict[str, bytes] = {
        "digicert_live_response": response,
        "synthetic_granted": good,
        "granted_with_mods": synthetic_token(hash_a, nonce_a, status=1),
        "rejected_status_2": synthetic_token(hash_a, nonce_a, status=2, with_token=False),
        "rejected_status_5_with_token": synthetic_token(hash_a, nonce_a, status=5),
        "granted_without_token": synthetic_token(hash_a, nonce_a, with_token=False),
        "status_wider_than_one_byte": synthetic_token(hash_a, nonce_a, status=258),
        "no_nonce": synthetic_token(hash_a, None),
        "nonce_high_bit": synthetic_token(hash_a, bytes.fromhex("80" + "11" * 15)),
        "nonce_all_zero": synthetic_token(hash_a, b"\x00" * 16),
        "nonce_octet_string_not_integer": synthetic_token(hash_a, nonce_a, nonce_is_integer=False),
        "sha1_imprint": synthetic_token(hash_a, nonce_a, algorithm_oid=bytes.fromhex("06052b0e03021a")),
        "tstinfo_not_sequence": synthetic_token(hash_a, nonce_a, tst_tag=0x31),
        "gentime_fractional": synthetic_token(hash_a, nonce_a, "20260928123000.987Z"),
        "gentime_no_z": synthetic_token(hash_a, nonce_a, "20260928123000"),
        "gentime_many_z": synthetic_token(hash_a, nonce_a, "20260928123000ZZZ"),
        "gentime_leap_day": synthetic_token(hash_a, nonce_a, "20240229235959Z"),
        "gentime_not_leap_day": synthetic_token(hash_a, nonce_a, "20230229000000Z"),
        "gentime_year_1": synthetic_token(hash_a, nonce_a, "00010101000000Z"),
        "gentime_year_0": synthetic_token(hash_a, nonce_a, "00000101000000Z"),
        "gentime_year_9999": synthetic_token(hash_a, nonce_a, "99991231235959Z"),
        "gentime_epoch": synthetic_token(hash_a, nonce_a, "19700101000000Z"),
        "gentime_before_epoch": synthetic_token(hash_a, nonce_a, "19691231235959Z"),
        "gentime_second_60": synthetic_token(hash_a, nonce_a, "20260928123060Z"),
        "gentime_second_61": synthetic_token(hash_a, nonce_a, "20260928123061Z"),
        "gentime_second_62": synthetic_token(hash_a, nonce_a, "20260928123062Z"),
        "gentime_month_13": synthetic_token(hash_a, nonce_a, "20261328123000Z"),
        "gentime_month_00": synthetic_token(hash_a, nonce_a, "20260028123000Z"),
        "gentime_day_32": synthetic_token(hash_a, nonce_a, "20260932123000Z"),
        "gentime_hour_24": synthetic_token(hash_a, nonce_a, "20260928243000Z"),
        "gentime_minute_60": synthetic_token(hash_a, nonce_a, "20260928126000Z"),
        "gentime_lenient_13_digits": synthetic_token(hash_a, nonce_a, "2026092812300Z"),
        "gentime_lenient_12_digits": synthetic_token(hash_a, nonce_a, "202609281230Z"),
        "gentime_lenient_10_digits": synthetic_token(hash_a, nonce_a, "2026092812Z"),
        "gentime_lenient_9_digits": synthetic_token(hash_a, nonce_a, "202609281Z"),
        "gentime_lenient_8_digits": synthetic_token(hash_a, nonce_a, "20260928Z"),
        "gentime_lenient_7_digits": synthetic_token(hash_a, nonce_a, "2026092Z"),
        "gentime_lenient_6_digits": synthetic_token(hash_a, nonce_a, "202609Z"),
        "gentime_lenient_5_digits": synthetic_token(hash_a, nonce_a, "20260Z"),
        "gentime_15_digits": synthetic_token(hash_a, nonce_a, "202609281230001Z"),
        "gentime_trailing_text": synthetic_token(hash_a, nonce_a, "20260928123000Zx"),
        "gentime_leading_space": synthetic_token(hash_a, nonce_a, " 20260928123000Z"),
        "gentime_space_padded_day": synthetic_token(hash_a, nonce_a, "2026092 123000Z"),
        "gentime_non_digit": synthetic_token(hash_a, nonce_a, "2026-09-28T12:30Z"),
        "gentime_non_ascii": synthetic_token(hash_a, nonce_a, "2026092812300\xe9Z".encode("latin-1")),
        "gentime_empty": synthetic_token(hash_a, nonce_a, ""),
        "gentime_after_extra_children": synthetic_token(hash_a, nonce_a, extra_before_gentime=_der(0x02, b"\x07")),
        "empty": b"",
        "one_byte": b"\x30",
        "wrong_outer_tag": b"\x31" + good[1:],
        "trailing_bytes_after_response": good + b"\x00\xff",
        "indefinite_length": b"\x30\x80" + good[2:],
        "five_byte_length": b"\x30\x85\x00\x00\x00\x00\x05" + good[2:],
        "length_overrun": good[:-1],
        "digicert_truncated_40": response[:40],
        "digicert_truncated_minus_11": response[:-11],
        "digicert_flipped_imprint_byte": response.replace(bytes.fromhex(digicert_hash)[:4], b"\xff\xff\xff\xff", 1),
    }
    parse_vectors = [
        {"name": name, "response_hex": body.hex(), **parse_result(body)}
        for name, body in parse_bodies.items()
    ]

    request_cases = [
        ("digicert_fixture", digicert_hash, bytes.fromhex(digicert_nonce)),
        ("nonce_leading_zero_bytes", hash_a, bytes.fromhex("0000ab")),
        ("nonce_high_bit", hash_a, bytes.fromhex("ff" * 16)),
        ("nonce_all_zero", hash_a, b"\x00" * 16),
        ("nonce_empty", hash_a, b""),
        ("nonce_one_byte_7f", hash_a, b"\x7f"),
        ("nonce_one_byte_80", hash_a, b"\x80"),
        ("nonce_20_bytes", hash_a, bytes(range(1, 21))),
        ("nonce_200_bytes_long_form_length", hash_a, bytes([0x81] * 200)),
        ("uppercase_hash", hash_a.upper(), b"\x01" * 16),
        ("hash_with_whitespace", " ".join(hash_a[i:i + 2] for i in range(0, 64, 2)), b"\x01" * 16),
        ("short_hash", hash_a[:62], b"\x01" * 16),
        ("odd_length_hash", hash_a[:63], b"\x01" * 16),
        ("non_hex_hash", "zz" + hash_a[2:], b"\x01" * 16),
    ]
    request_vectors = []
    for name, data_hash, nonce in request_cases:
        try:
            der = encode_timestamp_request(data_hash, nonce)
            request_vectors.append({"name": name, "data_hash": data_hash, "nonce_hex": nonce.hex(),
                                    "request_hex": der.hex()})
        except ValueError:
            request_vectors.append({"name": name, "data_hash": data_hash, "nonce_hex": nonce.hex(),
                                    "request_hex": None})

    # anchor_record: the real TimeAnchorService against a patched transport.
    url = "http://tsa.parity.example/ts"
    record_cases: list[tuple[str, str, bytes, bytes]] = [
        ("digicert_live_response", digicert_hash, bytes.fromhex(digicert_nonce), response),
        ("synthetic_granted", hash_a, nonce_a, good),
        ("synthetic_fractional_gentime", hash_a, nonce_a, synthetic_token(hash_a, nonce_a, "20260928123000.5Z")),
        ("synthetic_year_one", hash_a, nonce_a, synthetic_token(hash_a, nonce_a, "00010203040506Z")),
        ("wrong_imprint", "0" * 64, nonce_a, good),
        ("wrong_nonce", hash_a, bytes.fromhex("11" * 16), good),
        ("no_nonce_echo", hash_a, nonce_a, synthetic_token(hash_a, None)),
        ("refused_status_2", hash_a, nonce_a, synthetic_token(hash_a, nonce_a, status=2, with_token=False)),
        ("granted_without_token", hash_a, nonce_a, synthetic_token(hash_a, nonce_a, with_token=False)),
        ("malformed_body", hash_a, nonce_a, good[:30]),
        ("empty_body", hash_a, nonce_a, b""),
        ("sha1_imprint", hash_a, nonce_a, synthetic_token(hash_a, nonce_a, algorithm_oid=bytes.fromhex("06052b0e03021a"))),
        ("oversized_body", hash_a, nonce_a, b"\x00" * (MAX_TSA_RESPONSE_BYTES + 1)),
        ("body_at_size_bound_is_parsed", hash_a, nonce_a, good + b"\x00" * (MAX_TSA_RESPONSE_BYTES - len(good))),
    ]
    # Bodies at the size bound are stored as their non-zero prefix plus a pad length.
    padded = {"oversized_body": 0, "body_at_size_bound_is_parsed": len(good)}
    record_vectors = []
    for name, data_hash, nonce, body in record_cases:
        service = TimeAnchorService(RFC3161Provider(url))
        result = HttpResult(200, "HTTP/1.1 200 OK", body[: MAX_TSA_RESPONSE_BYTES + 1])
        with unittest.mock.patch.object(anchor_module, "http_fetch", return_value=result), \
                unittest.mock.patch.object(anchor_module.secrets, "token_bytes", return_value=nonce):
            record = service.anchor_record(data_hash)
        record_vectors.append({
            "name": name, "tsa_url": url, "data_hash": data_hash, "nonce_hex": nonce.hex(),
            "response_hex": body[: padded.get(name, len(body))].hex(),
            "pad_zero_to": len(body) if name in padded else None,
            "expected": record,
            "message_exact": not str(record.get("reason", "")).startswith(
                ("time data", "unconverted", "day ", "year ", "second ", "hour ", "minute ", "month ")
            ),
        })

    # verifier vectors: _check_time_anchor over hand-shaped manifests.
    anchored = anchor_module.TimeAnchorService  # noqa: F841 (documenting the source of `record`)
    digicert_record = next(v for v in record_vectors if v["name"] == "digicert_live_response")["expected"]
    synthetic_record = next(v for v in record_vectors if v["name"] == "synthetic_granted")["expected"]

    def manifest_for(anchor: object, export: object = "same") -> dict:
        if export == "same":
            export = {"sha256": anchor.get("data_hash") if isinstance(anchor, dict) else None}
        data: dict = {"time_anchor": anchor}
        if export is not None:
            data["export"] = export
        return data

    def mutated(record: dict, **changes: object) -> dict:
        result = dict(record)
        for key, value in changes.items():
            key = "apw:proof_level" if key == "apw_proof_level" else key
            if value is ...:
                result.pop(key, None)
            else:
                result[key] = value
        return result

    base_record = synthetic_record
    base_nonce = nonce_a.hex()
    base_token = base_record["response_der_hex"]
    verify_cases: list[tuple[str, dict]] = [
        ("absent", {"export": {"sha256": hash_a}}),
        ("null", {"time_anchor": None, "export": {"sha256": hash_a}}),
        ("not_an_object_list", {"time_anchor": [], "export": {"sha256": hash_a}}),
        ("not_an_object_string", {"time_anchor": "anchored", "export": {"sha256": hash_a}}),
        ("unavailable", manifest_for({"status": "unavailable", "data_hash": hash_a,
                                      "apw:proof_level": "unknown_unobserved"})),
        ("status_missing", manifest_for({"data_hash": hash_a})),
        ("status_unknown", manifest_for({"status": "pending", "data_hash": hash_a})),
        ("status_not_string", manifest_for({"status": ["anchored"], "data_hash": hash_a})),
        ("consistent_digicert", manifest_for(digicert_record)),
        ("digicert_uppercase_token_hex", manifest_for(mutated(digicert_record, response_der_hex=digicert_record["response_der_hex"].upper()))),
        ("consistent_synthetic", manifest_for(synthetic_record)),
        ("consistent_unknown_unobserved_label", manifest_for(mutated(base_record, apw_proof_level="unknown_unobserved"))),
        ("uppercase_base_token", manifest_for(mutated(base_record, response_der_hex=base_token.upper()))),
        ("whitespace_between_token_bytes", manifest_for(mutated(
            base_record, response_der_hex=" ".join(base_token[i:i + 2] for i in range(0, len(base_token), 2))))),
        ("whitespace_inside_a_byte", manifest_for(mutated(base_record, response_der_hex="0" + " " + base_token[1:]))),
        ("empty_nonce_hex", manifest_for(mutated(base_record, nonce_hex=""))),
        ("nonce_with_leading_zero_bytes", manifest_for(mutated(base_record, nonce_hex="0000" + base_nonce))),
        ("wrong_nonce", manifest_for(mutated(base_record, nonce_hex="00" * 16))),
        ("odd_nonce_hex", manifest_for(mutated(base_record, nonce_hex="abc"))),
        ("non_hex_token", manifest_for(mutated(base_record, response_der_hex="zz"))),
        ("truncated_token", manifest_for(mutated(base_record, response_der_hex=base_token[:-22]))),
        ("empty_token", manifest_for(mutated(base_record, response_der_hex=""))),
        ("tampered_timestamp_ms", manifest_for(mutated(base_record, timestamp_ms=base_record["timestamp_ms"] + 1000))),
        ("timestamp_ms_float", manifest_for(mutated(base_record, timestamp_ms=float(base_record["timestamp_ms"])))),
        ("timestamp_ms_bool", manifest_for(mutated(base_record, timestamp_ms=True))),
        ("timestamp_ms_missing", manifest_for(mutated(base_record, timestamp_ms=...))),
        ("timestamp_ms_huge", manifest_for(mutated(base_record, timestamp_ms=10**40))),
        ("source_missing", manifest_for(mutated(base_record, source=...))),
        ("source_not_string", manifest_for(mutated(base_record, source=5))),
        ("nonce_hex_missing", manifest_for(mutated(base_record, nonce_hex=...))),
        ("token_missing", manifest_for(mutated(base_record, response_der_hex=...))),
        ("token_not_string", manifest_for(mutated(base_record, response_der_hex=12))),
        ("proof_level_directly_observed", manifest_for(mutated(base_record, apw_proof_level="directly_observed"))),
        ("proof_level_externally_verified", manifest_for(mutated(base_record, apw_proof_level="externally_verified"))),
        ("proof_level_missing", manifest_for(mutated(base_record, apw_proof_level=...))),
        ("proof_level_not_string", manifest_for(mutated(base_record, apw_proof_level=3))),
        ("cms_verified_true", manifest_for(mutated(base_record, cms_signature_verified=True))),
        ("cms_verified_zero", manifest_for(mutated(base_record, cms_signature_verified=0))),
        ("cms_verified_missing", manifest_for(mutated(base_record, cms_signature_verified=...))),
        ("cms_verified_null", manifest_for(mutated(base_record, cms_signature_verified=None))),
        ("data_hash_differs_from_export", manifest_for(base_record, export={"sha256": "1" * 64})),
        ("data_hash_uppercase", manifest_for(mutated(base_record, data_hash=hash_a.upper()),
                                             export={"sha256": hash_a})),
        ("export_missing", {"time_anchor": base_record}),
        ("export_not_object", {"time_anchor": base_record, "export": "x"}),
        ("export_hash_null_and_anchor_hash_null", {
            "time_anchor": mutated(base_record, data_hash=None), "export": {"sha256": None}}),
        ("export_sha_missing_anchor_hash_null", {
            "time_anchor": mutated(base_record, data_hash=None), "export": {}}),
        ("export_hash_not_string", {
            "time_anchor": mutated(base_record, data_hash=5), "export": {"sha256": 5}}),
    ]
    verify_vectors = []
    for name, data in verify_cases:
        result = verify_module.VerificationResult()
        verify_module._check_time_anchor(data, result)
        verify_vectors.append({
            "name": name,
            "manifest": data,
            "expected_findings": [
                {"severity": f.severity.value if hasattr(f.severity, "value") else str(f.severity),
                 "code": f.code, "message": f.message}
                for f in result.findings
            ],
        })

    # verify(): the provider-level primitive, over TimeProof inputs.
    proofs = []
    provider = RFC3161Provider()
    for name, proof, data_hash in [
        ("digicert_ok", TimeProof("s", digicert_gentime, digicert_nonce, response.hex(), None), digicert_hash),
        ("digicert_wrong_hash", TimeProof("s", digicert_gentime, digicert_nonce, response.hex(), None), "0" * 64),
        ("digicert_wrong_time", TimeProof("s", digicert_gentime + 1, digicert_nonce, response.hex(), None), digicert_hash),
        ("digicert_wrong_nonce", TimeProof("s", digicert_gentime, "00" * 16, response.hex(), None), digicert_hash),
        ("digicert_uppercase_response", TimeProof("s", digicert_gentime, digicert_nonce, response.hex().upper(), None), digicert_hash),
        ("rejected_token", TimeProof("s", 0, nonce_a.hex(), synthetic_token(hash_a, nonce_a, status=2, with_token=False).hex(), None), hash_a),
    ]:
        proofs.append({
            "name": name, "timestamp_ms": proof.timestamp_ms, "nonce_hex": proof.nonce_hex,
            "response_hex": proof.response_hex, "data_hash": data_hash,
            "expected": provider.verify(proof, data_hash),
        })

    return {
        "max_response_bytes": MAX_TSA_RESPONSE_BYTES,
        "digicert": {"data_hash": digicert_hash, "nonce_hex": digicert_nonce, "gentime_ms": digicert_gentime},
        "parse": parse_vectors,
        "request": request_vectors,
        "record": record_vectors,
        "verify": verify_vectors,
        "provider_verify": proofs,
    }


# --------------------------- a real signed rehearsal manifest ---------------------------
def rehearsal_fixture(manifest: dict) -> dict:

    portable_input = {k: v for k, v in manifest.items() if k not in {"portable_signature", "manifest_signature"}}
    local_input = {k: v for k, v in manifest.items() if k != "manifest_signature"}
    portable_bytes = canonical_json_bytes(portable_input)
    local_bytes = json.dumps(local_input, sort_keys=True, separators=(",", ":")).encode()
    if manifest["portable_signature"]["signed_content_hash"] != hashlib.sha256(portable_bytes).hexdigest():
        raise SystemExit("portable signing input reconstruction disagrees with the daemon's signature")
    result = verify_module.VerificationResult()
    verify_module._check_time_anchor(manifest, result)
    return {
        "manifest": manifest,
        "portable_signing_input_sha256": hashlib.sha256(portable_bytes).hexdigest(),
        "portable_signing_input_length": len(portable_bytes),
        "local_signing_input_sha256": hashlib.sha256(local_bytes).hexdigest(),
        "local_signing_input_length": len(local_bytes),
        "time_anchor_findings": [
            {"severity": str(f.severity.value if hasattr(f.severity, "value") else f.severity),
             "code": f.code, "message": f.message}
            for f in result.findings
        ],
    }


# ----------------------- session state, host environment, key paths -----------------------
def _fake_daemon():
    import collections
    import threading
    import types

    from daemon.__main__ import Daemon

    fake = types.SimpleNamespace(
        _session_lock=threading.Lock(),
        _active_layers=set(),
        _plugin_instance_ids=collections.OrderedDict(),
        _latest_plugin_telemetry=collections.OrderedDict(),
        _telemetry_regressions=0,
        _buffer_hash_count=0,
        _first_hash_event=None,
        _last_hash_event=None,
        _feature_events=collections.deque(maxlen=12_000),
        _feature_window_drops=0,
        _host_environment=None,
        _host_environment_conflicts=0,
        _append_event=lambda event: None,
    )
    fake._record_host_environment = lambda event: Daemon._record_host_environment(fake, event)
    return fake, Daemon


def session_cases() -> list[dict]:
    from daemon.manifest_builder.generator import derive_host_environment

    def plugin(instance: str, **telemetry: int) -> dict:
        return {"event_type": "session_config_change", "plugin_instance_id": instance, "telemetry": telemetry}

    def host(recognised: object, name: object = None, executable: object = None, fmt: object = "VST3") -> dict:
        return {"event_type": "host_environment", "host_recognised": recognised, "host_name": name,
                "host_executable_name": executable, "wrapper_format": fmt}

    cases = [
        ("no_events", []),
        ("telemetry_monotonic", [plugin("a", x=1, y=5), plugin("a", x=2, y=5), plugin("a", z=9)]),
        ("telemetry_regression_ignored_and_counted", [plugin("a", x=10, y=5), plugin("a", x=3), plugin("a", x=11, y=4)]),
        ("telemetry_equal_is_not_a_regression", [plugin("a", x=10), plugin("a", x=10)]),
        ("telemetry_non_int_ignored", [{"event_type": "midi_event", "plugin_instance_id": "a",
                                        "telemetry": {"f": 1.5, "s": "3", "b": True, "n": None, "i": 4}}]),
        ("telemetry_lru_eviction", [plugin("a", **{f"k{i:02d}": i for i in range(40)}),
                                     plugin("a", **{f"m{i:02d}": i for i in range(40)}),
                                     plugin("a", k00=1, k01=99)]),
        ("host_none_reported", [plugin("a", x=1)]),
        ("host_recognised", [host(True, "Ableton Live", "Live", "AU")]),
        ("host_unrecognised", [host(False, None, "mystery.exe", "VST3")]),
        ("host_recognised_then_other_format_same_host", [host(True, "Logic", "Logic Pro", "AU"), host(True, "Logic", "Logic Pro X", "VST3")]),
        ("host_conflict_on_name", [host(True, "Logic"), host(True, "Cubase")]),
        ("host_conflict_on_recognition", [host(True, "Logic"), host(False, None)]),
        ("host_conflicts_counted", [host(True, "Logic"), host(True, "X"), host(True, "Y"), host(True, "Logic")]),
        ("host_first_wins", [host(False, None, "a"), host(False, None, "b")]),
        ("host_falsy_fields_normalised", [host(0, "", "", "")]),
        ("host_truthy_non_bool_recognised", [host(1, "Reaper", "reaper", "CLAP")]),
        ("host_name_ignored_when_unrecognised", [host(False, "Ghost", "g", "VST3")]),
    ]
    out = []
    for name, events in cases:
        fake, daemon_cls = _fake_daemon()
        for event in events:
            daemon_cls._record_plugin_event(fake, event, "session")
        out.append({
            "name": name,
            "events": events,
            "telemetry": [[key, value] for key, value in fake._latest_plugin_telemetry.items()],
            "telemetry_regressions": fake._telemetry_regressions,
            "host_environment": derive_host_environment(fake),
        })
    return out


def network_validation_cases() -> list[dict]:
    from daemon.evidence_receiver.taxonomy import validate_network_event

    def host(**overrides: object) -> dict:
        event = {"event_type": "host_environment", "proof_level": "directly_observed",
                 "host_recognised": True, "host_name": "Live", "host_executable_name": "Live",
                 "wrapper_format": "AU"}
        event.update(overrides)
        return {k: v for k, v in event.items() if v is not ...}

    def config(**overrides: object) -> dict:
        event = {"event_type": "session_config_change", "proof_level": "directly_observed",
                 "sample_rate_hz": 44100, "channel_count": 2}
        event.update(overrides)
        return event

    nested: object = 1
    for _ in range(9):
        nested = {"a": nested}
    cases = [
        ("host_valid", host()),
        ("host_unrecognised_null_name", host(host_recognised=False, host_name=None, host_executable_name=None)),
        ("host_missing_wrapper_format", host(wrapper_format=...)),
        ("host_null_wrapper_format", host(wrapper_format=None)),
        ("host_missing_recognised", host(host_recognised=...)),
        ("host_recognised_not_bool", host(host_recognised=1)),
        ("host_name_not_string", host(host_name=5)),
        ("host_name_at_limit", host(host_name="x" * 128)),
        ("host_name_over_limit", host(host_name="x" * 129)),
        ("host_name_multibyte_at_limit", host(host_name="\u00e9" * 128)),
        ("host_name_absent", host(host_name=...)),
        ("host_proof_level_too_strong", host(proof_level="externally_verified")),
        ("telemetry_valid", config(telemetry={"a": 0, "b": 2**53})),
        ("telemetry_not_object", config(telemetry=[1])),
        ("telemetry_null", config(telemetry=None)),
        ("telemetry_33_entries", config(telemetry={f"k{i}": 1 for i in range(33)})),
        ("telemetry_32_entries", config(telemetry={f"k{i}": 1 for i in range(32)})),
        ("telemetry_empty_key", config(telemetry={"": 1})),
        ("telemetry_key_65_chars", config(telemetry={"k" * 65: 1})),
        ("telemetry_key_64_chars", config(telemetry={"k" * 64: 1})),
        ("telemetry_float", config(telemetry={"a": 1.0})),
        ("telemetry_bool", config(telemetry={"a": True})),
        ("telemetry_string", config(telemetry={"a": "1"})),
        ("telemetry_negative", config(telemetry={"a": -1})),
        ("telemetry_over_range", config(telemetry={"a": 2**53 + 1})),
        ("telemetry_huge_int", config(telemetry={"a": 10**40})),
        ("telemetry_quote_in_key", config(telemetry={"it's": 1.5})),
        ("nested_at_depth_limit", config(extra={"a": {"a": {"a": {"a": {"a": {"a": {"a": 1}}}}}}})),
        ("nested_over_depth_limit", config(extra=nested)),
        ("instance_id_long", config(plugin_instance_id="i" * 129)),
        ("instance_id_at_limit", config(plugin_instance_id="i" * 128)),
        ("instance_id_not_string", config(plugin_instance_id=7)),
        ("capture_session_id_not_string", config(plugin_capture_session_id=[])),
        ("numeric_field_string", config(sample_rate_hz="44100")),
    ]
    out = []
    for name, event in cases:
        ok, message = validate_network_event(event)
        out.append({"name": name, "event": event, "ok": ok, "message": message})
    return out


def key_paths(value: object, path: str = "", out: dict | None = None) -> dict:
    """path -> {kind, keys}: every dict key path (list indexes collapse to []), the
    value kinds seen there, and the insertion order of each object's keys."""
    out = {} if out is None else out
    kind = ("null" if value is None else "bool" if isinstance(value, bool) else "int" if isinstance(value, int)
            else "float" if isinstance(value, float) else "str" if isinstance(value, str)
            else "list" if isinstance(value, list) else "object")
    entry = out.setdefault(path or "/", {"kinds": [], "keys": None})
    if kind not in entry["kinds"]:
        entry["kinds"].append(kind)
        entry["kinds"].sort()
    if isinstance(value, dict):
        if entry["keys"] is None:
            entry["keys"] = list(value)
        for key, child in value.items():
            key_paths(child, f"{path}/{key}", out)
    elif isinstance(value, list):
        for child in value:
            key_paths(child, f"{path}[]", out)
    return out


# Values that differ per run and are excluded from the comparison: only the kind and the
# key order are compared at these paths, never the content. Kept deliberately empty of
# key paths: every difference in which keys exist is a failure.
RUN_SPECIFIC_NOTE = (
    "Only key paths, value kinds and key order are compared; values (ids, hashes, paths, "
    "timestamps, signatures, counts) are run-specific and never compared."
)


def rehearsal_capture() -> dict:
    """Run the Python rehearsal once, capturing the exact wire events it sent."""
    import synthetic_rehearsal

    sent: list[dict] = []
    original = synthetic_rehearsal._send_with_acknowledgements

    def capture(events, port):
        result = original(events, port)
        sent.extend(json.loads(json.dumps(events)))
        return result

    def fake_tsa(url, *, data=None, **_kwargs):
        _tag, content, _ = _der_read(data, 0)
        children = _der_children(content)
        body = synthetic_token(_der_children(children[1][1])[1][1].hex(), children[2][1])
        return HttpResult(200, "HTTP/1.1 200 OK", body)

    def fake_calendar(url, *, method="GET", data=None, **_kwargs):
        assert method == "POST" and url.endswith("/digest")
        uri = url[: -len("/digest")].encode()
        body = b"\xf0\x08" + b"\x07" * 8 + b"\x08\x00" + ots_module.TAG_PENDING + bytes([len(uri) + 1, len(uri)]) + uri
        return HttpResult(200, "HTTP/1.1 200 OK", body)

    real_daemon = synthetic_rehearsal.Daemon

    class OtsDaemon(real_daemon):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, ots_calendars=["http://calendar.parity.example/cal"], **kwargs)

    with tempfile.TemporaryDirectory() as directory, \
            unittest.mock.patch.object(synthetic_rehearsal, "_send_with_acknowledgements", capture), \
            unittest.mock.patch.object(synthetic_rehearsal, "Daemon", OtsDaemon), \
            unittest.mock.patch.object(anchor_module, "http_fetch", side_effect=fake_tsa), \
            unittest.mock.patch.object(ots_module, "http_fetch", side_effect=fake_calendar), \
            contextlib.redirect_stdout(io.StringIO()):
        manifest_path = synthetic_rehearsal.run(Path(directory), time_anchor_url="http://tsa.parity.example/ts")
        manifest = json.loads(manifest_path.read_text())
    return {"manifest": manifest, "events": sent}


# ------------------------------------ OpenTimestamps ------------------------------------
OTS_DIR = HERE / "ots"
REAL_HEADER_358391 = bytes.fromhex(
    "02000000b96394585a281b7e5f438fd1c9ed492645a1fd61cb3802040000000000000000007ee445d23ad0"
    "61af4a36b809501fab1ac4f2d7e7a739817dd0cbb7ec661b8a1e376755f58616186272def6"
)


def _v(number: int) -> bytes:
    out = bytearray()
    while True:
        byte, number = number & 0x7F, number >> 7
        out.append(byte | (0x80 if number else 0))
        if not number:
            return bytes(out)


def _vb(data: bytes) -> bytes:
    return _v(len(data)) + data


def _att_pending(uri: str) -> bytes:
    return ots_module.TAG_PENDING + _vb(_vb(uri.encode()))


def _att_height(tag: bytes, height: int) -> bytes:
    return tag + _vb(_v(height))


def _leaf(attestation: bytes) -> bytes:
    return b"\x00" + attestation


def _detached(op: int, digest: bytes, body: bytes, *, version: int = 1) -> bytes:
    return ots_module.HEADER_MAGIC + bytes([version, op]) + digest + body


def _make_header(merkle_root: bytes, time_: int, *, mined: bool = True) -> bytes:
    """A CONSTRUCTED header. With mined=True the nonce is ground until the hash meets an
    easy (regtest-style) target, so it passes the proof-of-work check without real work."""
    bits = 0x207FFFFF if mined else 0x1D00FFFF
    for nonce in range(1_000_000):
        raw = (1).to_bytes(4, "little") + b"\x00" * 32 + merkle_root + time_.to_bytes(4, "little") \
            + bits.to_bytes(4, "little") + nonce.to_bytes(4, "little")
        if ots_module.BlockHeader(raw).meets_own_target() == mined:
            return raw
    raise SystemExit("could not construct a header")


def _parse_outcome(data: bytes) -> dict:
    try:
        proof = ots_module.parse_detached(data)
    except ots_module.DeserializationError as error:
        return {"error": str(error)}
    return {"ok": {
        "file_hash_op": ots_module._OP_NAMES[proof.file_hash_op],
        "digest": proof.file_digest.hex(),
        "attestations": ots_module.attestation_summary(proof.timestamp),
        "messages": sorted({m.hex() for m in proof.timestamp.messages()}),
        "reserialized_sha256": hashlib.sha256(proof.serialize()).hexdigest(),
        "reserialized_equals_input": proof.serialize() == data,
    }}


class TableSource:
    def __init__(self, kind: str, headers: dict[int, bytes]) -> None:
        self.kind = kind
        self.headers = {h: ots_module.BlockHeader(raw) for h, raw in headers.items()}

    def header_at(self, height: int):
        if height not in self.headers:
            raise LookupError(f"no header for height {height}")
        return self.headers[height]


def ots_fixture() -> dict:
    sources = json.loads((OTS_DIR / "SOURCES.json").read_text())
    real_names = [n for n in sources["files"] if n.endswith(".ots")]
    parse_vectors: list[dict] = [
        {"name": f"real:{name}", "origin": "real", "proof_hex": (OTS_DIR / name).read_bytes().hex(),
         **_parse_outcome((OTS_DIR / name).read_bytes())}
        for name in real_names
    ]

    digest = hashlib.sha256(b"ots parity constructed").digest()
    sha = lambda b: hashlib.sha256(b).digest()  # noqa: E731
    pend = _att_pending("https://calendar.example/a")
    btc = _att_height(ots_module.TAG_BITCOIN, 700_000)
    ltc = _att_height(ots_module.TAG_LITECOIN, 5)
    eth = _att_height(ots_module.TAG_ETHEREUM, 9)
    unknown = bytes.fromhex("0102030405060708") + _vb(b"opaque")
    sha256op = b"\x08"

    def with_ops(*pieces: bytes) -> bytes:
        return b"".join(pieces)

    constructed: list[tuple[str, bytes]] = [
        ("pending_leaf_only", _detached(0x08, digest, _leaf(pend))),
        ("bitcoin_leaf_only", _detached(0x08, digest, _leaf(btc))),
        ("append_sha256_pending", _detached(0x08, digest, b"\xf0" + _vb(b"\x01" * 8) + sha256op + _leaf(pend))),
        ("prepend_then_bitcoin", _detached(0x08, digest, b"\xf1" + _vb(b"pre") + _leaf(btc))),
        ("all_known_attestation_kinds_sorted", _detached(0x08, digest, b"\xff" + _leaf(unknown) + b"\xff" + _leaf(pend)
                                                          + b"\xff" + _leaf(eth) + b"\xff" + _leaf(ltc) + _leaf(btc))),
        ("non_canonical_order_is_normalised", _detached(0x08, digest, b"\xff" + _leaf(btc) + _leaf(pend))),
        ("duplicate_attestation_collapses", _detached(0x08, digest, b"\xff" + _leaf(pend) + _leaf(pend))),
        ("two_ops_branching", _detached(0x08, digest, b"\xff\xf0" + _vb(b"b") + _leaf(pend) + b"\xf0" + _vb(b"a") + _leaf(btc))),
        ("duplicate_op_last_wins", _detached(0x08, digest, b"\xff\xf0" + _vb(b"a") + _leaf(pend) + b"\xf0" + _vb(b"a") + _leaf(btc))),
        ("hexlify", _detached(0x08, digest, b"\xf3" + sha256op + _leaf(pend))),
        ("reverse", _detached(0x08, digest, b"\xf2" + _leaf(pend))),
        ("sha1_op", _detached(0x08, digest, b"\x02" + b"\xf3" + _leaf(pend))),
        ("ripemd160_op", _detached(0x08, digest, b"\x03" + b"\xf0" + _vb(b"x") + _leaf(pend))),
        ("sha1_file_hash", _detached(0x02, digest[:20], _leaf(pend))),
        ("ripemd160_file_hash", _detached(0x03, digest[:20], _leaf(pend))),
        ("unknown_attestation_payload_kept", _detached(0x08, digest, _leaf(unknown))),
        ("pending_uri_max_length", _detached(0x08, digest, _leaf(_att_pending("a" * 1000)))),
        ("pending_uri_too_long", _detached(0x08, digest, _leaf(_att_pending("a" * 1001)))),
        ("pending_uri_bad_char", _detached(0x08, digest, _leaf(_att_pending("https://x.example/?q=1")))),
        ("pending_payload_trailing_byte", _detached(0x08, digest, _leaf(ots_module.TAG_PENDING + _vb(_vb(b"ab") + b"\x00")))),
        ("bitcoin_payload_trailing_byte", _detached(0x08, digest, _leaf(ots_module.TAG_BITCOIN + _vb(_v(5) + b"\x00")))),
        ("attestation_payload_over_limit", _detached(0x08, digest, _leaf(bytes(8) + _vb(b"z" * 8193)))),
        ("attestation_payload_at_limit", _detached(0x08, digest, _leaf(bytes(8) + _vb(b"z" * 8192)))),
        ("varuint_63_bits_ok", _detached(0x08, digest, _leaf(_att_height(ots_module.TAG_BITCOIN, (1 << 63) - 1)))),
        ("varuint_64_bits_rejected", _detached(0x08, digest, _leaf(ots_module.TAG_BITCOIN + _vb(_v(1 << 63))))),
        ("varuint_unterminated", _detached(0x08, digest, _leaf(ots_module.TAG_BITCOIN + _vb(b"\x80" * 12)))),
        ("empty_append_arg", _detached(0x08, digest, b"\xf0" + _vb(b"") + _leaf(pend))),
        ("append_result_over_limit", _detached(0x08, digest, b"\xf0" + _vb(b"a" * 4096) + _leaf(pend))),
        ("append_result_at_limit", _detached(0x08, digest, b"\xf0" + _vb(b"a" * 4064) + _leaf(pend))),
        ("append_then_hexlify_message_over_limit", _detached(0x08, digest, b"\xf0" + _vb(b"a" * 4000) + b"\xf3" + _leaf(pend))),
        ("hexlify_at_limit", _detached(0x08, digest, b"\xf0" + _vb(b"a" * 2016) + b"\xf3" + _leaf(pend))),
        ("unknown_op_tag", _detached(0x08, digest, b"\xf4" + _leaf(pend))),
        ("keccak_op_rejected", _detached(0x08, digest, b"\x67" + _leaf(pend))),
        ("keccak_file_hash_rejected", _detached(0x67, digest, _leaf(pend))),
        ("unknown_file_hash_op", _detached(0xF0, digest, _leaf(pend))),
        ("bad_major_version", _detached(0x08, digest, _leaf(pend), version=2)),
        ("bad_magic", b"\x01" + ots_module.HEADER_MAGIC[1:] + b"\x01\x08" + digest + _leaf(pend)),
        ("truncated_digest", ots_module.HEADER_MAGIC + b"\x01\x08" + digest[:10]),
        ("truncated_after_op", _detached(0x08, digest, b"\xf0")),
        ("trailing_garbage", _detached(0x08, digest, _leaf(pend) + b"\x00")),
        ("empty_input", b""),
        ("header_only", ots_module.HEADER_MAGIC),
        ("nesting_255_ok", _detached(0x08, digest, b"\xf0\x01a" * 255 + _leaf(pend))),
        ("nesting_256_rejected", _detached(0x08, digest, b"\xf0\x01a" * 256 + _leaf(pend))),
    ]
    parse_vectors += [
        {"name": f"constructed:{name}", "origin": "constructed", "proof_hex": raw.hex(), **_parse_outcome(raw)}
        for name, raw in constructed
    ]

    # ----- header vectors -----
    header_vectors = [{
        "name": "real:358391", "origin": "real", "header_hex": REAL_HEADER_358391.hex(),
        "block_hash": ots_module.BlockHeader(REAL_HEADER_358391).block_hash().hex(),
        "merkle_root": ots_module.BlockHeader(REAL_HEADER_358391).merkle_root.hex(),
        "time": ots_module.BlockHeader(REAL_HEADER_358391).time,
        "meets_own_target": ots_module.BlockHeader(REAL_HEADER_358391).meets_own_target(),
    }]
    for name, raw in (("constructed:mined", _make_header(sha(b"m"), 1_700_000_000)),
                      ("constructed:unmined", _make_header(sha(b"m"), 1_700_000_000, mined=False))):
        header = ots_module.BlockHeader(raw)
        header_vectors.append({
            "name": name, "origin": "constructed", "header_hex": raw.hex(), "block_hash": header.block_hash().hex(),
            "merkle_root": header.merkle_root.hex(), "time": header.time, "meets_own_target": header.meets_own_target(),
        })
    for bits in (0x00000000, 0x03800001, 0x20000000 | 0x00800000, 0x1D00FFFF, 0x2100FFFF, 0x01003456, 0x03123456):
        raw = bytes(72) + bits.to_bytes(4, "little") + bytes(4)
        header_vectors.append({"name": f"bits:{bits:08x}", "origin": "constructed", "header_hex": raw.hex(),
                               "meets_own_target": ots_module.BlockHeader(raw).meets_own_target()})

    # ----- calendar/service records (Python service against a patched transport) -----
    fixed_nonce = bytes(range(16))
    file_hash = hashlib.sha256(b"ots parity export").hexdigest()
    commitment = sha(bytes.fromhex(file_hash) + fixed_nonce)

    def reply(*attestations: bytes, ops: bytes = b"") -> bytes:
        body = ops + (b"".join(b"\xff" + _leaf(a) for a in attestations[:-1]) + _leaf(attestations[-1]))
        return body

    r_a = reply(_att_pending("https://a.example"), ops=b"\xf0" + _vb(b"\x11" * 8) + sha256op)
    r_b = reply(_att_pending("https://b.example/cal"), ops=b"\xf1" + _vb(b"\x22" * 4) + sha256op)
    cases = [
        ("one_calendar", ["https://a.example"], {"https://a.example": (200, r_a)}),
        ("two_calendars_merge", ["https://a.example", "https://b.example/cal/"],
         {"https://a.example": (200, r_a), "https://b.example/cal/": (200, r_b)}),
        ("one_fails_one_succeeds", ["https://a.example", "https://b.example/cal"],
         {"https://a.example": (500, b""), "https://b.example/cal": (200, r_b)}),
        ("all_fail", ["https://a.example", "https://b.example/cal"],
         {"https://a.example": (500, b""), "https://b.example/cal": (404, b"")}),
        ("malformed_reply", ["https://a.example"], {"https://a.example": (200, b"\xf0")}),
        ("oversized_reply", ["https://a.example"], {"https://a.example": (200, b"\x00" * 10_001)}),
        ("bad_uri_in_reply", ["https://a.example"], {"https://a.example": (200, reply(_att_pending("https://a.example/?x")))}),
        ("duplicate_attestation_across_calendars", ["https://a.example", "https://a2.example"],
         {"https://a.example": (200, r_a), "https://a2.example": (200, r_a)}),
    ]
    record_vectors = []
    for name, calendars, replies in cases:
        def fake_fetch(url, *, method="GET", data=None, **_kw):
            base = url[: -len("/digest")]
            status, body = next(v for k, v in replies.items() if k.rstrip("/") == base)
            assert data == commitment, "the calendar must receive the nonce-hashed commitment"
            return HttpResult(status, f"HTTP/1.1 {status} X", body[: 10_001])

        service = ots_module.OtsAnchorService(calendars)
        with unittest.mock.patch.object(ots_module, "http_fetch", side_effect=fake_fetch), \
                unittest.mock.patch.object(ots_module.secrets, "token_bytes", return_value=fixed_nonce):
            record = service.anchor_record(file_hash)
        record_vectors.append({
            "name": name, "calendars": calendars, "data_hash": file_hash, "nonce_hex": fixed_nonce.hex(),
            "replies": {url: {"status": status, "body_hex": body[:10_001].hex()} for url, (status, body) in replies.items()},
            "expected": record,
        })
    for name, bad_hash in (("data_hash_short", "ab" * 31), ("data_hash_not_hex", "zz" * 32)):
        record_vectors.append({
            "name": name, "calendars": ["https://a.example"], "data_hash": bad_hash, "nonce_hex": fixed_nonce.hex(),
            "replies": {}, "expected": ots_module.OtsAnchorService(["https://a.example"]).anchor_record(bad_hash),
        })

    # ----- evaluate vectors -----
    hello = (OTS_DIR / "hello-world.txt.ots").read_bytes()
    incomplete = (OTS_DIR / "incomplete.txt.ots").read_bytes()

    def record_from(proof_bytes: bytes, *, commitment_index: int = 1, **overrides: object) -> dict:
        proof = ots_module.parse_detached(proof_bytes)
        node = sorted({m.hex() for m in proof.timestamp.messages()})[commitment_index]
        record = {
            "status": "pending", "data_hash": proof.file_digest.hex(), "file_hash_op": "sha256",
            "commitment_hex": node, "calendars": [{"url": "https://a.example", "status": "submitted"}],
            "attestations": ots_module.attestation_summary(proof.timestamp), "proof_hex": proof_bytes.hex(),
            "scope": "s", "apw:proof_level": "unknown_unobserved",
        }
        for key, value in overrides.items():
            key = "apw:proof_level" if key == "proof_level" else key
            if value is ...:
                record.pop(key, None)
            else:
                record[key] = value
        return record

    def manifest_of(record: object, export_hash: object) -> dict:
        return {"export": {"sha256": export_hash}, "time_anchor_opentimestamps": record}

    real_record = record_from(hello)
    real_hash = real_record["data_hash"]
    pending_record = record_from(incomplete)

    # A constructed proof from a pending record to a Bitcoin attestation over a constructed header.
    export_digest = hashlib.sha256(b"ots evaluate export").digest()
    nonce = bytes(range(16, 32))
    base = ots_module.Timestamp(export_digest)
    commit = base.add_op((ots_module.OP_APPEND, nonce)).add_op((ots_module.OP_SHA256, None))
    pending_stamp = ots_module.Timestamp(export_digest)
    pending_stamp.merge(base)
    commit_pending = pending_stamp.add_op((ots_module.OP_APPEND, nonce)).add_op((ots_module.OP_SHA256, None))
    commit_pending.attestations.add(ots_module.pending("https://a.example"))
    upgraded_stamp = ots_module.Timestamp(export_digest)
    upgraded_commit = upgraded_stamp.add_op((ots_module.OP_APPEND, nonce)).add_op((ots_module.OP_SHA256, None))
    tip = upgraded_commit.add_op((ots_module.OP_PREPEND, b"\x01\x02")).add_op((ots_module.OP_SHA256, None))
    tip.attestations.add(ots_module.Attestation("bitcoin", ots_module.TAG_BITCOIN, height=800_000))
    good_header = _make_header(tip.msg, 1_750_000_000)
    pending_bytes = ots_module.DetachedTimestampFile(ots_module.OP_SHA256, pending_stamp).serialize()
    upgraded_bytes = ots_module.DetachedTimestampFile(ots_module.OP_SHA256, upgraded_stamp).serialize()
    constructed_record = record_from(pending_bytes, commitment_index=0,
                                     commitment_hex=commit.msg.hex())
    other_digest = hashlib.sha256(b"other").digest()
    other_stamp = ots_module.Timestamp(other_digest)
    other_stamp.add_op((ots_module.OP_APPEND, nonce)).add_op((ots_module.OP_SHA256, None)).attestations.add(
        ots_module.Attestation("bitcoin", ots_module.TAG_BITCOIN, height=1))
    unrelated_stamp = ots_module.Timestamp(export_digest)
    unrelated_stamp.add_op((ots_module.OP_APPEND, b"different nonce")).attestations.add(
        ots_module.Attestation("bitcoin", ots_module.TAG_BITCOIN, height=1))

    def src(kind: str, headers: dict[int, bytes]) -> dict:
        return {"kind": kind, "headers": {str(h): raw.hex() for h, raw in headers.items()}}

    evaluate_cases: list[tuple[str, dict, dict | None, bytes | None]] = [
        ("absent", {"export": {"sha256": "aa" * 32}}, None, None),
        ("null", {"export": {"sha256": "aa" * 32}, "time_anchor_opentimestamps": None}, None, None),
        ("not_an_object", manifest_of([], real_hash), None, None),
        ("unavailable", manifest_of({"status": "unavailable", "data_hash": real_hash}, real_hash), None, None),
        ("status_unknown", manifest_of({**real_record, "status": "anchored"}, real_hash), None, None),
        ("status_missing", manifest_of({k: v for k, v in real_record.items() if k != "status"}, real_hash), None, None),
        ("proof_level_too_strong", manifest_of(record_from(hello, proof_level="directly_observed"), real_hash), None, None),
        ("proof_level_inferred_allowed", manifest_of(record_from(incomplete, proof_level="inferred"), pending_record["data_hash"]), None, None),
        ("data_hash_differs", manifest_of(real_record, "00" * 32), None, None),
        ("export_missing", {"time_anchor_opentimestamps": real_record}, None, None),
        ("missing_proof", manifest_of(record_from(hello, proof_hex=...), real_hash), None, None),
        ("file_hash_op_wrong", manifest_of(record_from(hello, file_hash_op="sha1"), real_hash), None, None),
        ("proof_not_hex", manifest_of(record_from(hello, proof_hex="zz"), real_hash), None, None),
        ("proof_does_not_parse", manifest_of(record_from(hello, proof_hex=hello[:-3].hex()), real_hash), None, None),
        ("proof_for_other_digest", manifest_of(record_from(hello, data_hash="11" * 32), "11" * 32), None, None),
        ("commitment_not_in_proof", manifest_of(record_from(hello, commitment_hex="22" * 32), real_hash), None, None),
        ("commitment_not_hex", manifest_of(record_from(hello, commitment_hex="xyz"), real_hash), None, None),
        ("attestation_summary_mismatch", manifest_of(record_from(hello, attestations=[]), real_hash), None, None),
        ("real_pending_proof", manifest_of(pending_record, pending_record["data_hash"]), None, None),
        ("real_bitcoin_no_header_source", manifest_of(real_record, real_hash), None, None),
        ("real_bitcoin_local_header_verified", manifest_of(real_record, real_hash),
         src("local_header", {358391: REAL_HEADER_358391}), None),
        ("real_bitcoin_explorer_header_verified", manifest_of(real_record, real_hash),
         src("explorer", {358391: REAL_HEADER_358391}), None),
        ("real_bitcoin_header_for_another_block", manifest_of(real_record, real_hash),
         src("local_header", {358391: good_header}), None),
        ("real_bitcoin_header_missing", manifest_of(real_record, real_hash), src("explorer", {}), None),
        ("real_bitcoin_header_fails_own_pow", manifest_of(real_record, real_hash),
         src("local_header", {358391: REAL_HEADER_358391[:76] + bytes(4)}), None),
        ("constructed_pending", manifest_of(constructed_record, export_digest.hex()), None, None),
        ("constructed_override_verified_local", manifest_of(constructed_record, export_digest.hex()),
         src("local_header", {800_000: good_header}), upgraded_bytes),
        ("constructed_override_verified_explorer", manifest_of(constructed_record, export_digest.hex()),
         src("explorer", {800_000: good_header}), upgraded_bytes),
        ("constructed_override_no_source", manifest_of(constructed_record, export_digest.hex()), None, upgraded_bytes),
        ("constructed_override_wrong_header", manifest_of(constructed_record, export_digest.hex()),
         src("local_header", {800_000: _make_header(sha(b"not it"), 1_750_000_000)}), upgraded_bytes),
        ("override_is_pending_only", manifest_of(constructed_record, export_digest.hex()), None, pending_bytes),
        ("override_for_other_digest", manifest_of(constructed_record, export_digest.hex()), None,
         ots_module.DetachedTimestampFile(ots_module.OP_SHA256, other_stamp).serialize()),
        ("override_without_commitment", manifest_of(constructed_record, export_digest.hex()), None,
         ots_module.DetachedTimestampFile(ots_module.OP_SHA256, unrelated_stamp).serialize()),
        ("override_unparsable", manifest_of(constructed_record, export_digest.hex()), None, b"not a proof"),
    ]
    evaluate_vectors = []
    for name, manifest, source, override in evaluate_cases:
        header_source = TableSource(source["kind"], {int(h): bytes.fromhex(x) for h, x in source["headers"].items()}) if source else None
        findings = ots_module.evaluate_record(manifest, header_source, override)
        evaluate_vectors.append({
            "name": name, "manifest": manifest, "header_source": source,
            "override_hex": override.hex() if override is not None else None,
            "expected_findings": [{"severity": sev, "code": code, "message": msg} for sev, code, msg in findings],
        })
    return {
        "sources": "tests/fixtures/parity/ots/SOURCES.json",
        "parse": parse_vectors, "headers": header_vectors, "records": record_vectors,
        "evaluate": evaluate_vectors,
    }


def main() -> None:
    dump("forgery_analysis.json", forgery_cases())
    dump("time_anchor.json", time_anchor_fixture())
    capture = rehearsal_capture()
    dump("manifest_rehearsal.json", rehearsal_fixture(capture["manifest"]))
    dump("synthetic_events.json", capture["events"])
    dump("manifest_key_paths.json", {"note": RUN_SPECIFIC_NOTE, "paths": key_paths(capture["manifest"])})
    dump("session_state.json", session_cases())
    dump("network_validation.json", network_validation_cases())
    dump("ots_vectors.json", ots_fixture())


if __name__ == "__main__":
    main()
