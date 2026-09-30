"""Bounded Zstandard and ustar/pax readers for `.vcv` archives.

The Rust port (`rust/apw-daemon/src/project/tarzst.rs`) implements the same rules
line for line; `tests/fixtures/parity/project_modules_corpus.json` holds the
inputs both are tested against. Nothing here writes to disk or executes anything.

Zstandard (RFC 8878): a `.vcv` is exactly one frame that consumes the whole
input. The frame is walked here first (header, block headers, checksum) so that
truncation, trailing bytes, dictionaries, reserved bits and oversized windows are
refused identically whichever decoder follows. Decoding is then capped.

tar: POSIX ustar with pax `path` and `size` overrides, as written by libarchive's
`pax_restricted` format (what Rack 2 uses). Only regular files and directories are
accepted; links, devices, FIFOs, GNU long-name entries and pax global headers are
refused. Names may not be absolute, drive-lettered, NUL-bearing or contain `..`;
`.` segments are dropped (Rack writes `./patch.json`). Regular-file names must be
unique after that.
"""

from __future__ import annotations

import io

from . import _safe

ZSTD_MAGIC = b"\x28\xb5\x2f\xfd"
_ZSTD_MAX_WINDOW = 1 << 27
_ZSTD_MAX_BLOCK = 128 * 1024

_BLOCK = 512
_ZERO_BLOCK = bytes(_BLOCK)
_MAX_PAX_BYTES = 64 * 1024
_MAX_PAX_DIGITS = 10
_MAX_PAX_SIZE_DIGITS = 15


def _oversize_message(cap: int) -> str:
    return f"Zstandard data is invalid or decompresses past {cap} bytes; refusing to parse"


def _frame_walk(data: bytes) -> int | None:
    """Validate one whole Zstandard frame; return its declared content size or None."""
    if data[:4] != ZSTD_MAGIC:
        raise ValueError("not a Zstandard frame")
    if len(data) < 6:
        raise ValueError("truncated Zstandard frame")
    descriptor = data[4]
    fcs_flag = descriptor >> 6
    single_segment = bool(descriptor & 0x20)
    if descriptor & 0x08 or descriptor & 0x03:
        raise ValueError("unsupported Zstandard frame: reserved bit or dictionary")
    has_checksum = bool(descriptor & 0x04)
    pos = 5
    window_descriptor = None
    if not single_segment:
        window_descriptor = data[pos]
        pos += 1
    fcs_size = (1 if single_segment else 0, 2, 4, 8)[fcs_flag]
    if pos + fcs_size > len(data):
        raise ValueError("truncated Zstandard frame")
    content_size: int | None = None
    if fcs_size:
        content_size = int.from_bytes(data[pos : pos + fcs_size], "little")
        if fcs_flag == 1:
            content_size += 256
    pos += fcs_size
    if window_descriptor is not None:
        base = 1 << (10 + (window_descriptor >> 3))
        window = base + (base >> 3) * (window_descriptor & 7)
    else:
        window = content_size or 0
    if window > _ZSTD_MAX_WINDOW:
        raise ValueError("unsupported Zstandard frame: window past 128 MiB")
    block_max = min(window, _ZSTD_MAX_BLOCK) if window else _ZSTD_MAX_BLOCK
    while True:
        if pos + 3 > len(data):
            raise ValueError("truncated Zstandard frame")
        header = int.from_bytes(data[pos : pos + 3], "little")
        pos += 3
        last, kind, size = header & 1, (header >> 1) & 3, header >> 3
        if kind == 3:
            raise ValueError("unsupported Zstandard frame: reserved block type")
        if size > block_max:
            raise ValueError("unsupported Zstandard frame: block past the maximum block size")
        pos += 1 if kind == 1 else size
        if pos > len(data):
            raise ValueError("truncated Zstandard frame")
        if last:
            break
    if has_checksum:
        pos += 4
        if pos > len(data):
            raise ValueError("truncated Zstandard frame")
    if pos != len(data):
        raise ValueError("trailing data after the Zstandard frame")
    return content_size


def inflate_zstd(data: bytes, limit: int | None = None) -> bytes:
    """Decompress one Zstandard frame, refusing output past the cap."""
    cap = _safe.MAX_TAR_BYTES if limit is None else limit
    content_size = _frame_walk(data)
    if content_size is not None and content_size > cap:
        raise ValueError(_oversize_message(cap))
    try:
        import zstandard
    except ImportError as exc:  # pragma: no cover - dependency is pinned in requirements.txt
        raise ValueError("the zstandard package is required to read VCV Rack patches") from exc
    try:
        with zstandard.ZstdDecompressor().stream_reader(io.BytesIO(data), read_across_frames=False) as reader:
            out = reader.read(cap + 1)
    except zstandard.ZstdError as exc:
        raise ValueError(_oversize_message(cap)) from exc
    if len(out) > cap:
        raise ValueError(_oversize_message(cap))
    if content_size is not None and len(out) != content_size:
        raise ValueError(_oversize_message(cap))
    return out


def _octal(field: bytes) -> int:
    if field and field[0] & 0x80:
        raise ValueError("unsupported tar numeric encoding")
    digits = field.strip(b"\x00 ")
    if not digits or any(byte < 0x30 or byte > 0x37 for byte in digits):
        raise ValueError("malformed tar number")
    return int(digits, 8)


def _cstring(field: bytes) -> bytes:
    return field.split(b"\x00", 1)[0]


def _decode_name(raw: bytes) -> str:
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ValueError("tar entry name is not valid UTF-8") from exc


def _parse_pax(buf: bytes) -> dict[bytes, bytes]:
    records: dict[bytes, bytes] = {}
    index = 0
    while index < len(buf):
        space = buf.find(b" ", index)
        digits = buf[index:space] if space > index else b""
        if not (1 <= len(digits) <= _MAX_PAX_DIGITS) or any(byte < 0x30 or byte > 0x39 for byte in digits):
            raise ValueError("malformed pax record")
        length = int(digits)
        if length < len(digits) + 3 or index + length > len(buf):
            raise ValueError("malformed pax record")
        record = buf[space + 1 : index + length]
        if not record.endswith(b"\n"):
            raise ValueError("malformed pax record")
        record = record[:-1]
        equals = record.find(b"=")
        if equals < 1:
            raise ValueError("malformed pax record")
        records[record[:equals]] = record[equals + 1 :]
        index += length
    return records


def _normalise(name: str) -> str:
    _safe.check_member_name(name)
    return "/".join(part for part in name.replace("\\", "/").split("/") if part not in ("", "."))


def _padded(size: int) -> int:
    return (size + _BLOCK - 1) // _BLOCK * _BLOCK


def read_tar(data: bytes) -> list[tuple[str, int, int]]:
    """Validate a whole tar stream; return (name, offset, size) of every regular file."""
    members: list[tuple[str, int, int]] = []
    seen: set[str] = set()
    pax: dict[bytes, bytes] | None = None
    entries = 0
    pos = 0
    while pos != len(data):
        if pos + _BLOCK > len(data):
            raise ValueError("truncated tar header")
        block = data[pos : pos + _BLOCK]
        if block == _ZERO_BLOCK:
            break
        checksum = sum(block[:148]) + 32 * 8 + sum(block[156:])
        if _octal(block[148:156]) != checksum:
            raise ValueError("tar header checksum mismatch")
        if block[257:262] != b"ustar":
            raise ValueError("unsupported tar format: missing ustar magic")
        size = _octal(block[124:136])
        kind = block[156]
        start = pos + _BLOCK
        if kind == 0x78:  # 'x' pax extended header
            if pax is not None:
                raise ValueError("tar has consecutive pax headers")
            if size > _MAX_PAX_BYTES or start + size > len(data):
                raise ValueError("truncated or oversized pax header")
            pax = _parse_pax(data[start : start + size])
            pos = start + _padded(size)
            continue
        if kind not in (0x30, 0x00, 0x35):
            raise ValueError(f"unsupported tar entry type {kind}")
        name_bytes = _cstring(block[0:100])
        prefix = _cstring(block[345:500])
        if prefix:
            name_bytes = prefix + b"/" + name_bytes
        if pax is not None:
            if b"path" in pax:
                name_bytes = pax[b"path"]
            if b"size" in pax:
                digits = pax[b"size"]
                if not (1 <= len(digits) <= _MAX_PAX_SIZE_DIGITS) or any(b < 0x30 or b > 0x39 for b in digits):
                    raise ValueError("malformed pax size")
                size = int(digits)
        pax = None
        name = _normalise(_decode_name(name_bytes))
        entries += 1
        if entries > _safe.MAX_TAR_MEMBERS:
            raise ValueError(f"tar has more than {_safe.MAX_TAR_MEMBERS} members; refusing to parse")
        if kind == 0x35:
            if size:
                raise ValueError("tar directory entry has data")
        else:
            if not name:
                raise ValueError("tar file entry has no name")
            if name in seen:
                raise ValueError(f"duplicate archive member: {name!r}")
            if start + size > len(data):
                raise ValueError("truncated tar entry")
            seen.add(name)
            members.append((name, start, size))
        pos = start + _padded(size)
    if pax is not None:
        raise ValueError("tar ends after a pax header")
    return members
