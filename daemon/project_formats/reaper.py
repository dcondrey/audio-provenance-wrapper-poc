"""REAPER .rpp parser.

RPP is plain text: nested `<TAG args ... >` blocks containing `KEY args` lines.
The layout is not officially specified; this parser reads the structure widely
documented by the REAPER community (SAMPLERATE, TEMPO, MARKER, TRACK, ITEM,
SOURCE, FXCHAIN) and reports only what it finds. It is exercised against
hand-written fixtures, not against files written by a real REAPER install.

The input is untrusted: size, depth, node and line counts are capped, and there
is no XML or entity processing.
"""

from __future__ import annotations

import hashlib
from dataclasses import dataclass, field
from pathlib import Path

from daemon.common import sha256_file
from daemon.project_differ.differ import ClipInfo, ProjectSnapshot, TrackInfo

from .registry import MAX_PROJECT_FILE_BYTES

MAX_DEPTH = 64
MAX_NODES = 500_000
MAX_LINES = 5_000_000

_FX_TAGS = frozenset({"VST", "VST3", "JS", "AU", "AUI", "CLAP", "DX", "VIDEO_EFFECT"})


@dataclass
class RppNode:
    tag: str
    args: list[str]
    attrs: list[tuple[str, list[str]]] = field(default_factory=list)
    children: list["RppNode"] = field(default_factory=list)
    digest: str = ""

    def attr(self, key: str) -> list[str] | None:
        for name, values in self.attrs:
            if name == key:
                return values
        return None

    def descendants(self, tag: str):
        for child in self.children:
            if child.tag == tag:
                yield child
            yield from child.descendants(tag)

    def walk(self):
        yield self
        for child in self.children:
            yield from child.walk()


def split_args(text: str) -> list[str]:
    """Split on whitespace, honouring "double", 'single' and `backtick` quoting."""
    out: list[str] = []
    i, n = 0, len(text)
    while i < n:
        ch = text[i]
        if ch.isspace():
            i += 1
        elif ch in "\"'`":
            end = text.find(ch, i + 1)
            if end == -1:
                out.append(text[i + 1 :])
                break
            out.append(text[i + 1 : end])
            i = end + 1
        else:
            j = i
            while j < n and not text[j].isspace():
                j += 1
            out.append(text[i:j])
            i = j
    return out


def parse_rpp(text: str) -> RppNode:
    """Parse RPP text into a tree. Raises ValueError on malformed or over-limit input."""
    lines = text.splitlines()
    if len(lines) > MAX_LINES:
        raise ValueError("RPP has too many lines; refusing to parse")

    root: RppNode | None = None
    stack: list[tuple[RppNode, int]] = []
    node_count = 0

    def close(node: RppNode, start: int, end: int) -> None:
        node.digest = hashlib.sha256("\n".join(s.strip() for s in lines[start : end + 1]).encode()).hexdigest()[:16]

    for index, raw in enumerate(lines):
        line = raw.strip()
        if not line:
            continue
        if root is None:
            if not line.startswith("<REAPER_PROJECT"):
                raise ValueError("not a REAPER project: missing <REAPER_PROJECT header")
        if line.startswith("<"):
            parts = split_args(line[1:])
            if not parts:
                raise ValueError(f"line {index + 1}: empty block tag")
            node = RppNode(tag=parts[0], args=parts[1:])
            node_count += 1
            if node_count > MAX_NODES:
                raise ValueError("RPP has too many blocks; refusing to parse")
            if stack:
                stack[-1][0].children.append(node)
            elif root is not None:
                raise ValueError(f"line {index + 1}: second top-level block")
            else:
                root = node
            if len(stack) >= MAX_DEPTH:
                raise ValueError(f"RPP nesting deeper than {MAX_DEPTH}; refusing to parse")
            stack.append((node, index))
        elif line == ">":
            if not stack:
                raise ValueError(f"line {index + 1}: unbalanced '>'")
            node, start = stack.pop()
            close(node, start, index)
        elif stack:
            # Opaque payload lines (base64 plug-in state) are long and not keys;
            # dropping them keeps memory proportional to the structure.
            key_end = line.find(" ")
            if (len(line) if key_end == -1 else key_end) <= 40:
                parts = split_args(line)
                stack[-1][0].attrs.append((parts[0], parts[1:]))
        else:
            raise ValueError(f"line {index + 1}: content outside the project block")

    if root is None or stack:
        raise ValueError("RPP is empty or has an unclosed block")
    return root


def _float(values: list[str] | None, position: int = 0, default: float = 0.0) -> float:
    try:
        return float(values[position])  # type: ignore[index]
    except (TypeError, ValueError, IndexError):
        return default


def _int(values: list[str] | None, position: int = 0) -> int | None:
    try:
        return int(float(values[position]))  # type: ignore[index]
    except (TypeError, ValueError, IndexError):
        return None


@dataclass(frozen=True)
class ReaperSource:
    source_type: str
    file: str


@dataclass(frozen=True)
class ReaperItem:
    item_id: str
    name: str
    position_seconds: float
    length_seconds: float
    sources: tuple[ReaperSource, ...]
    digest: str


@dataclass(frozen=True)
class ReaperTrack:
    track_id: str
    name: str
    is_folder: bool
    items: tuple[ReaperItem, ...]
    devices: tuple[str, ...]
    fx_digests: frozenset[str]
    envelope_points: int
    midi_note_ons: int


@dataclass(frozen=True)
class ReaperProject:
    version: str
    sample_rate: int | None
    tempo_bpm: float | None
    time_signature: tuple[int, int] | None
    tempo_envelope: bool
    loop_on: bool
    markers: int
    regions: int
    tracks: tuple[ReaperTrack, ...]


def _extract_source(node: RppNode) -> list[ReaperSource]:
    found: list[ReaperSource] = []
    for src in node.descendants("SOURCE"):
        file_attr = src.attr("FILE")
        found.append(
            ReaperSource(
                source_type=src.args[0] if src.args else "",
                file=file_attr[0] if file_attr else "",
            )
        )
    return found


def _is_note_on(values: list[str]) -> bool:
    # MIDI event line: `E <delta> <status> <data1> <data2>`; note-on with velocity 0 is a note-off.
    if len(values) < 4:
        return False
    try:
        status, velocity = int(values[1], 16), int(values[3], 16)
    except ValueError:
        return False
    return (status & 0xF0) == 0x90 and velocity > 0


def _extract_track(node: RppNode, position: int) -> ReaperTrack:
    name_attr = node.attr("NAME")
    isbus = node.attr("ISBUS")
    items: list[ReaperItem] = []
    for item_index, item in enumerate(node.descendants("ITEM")):
        iguid = item.attr("IGUID")
        name = item.attr("NAME")
        items.append(
            ReaperItem(
                item_id=iguid[0] if iguid else f"pos-{item_index}",
                name=name[0] if name else "",
                position_seconds=_float(item.attr("POSITION")),
                length_seconds=_float(item.attr("LENGTH")),
                sources=tuple(_extract_source(item)),
                digest=item.digest,
            )
        )

    devices: list[str] = []
    fx_digests: set[str] = set()
    for chain in node.descendants("FXCHAIN"):
        fx_digests.add(chain.digest)
        devices.extend(c.args[0] if c.args else c.tag for c in chain.children if c.tag in _FX_TAGS)

    points = 0
    notes = 0
    for sub in node.walk():
        if "ENV" in sub.tag:
            points += sum(1 for key, _ in sub.attrs if key == "PT")
        if sub.tag == "SOURCE" and sub.args[:1] == ["MIDI"]:
            notes += sum(1 for key, vals in sub.attrs if key in ("E", "e") and _is_note_on(vals))

    return ReaperTrack(
        track_id=node.args[0] if node.args else f"pos-{position}",
        name=name_attr[0] if name_attr else "",
        is_folder=bool(isbus and _int(isbus, 0) == 1),
        items=tuple(items),
        devices=tuple(devices),
        fx_digests=frozenset(fx_digests),
        envelope_points=points,
        midi_note_ons=notes,
    )


def extract_reaper_project(text: str) -> ReaperProject:
    root = parse_rpp(text)
    if root.tag != "REAPER_PROJECT":
        raise ValueError("not a REAPER project")

    rate = _int(root.attr("SAMPLERATE"), 0)
    tempo = root.attr("TEMPO")
    markers = regions = 0
    for key, vals in root.attrs:
        if key == "MARKER":
            # MARKER <idx> <pos> "<name>" <flags>: flag bit 0 marks a region.
            flags = _int(vals, 3) or 0
            if flags & 1:
                regions += 1
            else:
                markers += 1

    return ReaperProject(
        version=root.args[0] if root.args else "",
        sample_rate=rate if rate and rate > 0 else None,
        tempo_bpm=_float(tempo, 0) if tempo else None,
        time_signature=(
            (_int(tempo, 1) or 4, _int(tempo, 2) or 4) if tempo and len(tempo) >= 3 else None
        ),
        tempo_envelope=any(child.tag.startswith("TEMPOENV") for child in root.children),
        loop_on=_int(root.attr("LOOP"), 0) == 1,
        markers=markers,
        regions=regions,
        tracks=tuple(
            _extract_track(child, i)
            for i, child in enumerate(c for c in root.children if c.tag == "TRACK")
        ),
    )


def read_rpp_text(path: Path) -> str:
    size = path.stat().st_size
    if size > MAX_PROJECT_FILE_BYTES:
        raise ValueError(f"{path} is {size} bytes, past {MAX_PROJECT_FILE_BYTES}; refusing to parse")
    with path.open("rb") as handle:
        data = handle.read(MAX_PROJECT_FILE_BYTES + 1)
    if len(data) > MAX_PROJECT_FILE_BYTES:
        raise ValueError(f"{path} grew past {MAX_PROJECT_FILE_BYTES} bytes; refusing to parse")
    return data.decode("utf-8", errors="replace")


def snapshot_from_project(project: ReaperProject, text_digest_source: Path) -> ProjectSnapshot:
    """Map a ReaperProject onto the shared ProjectSnapshot the differ consumes.

    Clip positions are converted from seconds to beats with the project's base
    tempo; a tempo envelope, when present, makes that conversion approximate.
    """
    bpm = project.tempo_bpm if project.tempo_bpm else 120.0
    beats_per_second = bpm / 60.0

    track_infos: list[TrackInfo] = []
    clip_hashes: set[str] = set()
    slot_hashes: dict[tuple[str, str], str] = {}
    device_hashes: set[str] = set()
    sample_refs: set[str] = set()

    for track in project.tracks:
        clips: list[ClipInfo] = []
        track_samples: list[str] = []
        for item in track.items:
            clip_hashes.add(item.digest)
            slot_hashes[(track.track_id, item.item_id)] = item.digest
            files = [s.file for s in item.sources if s.file]
            for f in files:
                sample_refs.add(f)
                if f not in track_samples:
                    track_samples.append(f)
            is_midi = bool(item.sources) and all(s.source_type == "MIDI" for s in item.sources)
            clips.append(
                ClipInfo(
                    name=item.name,
                    position_beats=item.position_seconds * beats_per_second,
                    length_beats=item.length_seconds * beats_per_second,
                    sample_ref=files[0] if files else "",
                    warp_on=False,
                    is_midi=is_midi,
                )
            )
        device_hashes |= track.fx_digests
        track_infos.append(
            TrackInfo(
                track_id=track.track_id,
                name=track.name,
                track_type="FolderTrack" if track.is_folder else "Track",
                devices=track.devices,
                device_presets=tuple("" for _ in track.devices),
                sample_paths=tuple(track_samples),
                clips=tuple(clips),
                clip_count=len(track.items),
                automation_point_count=track.envelope_points,
                midi_note_count=track.midi_note_ons,
                device_chain_hashes=track.fx_digests,
                group_id="",
                routing_input="",
                routing_output="",
                sends=(),
                is_frozen=False,
                color_index=-1,
            )
        )

    return ProjectSnapshot(
        file_hash=sha256_file(text_digest_source),
        file_size_bytes=text_digest_source.stat().st_size,
        track_count=len(track_infos),
        track_names=tuple(t.name for t in track_infos),
        tracks=tuple(track_infos),
        clip_count=sum(t.clip_count for t in track_infos),
        clip_hashes=frozenset(clip_hashes),
        clip_slot_hashes=slot_hashes,
        device_chain_hashes=frozenset(device_hashes),
        automation_point_count=sum(t.automation_point_count for t in track_infos),
        midi_note_count=sum(t.midi_note_count for t in track_infos),
        sample_refs=frozenset(sample_refs),
        transport_bpm=bpm,
        transport_time_signature=project.time_signature or (4, 4),
        transport_loop_on=project.loop_on,
        transport_loop_range=(0.0, 0.0),
        locator_count=project.markers,
        sample_rate=project.sample_rate,
        project_format="reaper_rpp",
    )


def extract_reaper_snapshot(path: Path) -> ProjectSnapshot:
    return snapshot_from_project(extract_reaper_project(read_rpp_text(path)), path)
