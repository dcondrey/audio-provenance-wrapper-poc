"""Constructors for the `.vcv` fixture and for the malformed module/VCV corpus.

Used by build_fixtures.py (the committed constructed `vcv/basic.vcv`) and by
tests/fixtures/parity/generate_project_fixtures.py (the shared corpus both the
Python and the Rust parsers are tested against). Everything here is built from
the layouts documented in docs/PROJECT_FORMATS.md; no Rack, MilkyTracker or
OpenMPT bytes are copied.
"""

from __future__ import annotations

import json
import struct

import zstandard

# ---------------------------------------------------------------- tar


def _octal(value: int, width: int) -> bytes:
    return f"{value:0{width - 1}o}".encode("ascii") + b"\x00"


def ustar_header(
    name: bytes,
    size: int,
    kind: bytes = b"0",
    *,
    prefix: bytes = b"",
    magic: bytes = b"ustar\x0000",
    checksum_delta: int = 0,
    size_field: bytes | None = None,
) -> bytes:
    """One 512-byte ustar header laid out as libarchive's pax_restricted writer does."""
    block = bytearray(512)
    block[0 : len(name)] = name
    block[100:108] = b"0000644\x00"
    block[108:116] = b"0000000\x00"
    block[116:124] = b"0000000\x00"
    block[124:136] = size_field if size_field is not None else _octal(size, 12)
    block[136:148] = _octal(0, 12)
    block[148:156] = b" " * 8
    block[156:157] = kind
    block[257 : 257 + len(magic)] = magic
    block[345 : 345 + len(prefix)] = prefix
    checksum = sum(block) + checksum_delta
    block[148:156] = f"{checksum:06o}".encode("ascii") + b"\x00 "
    return bytes(block)


def _padded(data: bytes) -> bytes:
    return data + b"\x00" * (-len(data) % 512)


def pax_record(key: bytes, value: bytes) -> bytes:
    body = b" " + key + b"=" + value + b"\n"
    length = len(body) + 1
    while len(str(length).encode()) + len(body) != length:
        length = len(str(length).encode()) + len(body)
    return str(length).encode() + body


def tar_entry(name: str, data: bytes = b"", kind: bytes = b"0", **kwargs) -> bytes:
    return ustar_header(name.encode("utf-8"), len(data), kind, **kwargs) + _padded(data)


def tar_bytes(entries: list[bytes], trailer: bool = True) -> bytes:
    return b"".join(entries) + (b"\x00" * 1024 if trailer else b"")


# ---------------------------------------------------------------- zstd


def zstd_stream(data: bytes, *, level: int = 3, checksum: bool = False, content_size: bool = False) -> bytes:
    """One Zstandard frame; without content size, like libarchive's streaming writer."""
    compressor = zstandard.ZstdCompressor(level=level, write_checksum=checksum, write_content_size=content_size)
    if content_size:
        return compressor.compress(data)
    stream = compressor.compressobj()
    return stream.compress(data) + stream.flush()


def zstd_raw_frame(
    data: bytes,
    *,
    single_segment: bool = True,
    declared_size: int | None = None,
    checksum_flag: bool = False,
    descriptor_or: int = 0,
    window_descriptor: int = 0,
    block_size: int = 1 << 17,
) -> bytes:
    """A hand-built frame of raw blocks. `descriptor_or` sets arbitrary descriptor bits."""
    size = len(data) if declared_size is None else declared_size
    if single_segment:
        if size < 256:
            fcs_flag, fcs = 0, struct.pack("<B", size)
        elif size < 65536 + 256:
            fcs_flag, fcs = 1, struct.pack("<H", size - 256)
        elif size < 1 << 32:
            fcs_flag, fcs = 2, struct.pack("<I", size)
        else:
            fcs_flag, fcs = 3, struct.pack("<Q", size)
        descriptor = (fcs_flag << 6) | 0x20
        header = bytes([descriptor | (0x04 if checksum_flag else 0) | descriptor_or]) + fcs
    else:
        descriptor = 0x00
        header = bytes([descriptor | (0x04 if checksum_flag else 0) | descriptor_or, window_descriptor])
    blocks = b""
    chunks = [data[i : i + block_size] for i in range(0, len(data), block_size)] or [b""]
    for index, chunk in enumerate(chunks):
        last = 1 if index == len(chunks) - 1 else 0
        blocks += struct.pack("<I", (len(chunk) << 3) | (0 << 1) | last)[:3] + chunk
    return zstandard.FRAME_HEADER + header + blocks + (b"\x00\x00\x00\x00" if checksum_flag else b"")


# ---------------------------------------------------------------- VCV patch


def constructed_patch() -> dict:
    """A patch shaped by Rack v2.6.6 Manager::toJson / Module::toJson / Cable::toJson."""
    return {
        "version": "2.6.6",
        "zoom": 1.0,
        "gridOffset": [0.0, 0.0],
        "modules": [
            {
                "id": 101,
                "plugin": "Fundamental",
                "model": "VCO",
                "version": "2.6.4",
                "params": [{"value": 0.0, "id": 0}, {"value": 1.0, "id": 1}, {"value": 0.5, "id": 2}],
                "leftModuleId": 100,
                "pos": [0, 0],
            },
            {
                "id": 102,
                "plugin": "Fundamental",
                "model": "VCF",
                "version": "2.6.4",
                "params": [{"value": 0.25, "id": 0}, {"value": 0.5, "id": 1}],
                "pos": [9, 0],
            },
            {
                "id": 103,
                "plugin": "Core",
                "model": "AudioInterface2",
                "version": "2.6.6",
                "params": [{"value": 1.0, "id": 0}],
                "data": {"audio": {"driver": 5, "deviceName": "Speakers", "sampleRate": 48000.0, "blockSize": 256}},
                "pos": [18, 0],
            },
            {
                "id": 104,
                "plugin": "Fundamental",
                "model": "Scope",
                "version": "2.6.4",
                "params": [],
                "bypass": True,
                "pos": [27, 0],
            },
            {"id": 105, "plugin": "Fundamental", "model": "WTLFO", "version": "2.6.4", "params": [], "data": {"wavetable": "wavetable.wav"}, "pos": [36, 0]},
        ],
        "cables": [
            {"id": 1, "outputModuleId": 101, "outputId": 0, "inputModuleId": 102, "inputId": 3, "color": "#f3374b"},
            {"id": 2, "outputModuleId": 102, "outputId": 0, "inputModuleId": 103, "inputId": 0, "color": "#ffb437"},
            {"id": 3, "outputModuleId": 102, "outputId": 0, "inputModuleId": 104, "inputId": 0, "color": "#00b56e"},
        ],
        "masterModuleId": 103,
    }


def patch_json_bytes(patch: dict) -> bytes:
    return (json.dumps(patch, indent=2) + "\n").encode("utf-8")


def rack_tar(patch_json: bytes, extra_assets: dict[str, bytes] | None = None) -> bytes:
    """Entries and names in the order Rack 2's archiveDirectory writes them (`./...`)."""
    assets = extra_assets if extra_assets is not None else {"wavetable.wav": b"RIFF-constructed-asset-bytes"}
    entries = [
        tar_entry(".", kind=b"5"),
        tar_entry("./modules", kind=b"5"),
        tar_entry("./patch.json", patch_json),
    ]
    if assets:
        entries.append(tar_entry("./modules/105", kind=b"5"))
        for name, blob in assets.items():
            entries.append(tar_entry(f"./modules/105/{name}", blob))
    return tar_bytes(entries)


def constructed_vcv() -> bytes:
    return zstd_stream(rack_tar(patch_json_bytes(constructed_patch())), level=1)


# ---------------------------------------------------------------- XM and MOD


def _pad(text: str | bytes, width: int) -> bytes:
    raw = text.encode("latin-1") if isinstance(text, str) else text
    return raw[:width].ljust(width, b"\x00")


def xm_bytes(
    *,
    name: str = "Song",
    tracker: str = "constructed",
    version: int = 0x0104,
    channels: int = 2,
    orders: tuple[int, ...] = (0,),
    order_count: int | None = None,
    restart: int = 0,
    flags: int = 1,
    ticks: int = 6,
    bpm: int = 125,
    header_size: int = 276,
    patterns: list[tuple[int, bytes]] | None = None,
    pattern_count: int | None = None,
    pattern_header: int = 9,
    instruments: list[dict] | None = None,
    instrument_count: int | None = None,
) -> bytes:
    """Build an XM. Instruments: {name, samples: [(name, declared_length, actual_data_bytes)], size, sample_header}."""
    patterns = [(1, b"")] if patterns is None else patterns
    instruments = [] if instruments is None else instruments
    body = struct.pack(
        "<8H",
        len(orders) if order_count is None else order_count,
        restart,
        channels,
        len(patterns) if pattern_count is None else pattern_count,
        len(instruments) if instrument_count is None else instrument_count,
        flags,
        ticks,
        bpm,
    ) + bytes(orders).ljust(256, b"\x00")
    want = header_size - 4
    body = body[:want] if want < len(body) else body + b"\x00" * (want - len(body))
    out = b"Extended Module: " + _pad(name, 20) + b"\x1a" + _pad(tracker, 20) + struct.pack("<HI", version, header_size) + body
    for rows, packed in patterns:
        out += struct.pack("<IBHH", pattern_header, 0, rows, len(packed)) + b"\x00" * (pattern_header - 9) + packed
    for instrument in instruments:
        samples = instrument.get("samples", [])
        size = instrument.get("size", 263 if samples else 29)
        sample_header = instrument.get("sample_header", 40)
        head = struct.pack("<I", size) + _pad(instrument.get("name", ""), 22) + b"\x00" + struct.pack("<H", len(samples))
        if samples:
            head += struct.pack("<I", sample_header)
        out += head.ljust(size, b"\x00")
        headers, payload = b"", b""
        for index, (sample_name, declared, actual) in enumerate(samples):
            header = (
                struct.pack("<III", declared, 0, 0) + bytes([64, 0, 0, 128, 0, 0]) + _pad(sample_name, 22)
            ).ljust(sample_header, b"\x00")
            headers += header
            payload += bytes((index * 37 + i) & 0xFF for i in range(actual))
        out += headers + payload
    return out


def mod_bytes(
    *,
    title: str = "Song",
    slots: list[tuple[str, int]] | None = None,
    song_length: int = 1,
    restart: int = 127,
    order: tuple[int, ...] = (0,),
    tag: bytes = b"M.K.",
    channels: int = 4,
    sample_bytes: int | None = None,
) -> bytes:
    """Build a MOD. slots: (name, length in words); data is generated to match unless sample_bytes cuts it short."""
    slots = [] if slots is None else slots
    out = _pad(title, 20)
    for index in range(31):
        name, words = slots[index] if index < len(slots) else ("", 0)
        out += _pad(name, 22) + struct.pack(">H", words) + bytes([0, 64]) + struct.pack(">HH", 0, 1)
    out += bytes([song_length, restart]) + bytes(order).ljust(128, b"\x00") + tag
    out += bytes((i * 7) & 0xFF for i in range((max(order) + 1) * channels * 256))
    data = b"".join(bytes((i + 3) & 0xFF for i in range(words * 2)) for _, words in slots if words * 2 > 2)
    return out + (data if sample_bytes is None else data[:sample_bytes])
