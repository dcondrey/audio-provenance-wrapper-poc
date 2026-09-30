"""Shared Python/Rust corpus for the `.xm`, `.mod` and `.vcv` parsers.

`build_cases()` lists inputs (a committed fixture plus byte mutations, or inline
bytes) and the lowered limits to run them under. `evaluate()` runs one through
the Python oracle and returns its verdict: the sha256 of the snapshot's canonical
JSON, or the error message. The verdicts are committed in
tests/fixtures/parity/project_modules_corpus.json; the Rust port must reproduce
every one (`rust/apw-daemon/tests/project_modules_parity.rs`) and the Python
tests re-check them, so an edit to either parser that changes behaviour fails
until the corpus is regenerated on purpose.
"""

from __future__ import annotations

import hashlib
import json
import tempfile
from pathlib import Path
from unittest import mock

import module_builders as mb
import zstandard

HERE = Path(__file__).resolve().parent


def golden_digest(snapshot) -> str:
    from daemon.project_formats._snapshot import snapshot_to_golden

    text = json.dumps(snapshot_to_golden(snapshot), sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def apply_mutations(data: bytes, mutations: list[list]) -> bytes:
    for mutation in mutations:
        if mutation[0] == "truncate":
            data = data[: mutation[1]]
        elif mutation[0] == "set":
            patch = bytes.fromhex(mutation[2])
            data = data[: mutation[1]] + patch + data[mutation[1] + len(patch) :]
        elif mutation[0] == "append":
            data += bytes.fromhex(mutation[1])
        else:
            raise ValueError(mutation)
    return data


BASES: dict[str, str] = {}


def case_bytes(case: dict, bases: dict[str, str] | None = None) -> bytes:
    if "hex" in case:
        data = bytes.fromhex(case["hex"])
    elif "ref" in case:
        data = bytes.fromhex((BASES if bases is None else bases)[case["ref"]])
    else:
        data = (HERE / case["base"]).read_bytes()
    return apply_mutations(data, case.get("mut", []))


def evaluate(case: dict, bases: dict[str, str] | None = None) -> dict:
    """Run one case through the Python parsers under the case's limits."""
    from daemon.project_formats import _safe, parse_project, registry

    data = case_bytes(case, bases)
    limits = case.get("limits", {})
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / ("input" + case["ext"])
        path.write_bytes(data)
        with mock.patch.object(registry, "MAX_PROJECT_FILE_BYTES", limits.get("max_project_file_bytes", registry.MAX_PROJECT_FILE_BYTES)), \
                mock.patch.object(_safe, "MAX_TAR_MEMBERS", limits.get("max_tar_members", _safe.MAX_TAR_MEMBERS)), \
                mock.patch.object(_safe, "MAX_TAR_BYTES", limits.get("max_tar_bytes", _safe.MAX_TAR_BYTES)):
            try:
                return {"ok": golden_digest(parse_project(path))}
            except ValueError as error:
                message = str(error).replace(str(path), "<path>")
                # Library-specific JSON diagnostics differ between languages; the class is what is shared.
                if message.startswith("malformed JSON:"):
                    message = "malformed JSON:"
                return {"error": message}


def _hex(data: bytes) -> str:
    return data.hex()


def _inline(name: str, ext: str, data: bytes, **extra) -> dict:
    return {"name": name, "ext": ext, "hex": _hex(data), **extra}


def _mut(name: str, ext: str, base: str, *mutations: list, **extra) -> dict:
    return {"name": name, "ext": ext, "base": base, "mut": [list(m) for m in mutations], **extra}


# ------------------------------------------------------------------ XM / MOD


def _minimal_instrument(name: str = "i") -> dict:
    return {"name": name}


def xm_cases() -> list[dict]:
    out: list[dict] = []
    full = mb.xm_bytes(
        name="Full \xe9\x01 name  ",
        tracker="FastTracker v2.00  ",
        instruments=[{"name": "lead", "samples": [("saw", 10, 10), ("sq\x00hidden", 4, 4)]}, {"name": "empty"}, {"name": "drums", "samples": [("kick", 6, 6)]}],
        patterns=[(4, b"\x80" * 8), (16, b"")],
        orders=(0, 1, 0),
        bpm=140,
        ticks=3,
        flags=0,
    )
    out.append(_inline("xm_full", ".xm", full))
    BASES["xm_full"] = full.hex()
    for cut in range(len(full)):
        out.append({"name": f"xm_full_cut_{cut}", "ext": ".xm", "ref": "xm_full", "mut": [["truncate", cut]]})
    out.append(_inline("xm_minimal", ".xm", mb.xm_bytes()))
    out.append(_inline("xm_no_patterns_no_instruments", ".xm", mb.xm_bytes(patterns=[], orders=())))
    for channels in (0, 1, 2, 31, 32, 33, 255, 65535):
        out.append(_inline(f"xm_channels_{channels}", ".xm", mb.xm_bytes(channels=channels)))
    for count in (255, 256, 257):
        out.append(_inline(f"xm_patterns_{count}", ".xm", mb.xm_bytes(patterns=[(1, b"")] * count, orders=(0,))))
    for count in (254, 255, 256):
        out.append(_inline(f"xm_instruments_{count}", ".xm", mb.xm_bytes(instruments=[_minimal_instrument()] * count)))
    out.append(_inline("xm_instruments_declared_more_than_stored", ".xm", mb.xm_bytes(instruments=[_minimal_instrument()], instrument_count=5)))
    out.append(_inline("xm_instruments_declared_255_stored_0", ".xm", mb.xm_bytes(instruments=[], instrument_count=255)))
    for count in (95, 96, 97):
        out.append(_inline(f"xm_samples_per_instrument_{count}", ".xm", mb.xm_bytes(instruments=[{"name": "x", "samples": [("s", 1, 1)] * count}])))
    for rows in (0, 1, 255, 256, 257, 65535):
        out.append(_inline(f"xm_rows_{rows}", ".xm", mb.xm_bytes(patterns=[(rows, b"")])))
    for count in (255, 256, 257, 1000):
        out.append(_inline(f"xm_order_count_{count}", ".xm", mb.xm_bytes(orders=(0,), order_count=count)))
    for version in (0x0100, 0x0101, 0x0102, 0x0103, 0x0104, 0x0105, 0x0204):
        out.append(_inline(f"xm_version_{version:04x}", ".xm", mb.xm_bytes(version=version)))
    for size in (0, 3, 4, 5, 6, 20, 275, 276, 277, 300, 10_000):
        out.append(_inline(f"xm_header_size_{size}", ".xm", mb.xm_bytes(header_size=size)))
    for length in (0, 8, 9, 10, 20):
        out.append(_inline(f"xm_pattern_header_length_{length}", ".xm", mb.xm_bytes(patterns=[(2, b"\x80\x80")], pattern_header=length)))
    for size in (0, 28, 29, 30, 32, 33, 263, 300):
        instrument = {"name": "x", "samples": [("s", 2, 2)], "size": size}
        out.append(_inline(f"xm_instrument_size_{size}_with_sample", ".xm", mb.xm_bytes(instruments=[instrument])))
        out.append(_inline(f"xm_instrument_size_{size}_empty", ".xm", mb.xm_bytes(instruments=[{"name": "x", "size": size}])))
    for size in (0, 39, 40, 41, 60):
        out.append(_inline(f"xm_sample_header_{size}", ".xm", mb.xm_bytes(instruments=[{"name": "x", "samples": [("s", 3, 3)], "sample_header": size}])))
    out.append(_inline("xm_sample_data_short", ".xm", mb.xm_bytes(instruments=[{"name": "x", "samples": [("s", 100, 10)]}])))
    out.append(_inline("xm_sample_data_missing_then_next_instrument", ".xm", mb.xm_bytes(instruments=[{"name": "x", "samples": [("s", 100, 0)]}, {"name": "y"}])))
    out.append(_inline("xm_huge_declared_sample", ".xm", mb.xm_bytes(instruments=[{"name": "x", "samples": [("s", 0xFFFFFFFF, 2)]}])))
    out.append(_inline("xm_trailing_junk", ".xm", mb.xm_bytes() + b"junk"))
    out.append(_inline("xm_pattern_data_short", ".xm", mb.xm_bytes(patterns=[(1, b"\x80\x80\x80")])[:-2]))
    out.append(_inline("xm_id_wrong", ".xm", b"Extended Modulex" + mb.xm_bytes()[16:]))
    out.append(_inline("xm_id_matches_16_chars_only", ".xm", b"Extended Module:X" + mb.xm_bytes()[17:]))
    out.append(_inline("xm_content_in_mod_extension", ".mod", mb.xm_bytes()))
    for byte in (0x00, 0x1F, 0x20, 0x7F, 0x80, 0xFF):
        out.append(_inline(f"xm_name_byte_{byte:02x}", ".xm", mb.xm_bytes(name=bytes([65, byte, 66]).decode("latin-1"))))
    # Real file (OpenMPT test.xm, header size 22, zero-padded order table): every header offset poked, many cuts.
    base = "milkytracker/test.xm"
    size = (HERE / base).stat().st_size
    out.append(_mut("xm_real_unmodified", ".xm", base))
    for offset in range(0, 0x60):
        out.append(_mut(f"xm_real_poke_{offset}", ".xm", base, ["set", offset, "ff"]))
        out.append(_mut(f"xm_real_zero_{offset}", ".xm", base, ["set", offset, "00"]))
    for cut in sorted(set(list(range(0, 0x180, 3)) + list(range(0x180, size, 71)) + [size - 1, size])):
        out.append(_mut(f"xm_real_cut_{cut}", ".xm", base, ["truncate", cut]))
    out.append(_mut("xm_real_trailing", ".xm", base, ["append", "00"]))
    return out


def mod_cases() -> list[dict]:
    out: list[dict] = []
    slots = [("first", 4), ("second \xe9", 1), ("", 0), ("third", 3), ("only name", 0)]
    full = mb.mod_bytes(title="Title", slots=slots, order=(0, 1, 0), song_length=3, restart=0)
    out.append(_inline("mod_full", ".mod", full))
    BASES["mod_full"] = full.hex()
    for cut in range(len(full)):
        out.append({"name": f"mod_full_cut_{cut}", "ext": ".mod", "ref": "mod_full", "mut": [["truncate", cut]]})
    out.append(_inline("mod_minimal", ".mod", mb.mod_bytes()))
    tags = [b"M.K.", b"M!K!", b"FLT4", b"FLT8", b"OKTA", b"OCTA", b"FA08", b"CD81", b"1CHN", b"2CHN", b"9CHN", b"0CHN", b"10CH", b"32CH", b"99CH",
            b"32CN", b"00CH", b"01CH", b"0aCH", b"xxCH", b"\x00\x00\x00\x00", b"M.k.", b"FLT6", b"6CHN", b"8CHN", b"10CN", b"1CH ", b"1chn", b"11CX"]
    for tag in tags:
        channels = {b"FLT8": 8, b"OKTA": 8, b"OCTA": 8, b"FA08": 8, b"CD81": 8}.get(tag, 4)
        for label, value in (("tag", tag),):
            out.append(_inline(f"mod_tag_{tag.hex()}", ".mod", mb.mod_bytes(tag=value, channels=1 if tag[:1] in b"123456789" and tag[1:4] == b"CHN" else channels)))
    header_only = mb.mod_bytes()[:1084]
    for high in (0, 1, 127, 128, 255):
        data = bytearray(header_only)
        data[952] = high
        out.append(_inline(f"mod_max_order_{high}_no_pattern_data", ".mod", bytes(data)))
    for length in (0, 1, 127, 128, 129, 255):
        data = bytearray(header_only)
        data[950] = length
        out.append(_inline(f"mod_song_length_{length}", ".mod", bytes(data)))
    for words in (0, 1, 2, 3, 0xFFFF):
        out.append(_inline(f"mod_slot_words_{words}", ".mod", mb.mod_bytes(slots=[("s", min(words, 8))], sample_bytes=None) if words <= 8 else header_only[:20] + b"s".ljust(22, b"\x00") + words.to_bytes(2, "big") + header_only[44:]))
    out.append(_inline("mod_short_1083", ".mod", header_only[:1083]))
    out.append(_inline("mod_exact_1084", ".mod", header_only))
    out.append(_inline("mod_pattern_data_short", ".mod", mb.mod_bytes(order=(0, 1))[:1084 + 1024 + 10]))
    out.append(_inline("mod_sample_data_short", ".mod", mb.mod_bytes(slots=[("s", 50)], sample_bytes=10)))
    out.append(_inline("mod_trailing_junk", ".mod", mb.mod_bytes(slots=[("s", 4)]) + b"junk"))
    out.append(_inline("mod_content_in_xm_extension", ".xm", mb.mod_bytes()))
    for byte in (0x00, 0x20, 0x7F, 0x80, 0xFF):
        out.append(_inline(f"mod_title_byte_{byte:02x}", ".mod", mb.mod_bytes(title=bytes([65, byte, 66]).decode("latin-1"))))
    base = "milkytracker/test.mod"
    size = (HERE / base).stat().st_size
    out.append(_mut("mod_real_unmodified", ".mod", base))
    for offset in list(range(0, 44)) + list(range(940, 960)) + list(range(1076, 1090)):
        out.append(_mut(f"mod_real_poke_{offset}", ".mod", base, ["set", offset, "ff"]))
        out.append(_mut(f"mod_real_zero_{offset}", ".mod", base, ["set", offset, "00"]))
    for cut in sorted(set(list(range(0, 60)) + list(range(1070, 1100)) + list(range(1100, size, 89)) + [size - 1, size])):
        out.append(_mut(f"mod_real_cut_{cut}", ".mod", base, ["truncate", cut]))
    return out


# ------------------------------------------------------------------ VCV


def _patch_with(**overrides) -> bytes:
    patch = mb.constructed_patch()
    patch.update(overrides)
    return mb.patch_json_bytes(patch)


def _vcv(patch_json: bytes, **kwargs) -> bytes:
    return mb.zstd_stream(mb.rack_tar(patch_json, **kwargs), level=1)


def _nested(depth_inside_data: int) -> bytes:
    data: object = 1
    for _ in range(depth_inside_data):
        data = [data]
    patch = {"version": "2.6.6", "modules": [{"id": 1, "plugin": "a", "model": "b", "data": data}], "cables": []}
    return json.dumps(patch).encode()


def vcv_json_cases() -> list[dict]:
    out: list[dict] = []
    raw = lambda name, text, **extra: out.append(_inline(name, ".vcv", text if isinstance(text, bytes) else text.encode("utf-8"), **extra))  # noqa: E731
    raw("json_legacy_minimal", '{"version": "1.1.6", "modules": [], "cables": []}')
    raw("json_legacy_wires", '{"version": "0.6.2", "modules": [{"plugin": "Fundamental", "model": "VCO"}], "wires": [{"outputModuleId": 0, "outputId": 0, "inputModuleId": 1, "inputId": 1}]}')
    raw("json_cables_null_blocks_wires", '{"version": "1", "cables": null, "wires": [{"outputModuleId": 0}]}')
    raw("json_cables_present_not_list", '{"version": "1", "cables": 5, "wires": [{"outputModuleId": 0}]}')
    raw("json_version_missing", '{"modules": []}')
    raw("json_version_number", '{"version": 2, "modules": []}')
    raw("json_top_level_list", "[1, 2]")
    raw("json_top_level_string", '"x"')
    raw("json_empty", "")
    raw("json_three_bytes", "{  ")
    raw("json_bom", b"\xef\xbb\xbf" + b'{"version": "2.6.6"}')
    raw("json_invalid_utf8", b'{"version": "\xff"}')
    raw("json_trailing_garbage", '{"version": "2"} x')
    raw("json_modules_not_list", '{"version": "2", "modules": {"a": 1}}')
    raw("json_module_entries_mixed", '{"version": "2", "modules": [1, "x", null, [], {"plugin": "p", "model": "m"}, {"id": 4}]}')
    for label, ident in (("int", "7"), ("negative", "-7"), ("zero", "0"), ("neg_zero", "-0"), ("float", "7.0"), ("exp", "1e2"), ("true", "true"),
                         ("string", '"7"'), ("null", "null"), ("i64_max", "9223372036854775807"), ("i64_over", "9223372036854775808"),
                         ("i64_min", "-9223372036854775808"), ("i64_min_under", "-9223372036854775809"), ("huge", "1" + "0" * 400)):
        raw(f"json_module_id_{label}", '{"version": "2", "modules": [{"id": %s, "plugin": "p", "model": "m", "params": [{"id": %s, "value": 0.5}]}], "cables": [{"outputModuleId": %s, "outputId": %s, "inputModuleId": %s, "inputId": 0}]}' % (ident, ident, ident, ident, ident))
    for label, fields in (
        ("plugin_number", '"plugin": 5, "model": "m"'),
        ("model_missing", '"plugin": "p"'),
        ("version_number", '"plugin": "p", "model": "m", "version": 3'),
        ("params_object", '"plugin": "p", "model": "m", "params": {"a": 1}'),
        ("params_three", '"plugin": "p", "model": "m", "params": [1, 2, 3]'),
        ("bypass_true", '"plugin": "p", "model": "m", "bypass": true'),
        ("bypass_string", '"plugin": "p", "model": "m", "bypass": "true"'),
        ("disabled_true", '"plugin": "p", "model": "m", "disabled": true'),
        ("bypass_false_disabled_true", '"plugin": "p", "model": "m", "bypass": false, "disabled": true'),
        ("data_null", '"plugin": "p", "model": "m", "data": null'),
        ("data_empty_object", '"plugin": "p", "model": "m", "data": {}'),
        ("data_zero", '"plugin": "p", "model": "m", "data": 0'),
        ("data_unicode", '"plugin": "p\\u00e9\\ud83d\\ude00", "model": "m\\n", "data": {"k\\u0000": "v"}'),
        ("floats", '"plugin": "p", "model": "m", "params": [{"value": 1.10, "id": 0}, {"value": 1e-7, "id": 1}, {"value": 123456789012345678901234567890, "id": 2}, {"value": -0.0, "id": 3}, {"value": 1E5, "id": 4}, {"value": 0.1, "id": 5}, {"value": 1e22, "id": 6}, {"value": 5e-324, "id": 7}]'),
        ("nan", '"plugin": "p", "model": "m", "data": NaN'),
        ("infinity", '"plugin": "p", "model": "m", "data": Infinity'),
        ("neg_infinity", '"plugin": "p", "model": "m", "data": -Infinity'),
        ("float_overflow", '"plugin": "p", "model": "m", "data": 1e999'),
        ("float_underflow", '"plugin": "p", "model": "m", "data": 1e-999'),
        ("duplicate_keys", '"plugin": "p", "plugin": "q", "model": "m"'),
        ("sorted_keys_irrelevant", '"model": "m", "plugin": "p", "params": [{"value": 1, "id": 0}]'),
    ):
        raw(f"json_module_{label}", '{"version": "2.6.6", "modules": [{%s}], "cables": []}' % fields)
    raw("json_cable_shapes", '{"version": "2", "cables": [{"outputModuleId": 1.5, "outputId": "x", "inputModuleId": null, "inputId": true}, {}, 7, {"outputModuleId": 3, "outputId": 4, "inputModuleId": 5, "inputId": 6, "color": "#fff", "id": 9}]}')
    for depth in (124, 125, 126, 127, 128, 129, 130):
        out.append(_inline(f"json_depth_total_{depth + 3}", ".vcv", _nested(depth)))
    big_ints = '{"version": "2", "modules": [{"plugin": "p", "model": "m", "data": %s}]}'
    raw("json_int_4300_digits", big_ints % ("1" * 4300))
    raw("json_int_4301_digits", big_ints % ("1" * 4301))
    raw("json_neg_int_4300_digits", big_ints % ("-" + "1" * 4300))
    raw("json_neg_int_4301_digits", big_ints % ("-" + "1" * 4301))
    raw("json_zstd_magic_prefix_invalid", b"\x28\xb5\x2f\xfd{}")
    raw("json_skippable_frame_magic", b"\x50\x2a\x4d\x18\x00\x00\x00\x00")
    return out


def vcv_container_cases() -> list[dict]:
    out: list[dict] = []
    patch = mb.patch_json_bytes(mb.constructed_patch())
    fixture = mb.constructed_vcv()
    out.append(_mut("vcv_fixture", ".vcv", "vcv/basic.vcv"))
    for cut in range(len(fixture)):
        out.append(_mut(f"vcv_fixture_cut_{cut}", ".vcv", "vcv/basic.vcv", ["truncate", cut]))
    for offset in range(len(fixture)):
        out.append(_mut(f"vcv_fixture_flip_{offset}", ".vcv", "vcv/basic.vcv", ["set", offset, f"{fixture[offset] ^ 0x55:02x}"]))
    out.append(_mut("vcv_fixture_trailing_zero", ".vcv", "vcv/basic.vcv", ["append", "00"]))
    out.append(_mut("vcv_fixture_two_frames", ".vcv", "vcv/basic.vcv", ["append", fixture.hex()]))

    tar = mb.rack_tar(patch)

    def z(data: bytes, name: str, **extra) -> None:
        out.append(_inline(name, ".vcv", data, **extra))

    # ---- Zstandard framing
    z(mb.zstd_stream(tar, content_size=True), "zstd_sized_frame")
    z(mb.zstd_stream(tar, checksum=True), "zstd_checksum_frame")
    good = bytearray(mb.zstd_stream(tar, checksum=True))
    good[-1] ^= 1
    z(bytes(good), "zstd_bad_checksum")
    z(mb.zstd_stream(tar, level=19), "zstd_level_19")
    z(mb.zstd_raw_frame(tar), "zstd_raw_blocks_single_segment")
    z(mb.zstd_raw_frame(tar, single_segment=False, window_descriptor=0), "zstd_raw_block_bigger_than_1k_window")
    z(mb.zstd_raw_frame(tar, single_segment=False, window_descriptor=0, block_size=1024), "zstd_raw_blocks_window_1k")
    z(mb.zstd_raw_frame(tar, single_segment=False, window_descriptor=0, block_size=1025), "zstd_raw_blocks_window_1k_block_plus_1")
    z(mb.zstd_raw_frame(tar, block_size=100), "zstd_raw_many_small_blocks")
    z(mb.zstd_raw_frame(tar, single_segment=False, window_descriptor=(17 << 3)), "zstd_window_2_27")
    z(mb.zstd_raw_frame(tar, single_segment=False, window_descriptor=(17 << 3) | 1), "zstd_window_2_27_plus")
    z(mb.zstd_raw_frame(tar, single_segment=False, window_descriptor=(18 << 3)), "zstd_window_2_28")
    z(mb.zstd_raw_frame(tar, single_segment=False, window_descriptor=(31 << 3) | 7), "zstd_window_max")
    z(mb.zstd_raw_frame(tar, descriptor_or=0x01), "zstd_dictionary_flag_1")
    z(mb.zstd_raw_frame(tar, descriptor_or=0x03), "zstd_dictionary_flag_3")
    z(mb.zstd_raw_frame(tar, descriptor_or=0x08), "zstd_reserved_bit")
    z(mb.zstd_raw_frame(tar, descriptor_or=0x10), "zstd_unused_bit")
    z(mb.zstd_raw_frame(tar, declared_size=len(tar) + 1), "zstd_declared_size_larger")
    z(mb.zstd_raw_frame(tar, declared_size=len(tar) - 1), "zstd_declared_size_smaller")
    z(mb.zstd_raw_frame(tar, declared_size=(1 << 40)), "zstd_declared_size_huge")
    z(mb.zstd_raw_frame(tar, checksum_flag=True), "zstd_zero_checksum_wrong")
    z(mb.zstd_raw_frame(b""), "zstd_empty_content")
    z(mb.zstd_raw_frame(tar, block_size=(1 << 17)), "zstd_block_max_128k")
    huge = bytearray(mb.zstd_raw_frame(tar))
    z(bytes(huge[:-len(tar)]) + b"", "zstd_blocks_missing_payload")
    z(mb.zstd_stream(tar) + b"\x00", "zstd_trailing_zero")
    z(mb.zstd_stream(tar) + mb.zstd_stream(tar), "zstd_two_frames")
    z(mb.zstd_stream(b"") , "zstd_frame_of_nothing")
    z(mb.zstd_stream(b"\x00" * 1024), "zstd_frame_of_zero_block")
    # An RLE block: header, then 1 byte; regenerated size 512 of zeros (an empty tar) -> no patch.json.
    rle = zstandard.FRAME_HEADER + bytes([0x60, 0x00, 0x01]) + ((512 << 3) | (1 << 1) | 1).to_bytes(3, "little") + b"\x00"
    z(rle, "zstd_rle_block_zero_tar")
    rle_reserved = zstandard.FRAME_HEADER + bytes([0x20, 0x00]) + ((4 << 3) | (3 << 1) | 1).to_bytes(3, "little") + b"\x00"
    z(rle_reserved, "zstd_reserved_block_type")
    too_big = zstandard.FRAME_HEADER + bytes([0x00, 0x00]) + ((2000 << 3) | 1).to_bytes(3, "little") + b"\x00" * 2000
    z(too_big, "zstd_block_bigger_than_window")
    z(zstandard.FRAME_HEADER, "zstd_magic_only")
    z(zstandard.FRAME_HEADER + b"\x00", "zstd_five_bytes")
    z(zstandard.FRAME_HEADER + b"\x00\x00", "zstd_six_bytes")
    z(zstandard.FRAME_HEADER + bytes([0x20]) + b"\x00" + b"\x00\x00", "zstd_no_last_block")
    # Content large enough for several compressed blocks.
    modules = [{"id": i, "plugin": "Fundamental", "model": "VCO", "version": "2.6.4", "params": [{"id": j, "value": (i * 31 + j) % 17 / 16} for j in range(6)]} for i in range(6000)]
    big = mb.patch_json_bytes({"version": "2.6.6", "modules": modules, "cables": []})
    z(_vcv(big), "vcv_many_modules_multi_block")

    # ---- cap boundaries (limits lowered per case)
    n = len(tar)
    z(mb.zstd_stream(tar), "cap_tar_bytes_exact", limits={"max_tar_bytes": n})
    z(mb.zstd_stream(tar), "cap_tar_bytes_minus_1", limits={"max_tar_bytes": n - 1})
    z(mb.zstd_stream(tar), "cap_tar_bytes_plus_1", limits={"max_tar_bytes": n + 1})
    z(mb.zstd_stream(tar, content_size=True), "cap_tar_bytes_sized_exact", limits={"max_tar_bytes": n})
    z(mb.zstd_stream(tar, content_size=True), "cap_tar_bytes_sized_minus_1", limits={"max_tar_bytes": n - 1})
    z(mb.zstd_raw_frame(tar), "cap_tar_bytes_raw_exact", limits={"max_tar_bytes": n})
    z(mb.zstd_raw_frame(tar), "cap_tar_bytes_raw_minus_1", limits={"max_tar_bytes": n - 1})
    for count in (2, 3, 4, 5):
        entries = [mb.tar_entry(".", kind=b"5"), mb.tar_entry("./patch.json", patch)]
        entries += [mb.tar_entry(f"./a{i}", b"x") for i in range(count - 2)] if count > 2 else []
        z(mb.zstd_stream(mb.tar_bytes(entries[:count])), f"cap_members_{count}_of_3", limits={"max_tar_members": 3})
    # A pax header is not a member; a directory is.
    pax = mb.tar_entry("PaxHeader", mb.pax_record(b"path", b"./patch.json"), kind=b"x") + mb.tar_entry("ignored", patch)
    z(mb.zstd_stream(mb.tar_bytes([pax])), "cap_members_pax_counts_once", limits={"max_tar_members": 1})
    z(mb.zstd_stream(mb.tar_bytes([pax])), "cap_members_pax_counts_once_zero", limits={"max_tar_members": 0})
    plain = json.dumps({"version": "2"}).encode()
    for delta in (-1, 0, 1):
        z(plain, f"cap_file_size_{delta:+d}", limits={"max_project_file_bytes": len(plain) + delta})
    return out


def vcv_tar_cases() -> list[dict]:
    out: list[dict] = []
    patch = mb.patch_json_bytes(mb.constructed_patch())

    def t(name: str, entries: list[bytes], trailer: bool = True, **extra) -> None:
        out.append(_inline(name, ".vcv", mb.zstd_stream(mb.tar_bytes(entries, trailer=trailer)), **extra))

    def raw_tar(name: str, tar: bytes) -> None:
        out.append(_inline(name, ".vcv", mb.zstd_stream(tar)))

    good = mb.tar_entry("./patch.json", patch)
    t("tar_plain_names", [mb.tar_entry("patch.json", patch)])
    t("tar_dot_slash_dirs", [mb.tar_entry(".", kind=b"5"), mb.tar_entry("./modules/", kind=b"5"), good])
    t("tar_no_trailer", [good], trailer=False)
    raw_tar("tar_one_zero_block", good + b"\x00" * 512)
    raw_tar("tar_trailer_then_junk", good + b"\x00" * 1024 + b"junk")
    raw_tar("tar_empty", b"")
    raw_tar("tar_only_zero_blocks", b"\x00" * 1024)
    raw_tar("tar_partial_header", good + b"\x01" * 100)
    t("tar_no_patch_json", [mb.tar_entry("./other.json", patch)])
    t("tar_patch_json_in_subdir", [mb.tar_entry("./sub/patch.json", patch)])
    t("tar_patch_json_empty", [mb.tar_entry("./patch.json", b"")])
    t("tar_duplicate_after_normalisation", [mb.tar_entry("./patch.json", patch), mb.tar_entry("patch.json", patch)])
    t("tar_duplicate_dot_segments", [mb.tar_entry("./a/./b", b"1"), mb.tar_entry("a//b", b"2"), good])
    t("tar_duplicate_directories_allowed", [mb.tar_entry("./d", kind=b"5"), mb.tar_entry("d/", kind=b"5"), good])
    t("tar_backslash_duplicate", [mb.tar_entry("a\\b", b"1"), mb.tar_entry("a/b", b"2"), good])
    t("tar_second_patch_json_in_subdir_ok", [good, mb.tar_entry("./m/patch.json", b"x")])
    for kind, label in ((b"1", "hardlink"), (b"2", "symlink"), (b"3", "chardev"), (b"4", "blockdev"), (b"6", "fifo"), (b"7", "contiguous"),
                        (b"g", "pax_global"), (b"L", "gnu_longname"), (b"K", "gnu_longlink"), (b"S", "sparse"), (b"D", "gnu_dumpdir"), (b"A", "letter_a"), (b"9", "digit_9")):
        t(f"tar_type_{label}", [mb.tar_entry("./link", b"", kind=kind), good])
    for name in ("/patch.json", "../patch.json", "a/../patch.json", "a/..", "..", "C:/x", "c:x", "a\\..\\b", "\\abs", "./../x", "a/b/../../../c", ".../x", "..a", "a..", "a/..b/c", "x\u00e9/patch.json", "./\u00e9"):
        t(f"tar_name_{name.encode('unicode_escape').decode()}", [mb.tar_entry(name, b"x"), good])
    t("tar_empty_name_file", [mb.tar_entry("", b"x"), good])
    t("tar_empty_name_dir", [mb.tar_entry("", kind=b"5"), good])
    t("tar_dot_file", [mb.tar_entry(".", b"x"), good])
    t("tar_dot_slash_file", [mb.tar_entry("./", b"x"), good])
    t("tar_dir_with_data", [mb.tar_entry("./d", b"abc", kind=b"5"), good])
    t("tar_bad_checksum_high", [mb.tar_entry("./x", b"1", checksum_delta=1), good])
    t("tar_bad_checksum_low", [mb.tar_entry("./x", b"1", checksum_delta=-1), good])
    t("tar_v7_no_magic", [mb.tar_entry("./patch.json", patch, magic=b"")])
    t("tar_gnu_magic", [mb.tar_entry("./patch.json", patch, magic=b"ustar  \x00")])
    t("tar_magic_wrong_case", [mb.tar_entry("./patch.json", patch, magic=b"USTAR\x0000")])
    t("tar_prefix_join", [mb.tar_entry("patch.json", patch, prefix=b"sub")])
    t("tar_prefix_dot", [mb.tar_entry("patch.json", patch, prefix=b".")])
    t("tar_prefix_traversal", [mb.tar_entry("x", b"1", prefix=b".."), good])
    t("tar_long_name_via_prefix", [mb.tar_entry("n" * 99, b"1", prefix=b"p" * 150), good])
    t("tar_name_100_bytes", [mb.tar_entry("n" * 100, b"1"), good])
    t("tar_invalid_utf8_name", [mb.ustar_header(b"\xff\xfe", 1) + b"x".ljust(512, b"\x00"), good])
    t("tar_size_base256", [mb.ustar_header(b"./x", 1, size_field=b"\x80" + b"\x00" * 10 + b"\x01") + b"x".ljust(512, b"\x00"), good])
    t("tar_size_blank", [mb.ustar_header(b"./x", 0, size_field=b" " * 12), good])
    t("tar_size_all_nul", [mb.ustar_header(b"./x", 0, size_field=b"\x00" * 12), good])
    t("tar_size_digit_8", [mb.ustar_header(b"./x", 0, size_field=b"00000000008\x00"), good])
    t("tar_size_space_padded", [mb.ustar_header(b"./x", 1, size_field=b"       1 \x00\x00\x00"[:12]) + b"x".ljust(512, b"\x00"), good])
    t("tar_size_no_terminator", [mb.ustar_header(b"./x", 1, size_field=b"000000000001") + b"x".ljust(512, b"\x00"), good])
    t("tar_size_past_end", [mb.ustar_header(b"./x", 100000)])
    t("tar_data_truncated", [mb.ustar_header(b"./patch.json", len(patch)) + patch[:100]])
    raw_tar("tar_data_padding_missing", mb.ustar_header(b"./patch.json", len(patch)) + patch)
    raw_tar("tar_header_only_no_data", mb.ustar_header(b"./patch.json", 0))
    # ---- pax
    def pax(records: bytes, entry: bytes | None = None, kind: bytes = b"x") -> list[bytes]:
        return [mb.tar_entry("PaxHeader/x", records, kind=kind), entry if entry is not None else mb.tar_entry("junk", patch)]

    t("tar_pax_path_override", pax(mb.pax_record(b"path", b"./patch.json")))
    t("tar_pax_path_override_wrong", pax(mb.pax_record(b"path", b"../patch.json")))
    t("tar_pax_path_utf8", pax(mb.pax_record(b"path", "./caf\u00e9/patch.json".encode()), mb.tar_entry("junk", patch)) + [good])
    t("tar_pax_path_invalid_utf8", pax(mb.pax_record(b"path", b"\xff")) + [good])
    t("tar_pax_path_empty", pax(mb.pax_record(b"path", b"")) + [good])
    t("tar_pax_path_nul", pax(mb.pax_record(b"path", b"a\x00b")) + [good])
    t("tar_pax_path_last_wins", pax(mb.pax_record(b"path", b"./first") + mb.pax_record(b"path", b"./patch.json")))
    t("tar_pax_ignored_keys", pax(mb.pax_record(b"mtime", b"1.5") + mb.pax_record(b"path", b"./patch.json") + mb.pax_record(b"uid", b"0") + mb.pax_record(b"linkpath", b"x")))
    t("tar_pax_size_override_smaller", [mb.tar_entry("PaxHeader/x", mb.pax_record(b"size", b"5"), kind=b"x"), mb.ustar_header(b"./patch.json", len(patch)) + b"12345".ljust(512, b"\x00")])
    t("tar_pax_size_override_zero_dir", [mb.tar_entry("PaxHeader/x", mb.pax_record(b"size", b"0"), kind=b"x"), mb.tar_entry("./d", b"", kind=b"5"), good])
    t("tar_pax_size_override_nondigit", pax(mb.pax_record(b"size", b"12x")) + [good])
    t("tar_pax_size_override_empty", pax(mb.pax_record(b"size", b"")) + [good])
    t("tar_pax_size_15_digits", pax(mb.pax_record(b"size", b"0" * 14 + b"5")) + [good])
    t("tar_pax_size_16_digits", pax(mb.pax_record(b"size", b"0" * 15 + b"5")) + [good])
    t("tar_pax_size_huge", pax(mb.pax_record(b"size", b"9" * 15)) + [good])
    t("tar_pax_consecutive", [mb.tar_entry("P", mb.pax_record(b"path", b"a"), kind=b"x")] * 2 + [good])
    t("tar_pax_at_end", [good, mb.tar_entry("P", mb.pax_record(b"path", b"a"), kind=b"x")])
    t("tar_pax_before_dir", [mb.tar_entry("P", mb.pax_record(b"path", b"./d"), kind=b"x"), mb.tar_entry("junk", kind=b"5"), good])
    t("tar_pax_empty_header", [mb.tar_entry("P", b"", kind=b"x"), mb.tar_entry("./ok", b"1"), good])
    t("tar_pax_applies_once", [mb.tar_entry("P", mb.pax_record(b"path", b"./patch.json"), kind=b"x"), mb.tar_entry("junk", patch), mb.tar_entry("./patch.json", patch)])
    for label, records in (
        ("no_equals", b"12 nothing\nxx"[:11] + b"\n"),
        ("bad_length_short", b"5 a=b\n"),
        ("bad_length_long", b"99 a=b\n"),
        ("no_newline", b"9 a=bcde\x00"),
        ("empty_key", b"5 =b\n"),
        ("zero_length", b"0 a=b\n"),
        ("no_space", b"a=b\n"),
        ("leading_space", b" 6 a=b\n"),
        ("digits_11", b"00000000009 a=b\n"),
        ("digits_10", b"0000000009 a=b\n"),
        ("length_six_valid", b"6 a=b\n"),
        ("trailing_junk", b"6 a=b\nx"),
        ("value_with_equals", b"10 a=b=cde\n"),
        ("length_exact_min", b"5 a=\n"),
        ("length_four", b"4 a=\n"),
        ("crlf", b"7 a=b\r\n"),
        ("multibyte_len", b"12 path=x\xc3\xa9\n"),
    ):
        t(f"tar_pax_malformed_{label}", pax(records) + [good])
    t("tar_pax_oversized", [mb.tar_entry("P", b"x" * 65537, kind=b"x"), good])
    t("tar_pax_max_size", [mb.tar_entry("P", mb.pax_record(b"comment", b"c" * (65536 - 40)) + b"", kind=b"x"), good])
    out.append(_inline("tar_all_regular_assets", ".vcv", mb.constructed_vcv()))
    out.append(_inline("tar_no_assets", ".vcv", _vcv(patch, extra_assets={})))
    out.append(_inline("tar_many_assets", ".vcv", _vcv(patch, extra_assets={f"a{i}.wav": b"x" * i for i in range(50)})))
    return out


def build_cases() -> list[dict]:
    cases = xm_cases() + mod_cases() + vcv_json_cases() + vcv_container_cases() + vcv_tar_cases()
    names = [c["name"] for c in cases]
    duplicates = {n for n in names if names.count(n) > 1}
    if duplicates:
        raise ValueError(f"duplicate corpus case names: {sorted(duplicates)}")
    return cases
