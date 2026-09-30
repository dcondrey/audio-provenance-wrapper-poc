"""Untrusted-input hardening shared by the container and XML/JSON project parsers.

Every limit is read through this module at call time so tests can lower it and
exercise limit-1 / limit / limit+1 without building 64 MiB inputs.
"""

from __future__ import annotations

import io
import json
import re
import zipfile
import zlib
from pathlib import Path
from typing import Any
from xml.etree import ElementTree as ET
from xml.parsers import expat

from . import registry

MAX_ZIP_MEMBERS = 10_000
MAX_ZIP_TOTAL_BYTES = 256 * 1024 * 1024
MAX_XML_BYTES = 256 * 1024 * 1024
MAX_XML_DEPTH = 128
MAX_XML_ELEMENTS = 2_000_000
MAX_JSON_DEPTH = 128
MAX_JSON_NODES = 2_000_000
MAX_TAR_MEMBERS = 10_000
MAX_TAR_BYTES = 256 * 1024 * 1024

_CHUNK = 64 * 1024


def read_project_bytes(path: Path) -> bytes:
    """Read a whole project file, refusing anything over MAX_PROJECT_FILE_BYTES."""
    limit = registry.MAX_PROJECT_FILE_BYTES
    size = path.stat().st_size
    if size > limit:
        raise ValueError(f"{path} is {size} bytes, past {limit}; refusing to parse")
    with path.open("rb") as handle:
        data = handle.read(limit + 1)
    if len(data) > limit:
        raise ValueError(f"{path} grew past {limit} bytes; refusing to parse")
    return data


def check_member_name(name: str) -> None:
    """Reject absolute, drive-lettered, NUL-bearing and parent-traversing archive names."""
    normal = name.replace("\\", "/")
    if not normal or "\x00" in normal or normal.startswith("/") or re.match(r"^[A-Za-z]:", normal):
        raise ValueError(f"unsafe archive member name: {name!r}")
    if any(part == ".." for part in normal.split("/")):
        raise ValueError(f"unsafe archive member name: {name!r}")


def open_zip(data: bytes) -> zipfile.ZipFile:
    """Open an in-memory zip after validating member count, names and declared sizes."""
    try:
        archive = zipfile.ZipFile(io.BytesIO(data))
    except (zipfile.BadZipFile, ValueError, OSError, NotImplementedError) as exc:
        raise ValueError(f"not a valid zip archive: {exc}") from exc
    infos = archive.infolist()
    if len(infos) > MAX_ZIP_MEMBERS:
        raise ValueError(f"zip has {len(infos)} members, past {MAX_ZIP_MEMBERS}; refusing to parse")
    seen: set[str] = set()
    total = 0
    for info in infos:
        check_member_name(info.filename)
        if info.filename in seen:
            raise ValueError(f"duplicate archive member: {info.filename!r}")
        seen.add(info.filename)
        total += info.file_size
    if total > MAX_ZIP_TOTAL_BYTES:
        raise ValueError(f"zip declares {total} decompressed bytes, past {MAX_ZIP_TOTAL_BYTES}; refusing to parse")
    return archive


def read_member(archive: zipfile.ZipFile, name: str, limit: int | None = None) -> bytes:
    """Read one member through a capped stream; the declared size is never trusted."""
    cap = MAX_XML_BYTES if limit is None else limit
    try:
        with archive.open(name) as handle:
            data = handle.read(cap + 1)
    except KeyError as exc:
        raise ValueError(f"archive has no member {name!r}") from exc
    except (zipfile.BadZipFile, RuntimeError, NotImplementedError, zlib.error, EOFError, OSError) as exc:
        raise ValueError(f"cannot read archive member {name!r}: {exc}") from exc
    if len(data) > cap:
        raise ValueError(f"archive member {name!r} decompresses past {cap} bytes; refusing to parse")
    return data


def inflate_zlib(data: bytes, limit: int | None = None) -> bytes:
    """Decompress a zlib stream, stopping as soon as output would pass the limit."""
    cap = MAX_XML_BYTES if limit is None else limit
    decoder = zlib.decompressobj()
    try:
        out = decoder.decompress(data, cap + 1)
        if len(out) <= cap and decoder.unconsumed_tail:
            out += decoder.decompress(decoder.unconsumed_tail, cap + 1 - len(out))
        if len(out) <= cap:
            out += decoder.flush()
    except zlib.error as exc:
        raise ValueError(f"invalid zlib data: {exc}") from exc
    if len(out) > cap:
        raise ValueError(f"data decompresses past {cap} bytes; refusing to parse")
    return out


def parse_xml(data: bytes, *, allowed_doctype: str | None = None) -> ET.Element:
    """Parse XML with no DTD/entity support and depth and element caps.

    DOCTYPE and entity declarations are refused structurally through expat
    handlers, so a `<!DOCTYPE` inside CDATA or a comment is not a false
    positive. `allowed_doctype` names one bodyless root doctype (LMMS writes
    `<!DOCTYPE lmms-project>`) that is tolerated because it declares nothing.
    Because refusal is structural, encoding tricks (UTF-16 and so on) cannot hide a DTD.
    Tail text is not kept; the supported formats do not use mixed content.
    """
    if len(data) > MAX_XML_BYTES:
        raise ValueError(f"XML is {len(data)} bytes, past {MAX_XML_BYTES}; refusing to parse")
    if data.startswith(b"\xef\xbb\xbf"):
        data = data[3:]

    parser = expat.ParserCreate("utf-8")
    parser.buffer_text = True
    stack: list[ET.Element] = []
    state: dict[str, Any] = {"root": None, "elements": 0}

    def start(tag: str, attrs: dict[str, str]) -> None:
        if len(stack) >= MAX_XML_DEPTH:
            raise ValueError(f"XML nesting deeper than {MAX_XML_DEPTH}; refusing to parse")
        state["elements"] += 1
        if state["elements"] > MAX_XML_ELEMENTS:
            raise ValueError(f"XML has more than {MAX_XML_ELEMENTS} elements; refusing to parse")
        element = ET.Element(tag, attrs)
        if stack:
            stack[-1].append(element)
        elif state["root"] is None:
            state["root"] = element
        stack.append(element)

    def end(_tag: str) -> None:
        stack.pop()

    def chars(text: str) -> None:
        if stack:
            stack[-1].text = (stack[-1].text or "") + text

    def doctype(name: str, system_id: str | None, public_id: str | None, has_internal_subset: int) -> None:
        if name != allowed_doctype or system_id or public_id or has_internal_subset:
            raise ValueError("XML contains a DTD declaration; refusing to parse")

    def forbidden(*_args: Any) -> None:
        raise ValueError("XML contains an entity or external declaration; refusing to parse")

    parser.StartElementHandler = start
    parser.EndElementHandler = end
    parser.CharacterDataHandler = chars
    parser.StartDoctypeDeclHandler = doctype
    parser.EntityDeclHandler = forbidden
    parser.ExternalEntityRefHandler = forbidden
    parser.NotationDeclHandler = forbidden
    parser.SkippedEntityHandler = forbidden
    try:
        for offset in range(0, max(len(data), 1), _CHUNK):
            parser.Parse(data[offset : offset + _CHUNK], False)
        parser.Parse(b"", True)
    except expat.ExpatError as exc:
        raise ValueError(f"malformed XML: {exc}") from exc
    if state["root"] is None:
        raise ValueError("XML has no root element")
    return state["root"]


def parse_json(data: bytes, limit: int | None = None) -> Any:
    """Parse JSON with bounded depth and node count; `limit` replaces the project size cap."""
    if len(data) > (registry.MAX_PROJECT_FILE_BYTES if limit is None else limit):
        raise ValueError("JSON exceeds the project size cap; refusing to parse")
    try:
        text = data.decode("utf-8-sig")
        value = json.loads(text)
    except (UnicodeDecodeError, ValueError, RecursionError) as exc:
        raise ValueError(f"malformed JSON: {exc}") from exc
    check_json_bounds(value)
    return value


def check_json_bounds(value: Any) -> None:
    nodes = 0
    stack: list[tuple[Any, int]] = [(value, 1)]
    while stack:
        item, depth = stack.pop()
        nodes += 1
        if nodes > MAX_JSON_NODES:
            raise ValueError(f"JSON has more than {MAX_JSON_NODES} nodes; refusing to parse")
        if isinstance(item, (dict, list)) and depth > MAX_JSON_DEPTH:
            raise ValueError(f"JSON nesting deeper than {MAX_JSON_DEPTH}; refusing to parse")
        if isinstance(item, dict):
            stack.extend((child, depth + 1) for child in item.values())
        elif isinstance(item, list):
            stack.extend((child, depth + 1) for child in item)
