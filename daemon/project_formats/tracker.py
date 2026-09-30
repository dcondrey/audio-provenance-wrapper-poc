"""MilkyTracker module formats: FastTracker 2 XM (.xm) and ProTracker MOD (.mod).

Grounding (no code is copied; MilkyTracker is GPL-3.0-or-later, its loader files
carry a BSD-style header):
  https://github.com/milkytracker/MilkyTracker/tree/e3ceb05105e8c5bdc907d6451245bf5a1926b1f9
    src/milkyplay/LoaderXM.cpp   XM header, pattern header, instrument and sample layout
    src/milkyplay/LoaderMOD.cpp  MOD title, 31 sample slots, order table, format tags
    src/milkyplay/XModule.h      MP_MAXPATTERNS 256, MP_MAXINS 255, MP_MAXINSSAMPS 96
  and the FastTracker 2 "The Unofficial XM File Format Specification" layout it
  implements (offsets below are that layout).

XM (version 0x0104 only; the 0x0102/0x0103 layouts put instruments before patterns
and are refused). Offsets: 0 "Extended Module:" id text, 17 module name (20), 38
tracker name (20), 58 version, 60 header size (counted from itself), then from 64:
song length, restart, channels, patterns, instruments, flags, ticks per row,
BPM, order table (256). A short header is zero-padded like LoaderXM.cpp does.
Pattern header: length (>= 9), packing type, rows, packed size; the packed cell
data is only hashed, never unpacked. Instrument header: size (>= 29), name (22),
type, sample count; when samples exist: sample-header size (>= 40) at +29, sample
headers start at instrument + size, each: length in bytes (u32), ..., type (+14),
name (+18, 22). Sample data follows all sample headers of that instrument and is
only hashed. A file that ends where an instrument header should start, or inside
sample data, is tolerated as LoaderXM.cpp tolerates it (reported as `truncated`).

MOD: the 31-sample layout only: title (20), 31 x 30-byte sample slots (name 22,
length in words BE, finetune, volume, loop start, loop length), song length at
950, restart at 951, order table 952..1080, format tag at 1080. The tag decides
the channel count exactly as LoaderMOD.cpp's getPTnumchannels; a file without a
recognised tag (which includes the heuristically detected 15-sample Soundtracker
layout) is refused. Pattern count is max(order table) + 1.

Mapping onto the shared snapshot: a `module` track named after the title whose
devices state the header facts, a `patterns` track (order table and pattern hash),
and an `Instrument` track per XM instrument or used MOD sample slot whose devices
are `name (N bytes)` per sample. Neither format stores a time signature; XM
stores BPM (`transport_bpm`), MOD stores none (0.0). Text fields are bytes up to
the first NUL, trailing spaces removed, decoded as ISO-8859-1.

The input is untrusted: every count is bounded by the format maxima below and
every read is bounds-checked; nothing is evaluated and no audio is decoded.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

from . import _safe
from ._snapshot import NeutralProject, NeutralTrack, build_snapshot, digest_of

MAX_XM_CHANNELS = 32
MAX_XM_PATTERNS = 256
MAX_XM_INSTRUMENTS = 255
MAX_XM_SAMPLES_PER_INSTRUMENT = 96
MAX_XM_ORDERS = 256
MAX_XM_ROWS = 256

_XM_MIN_HEADER = 272
_XM_VERSION = 0x0104


def _text(raw: bytes) -> str:
    return raw.split(b"\x00", 1)[0].rstrip(b" ").decode("latin-1")


def _u16(data: bytes, offset: int) -> int:
    return int.from_bytes(data[offset : offset + 2], "little")


def _u32(data: bytes, offset: int) -> int:
    return int.from_bytes(data[offset : offset + 4], "little")


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _sample_device(name: str, length: int) -> str:
    return f"{name} ({length} bytes)"


def _patterns_track(order: list[int], patterns: list[list[object]]) -> NeutralTrack:
    return NeutralTrack(
        track_id="patterns",
        name="patterns",
        track_type="Patterns",
        device_chain_hashes=frozenset({digest_of(order, patterns)}),
    )


def extract_xm(data: bytes) -> NeutralProject:
    if len(data) < 64:
        raise ValueError("XM header truncated")
    if data[:16] != b"Extended Module:":
        raise ValueError("not an XM module: missing 'Extended Module:' id text")
    version = _u16(data, 58)
    if version != _XM_VERSION:
        raise ValueError(f"unsupported XM version 0x{version:04x}; only 0x0104 is read")
    header_size = _u32(data, 60)
    if header_size < 4:
        raise ValueError("XM header size is smaller than its own field")
    header_end = 60 + header_size
    if header_end > len(data):
        raise ValueError("XM header truncated")
    header = data[64:header_end].ljust(_XM_MIN_HEADER, b"\x00")
    order_count = min(_u16(header, 0), MAX_XM_ORDERS)
    restart = _u16(header, 2)
    channels = _u16(header, 4)
    pattern_count = _u16(header, 6)
    instrument_count = _u16(header, 8)
    flags = _u16(header, 10)
    ticks = _u16(header, 12)
    bpm = _u16(header, 14)
    order = list(header[16:272])[:order_count]
    if not 1 <= channels <= MAX_XM_CHANNELS:
        raise ValueError(f"XM channel count {channels} is outside 1..{MAX_XM_CHANNELS}")
    if pattern_count > MAX_XM_PATTERNS:
        raise ValueError(f"XM has {pattern_count} patterns, past {MAX_XM_PATTERNS}")
    if instrument_count > MAX_XM_INSTRUMENTS:
        raise ValueError(f"XM has {instrument_count} instruments, past {MAX_XM_INSTRUMENTS}")

    position = header_end
    patterns: list[list[object]] = []
    for _ in range(pattern_count):
        if position + 9 > len(data):
            raise ValueError("XM pattern header truncated")
        length = _u32(data, position)
        rows = _u16(data, position + 5)
        packed = _u16(data, position + 7)
        if length < 9:
            raise ValueError("XM pattern header length is smaller than 9")
        if not 1 <= rows <= MAX_XM_ROWS:
            raise ValueError(f"XM pattern has {rows} rows, outside 1..{MAX_XM_ROWS}")
        start = position + length
        if start + packed > len(data):
            raise ValueError("XM pattern data truncated")
        patterns.append([rows, packed, _sha(data[start : start + packed])])
        position = start + packed

    truncated = False
    instruments: list[NeutralTrack] = []
    sample_total = 0
    for index in range(instrument_count):
        if position >= len(data):
            truncated = True
            break
        if position + 29 > len(data):
            raise ValueError("XM instrument header truncated")
        size = _u32(data, position)
        if size < 29:
            raise ValueError("XM instrument header size is smaller than 29")
        end = position + size
        if end > len(data):
            raise ValueError("XM instrument header truncated")
        name = _text(data[position + 4 : position + 26])
        sample_count = _u16(data, position + 27)
        if sample_count > MAX_XM_SAMPLES_PER_INSTRUMENT:
            raise ValueError(f"XM instrument has {sample_count} samples, past {MAX_XM_SAMPLES_PER_INSTRUMENT}")
        samples: list[tuple[str, int]] = []
        parts: list[object] = [_sha(data[position:end])]
        cursor = end
        if sample_count:
            if size < 33:
                raise ValueError("XM instrument header is too small for its samples")
            sample_header = _u32(data, position + 29)
            if sample_header < 40:
                raise ValueError("XM sample header size is smaller than 40")
            headers = []
            for _ in range(sample_count):
                if cursor + sample_header > len(data):
                    raise ValueError("XM sample header truncated")
                headers.append(data[cursor : cursor + sample_header])
                cursor += sample_header
            for header_bytes in headers:
                length = _u32(header_bytes, 0)
                stop = min(cursor + length, len(data))
                if cursor + length > len(data):
                    truncated = True
                samples.append((_text(header_bytes[18:40]), length))
                parts.append([_sha(header_bytes), _sha(data[cursor:stop])])
                cursor = stop
        sample_total += len(samples)
        instruments.append(
            NeutralTrack(
                track_id=f"instrument-{index + 1}",
                name=name,
                track_type="Instrument",
                devices=tuple(_sample_device(n, length) for n, length in samples),
                device_chain_hashes=frozenset({digest_of(parts)}),
            )
        )
        position = cursor

    facts = [
        f"format: XM {version >> 8}.{version & 0xFF:02d}",
        f"tracker: {_text(data[38:58])}",
        f"channels: {channels}",
        f"song length: {order_count}",
        f"restart position: {restart}",
        f"patterns: {pattern_count}",
        f"instruments: {instrument_count}",
        f"samples: {sample_total}",
        f"frequency table: {'linear' if flags & 1 else 'amiga'}",
        f"ticks per row: {ticks}",
        f"bpm: {bpm}",
    ]
    if truncated:
        facts.append("truncated")
    module = NeutralTrack(track_id="module", name=_text(data[17:37]), track_type="Module", devices=tuple(facts))
    return NeutralProject(
        project_format="milkytracker",
        tracks=[module, _patterns_track(order, patterns), *instruments],
        bpm=float(bpm),
    )


def _mod_channels(tag: bytes) -> int:
    if tag in (b"M.K.", b"M!K!", b"FLT4"):
        return 4
    if tag in (b"FLT8", b"OKTA", b"OCTA", b"FA08", b"CD81"):
        return 8
    first, second = tag[0], tag[1]
    if 0x31 <= first <= 0x39 and tag[1:] == b"CHN":
        return first - 0x30
    if 0x31 <= first <= 0x39 and 0x30 <= second <= 0x39 and tag[2:] in (b"CH", b"CN"):
        return (first - 0x30) * 10 + second - 0x30
    return 0


def extract_mod(data: bytes) -> NeutralProject:
    if len(data) < 1084:
        raise ValueError("MOD header truncated")
    tag = data[1080:1084]
    channels = _mod_channels(tag)
    if not channels:
        raise ValueError("not a supported MOD module: no recognised format tag at offset 1080")
    song_length = data[950]
    restart = data[951]
    order = list(data[952:1080])
    pattern_count = max(order) + 1
    pattern_size = channels * 256
    truncated = False
    patterns: list[list[object]] = []
    position = 1084
    for _ in range(pattern_count):
        stop = min(position + pattern_size, len(data))
        if position + pattern_size > len(data):
            truncated = True
        patterns.append([64, stop - position, _sha(data[position:stop])])
        position = stop

    instruments: list[NeutralTrack] = []
    sample_total = 0
    for slot in range(31):
        offset = 20 + 30 * slot
        name = _text(data[offset : offset + 22])
        length = int.from_bytes(data[offset + 22 : offset + 24], "big") * 2
        payload = b""
        if length > 2:
            stop = min(position + length, len(data))
            if position + length > len(data):
                truncated = True
            payload = data[position:stop]
            position = stop
        if not name and not length:
            continue
        sample_total += 1 if length > 2 else 0
        instruments.append(
            NeutralTrack(
                track_id=f"instrument-{slot + 1}",
                name=name,
                track_type="Instrument",
                devices=(_sample_device(name, length),) if length > 2 else (),
                device_chain_hashes=frozenset({digest_of([_sha(data[offset : offset + 30]), _sha(payload)])}),
            )
        )

    facts = [
        f"format: MOD {_text(tag)}",
        f"channels: {channels}",
        f"song length: {song_length}",
        f"restart position: {restart}",
        f"patterns: {pattern_count}",
        f"samples: {sample_total}",
    ]
    if truncated:
        facts.append("truncated")
    module = NeutralTrack(track_id="module", name=_text(data[0:20]), track_type="Module", devices=tuple(facts))
    return NeutralProject(
        project_format="milkytracker",
        tracks=[module, _patterns_track(order[: min(song_length, 128)], patterns), *instruments],
    )


def extract_module_snapshot(path: Path):
    data = _safe.read_project_bytes(path)
    project = extract_xm(data) if data[:16] == b"Extended Module:" else extract_mod(data)
    return build_snapshot(project, path)
