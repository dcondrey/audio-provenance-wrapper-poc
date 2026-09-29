"""DAWproject (.dawproject) parser.

DAWproject is the open exchange format authored by Bitwig
(https://github.com/bitwig/dawproject, MIT): a ZIP whose `project.xml` follows
Project.xsd. This parser reads only what that schema defines: Transport
(Tempo, TimeSignature), Structure (nested Track > Channel > Devices), and
Arrangement (Lanes > Clips > Clip, Notes, Audio/File, Markers, Points).

Time semantics follow the reference: a timeline's `timeUnit` applies to it and
its nested timelines, inherited from the parent, defaulting to beats. Seconds
are converted to beats with the project tempo (approximate under tempo
automation).

Only `project.xml` and, to fingerprint plug-in state, the `State@path` members
it names are read. Member names are validated, sizes are capped, and nothing is
extracted to disk.
"""

from __future__ import annotations

import hashlib
from pathlib import Path
from xml.etree import ElementTree as ET

from . import _safe
from ._snapshot import NeutralClip, NeutralProject, NeutralTrack, build_snapshot, canonical_element, digest_of

PROJECT_MEMBER = "project.xml"
MAX_TRACK_DEPTH = 32

_POINT_TAGS = frozenset(
    {"Point", "RealPoint", "BoolPoint", "EnumPoint", "IntegerPoint", "TimeSignaturePoint"}
)
_ROLE_TYPES = {"master": "MasterTrack", "effect": "ReturnTrack", "submix": "SubmixTrack", "vca": "VcaTrack"}


def _float(value: str | None, default: float = 0.0) -> float:
    try:
        return float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return default


def _int(value: str | None, default: int) -> int:
    try:
        return int(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return default


def _unit(element: ET.Element, inherited: str) -> str:
    unit = element.get("timeUnit")
    return unit if unit in ("beats", "seconds") else inherited


def _to_beats(value: float, unit: str, bpm: float) -> float:
    return value * bpm / 60.0 if unit == "seconds" else value


def _device_label(device: ET.Element) -> str:
    return f"{device.tag}: {device.get('deviceName') or device.get('name') or ''}"


def _state_digest(archive, device: ET.Element, names: set[str]) -> str:
    state = device.find("State")
    path = state.get("path") if state is not None else None
    if not path or state.get("external") == "true" or path not in names:  # type: ignore[union-attr]
        return ""
    _safe.check_member_name(path)
    return hashlib.sha256(_safe.read_member(archive, path, _safe.registry.MAX_PROJECT_FILE_BYTES)).hexdigest()[:16]


def _clip_content(clip: ET.Element) -> tuple[list[str], int, bool]:
    """Audio file paths, note count and whether the clip carries notes, at any nesting depth."""
    files: list[str] = []
    notes = 0
    for element in clip.iter():
        if element.tag == "Audio":
            file = element.find("File")
            if file is not None and file.get("path"):
                files.append(file.get("path"))  # type: ignore[arg-type]
        elif element.tag == "Note":
            notes += 1
    return files, notes, notes > 0 or any(e.tag == "Notes" for e in clip.iter())


def _arrangement_clips(lane: ET.Element, unit: str, bpm: float) -> tuple[list[NeutralClip], int, int]:
    """Outermost Clip elements under one track lane, plus points and notes beneath it."""
    clips: list[NeutralClip] = []
    points = sum(1 for e in lane.iter() if e.tag in _POINT_TAGS)
    notes = 0
    ordered: list[tuple[ET.Element, str]] = []

    def walk(node: ET.Element, node_unit: str) -> None:
        # Depth is bounded by _safe.MAX_XML_DEPTH. Clips nested in a Clip are its content.
        for child in node:
            if child.tag == "Clip":
                ordered.append((child, node_unit))
            else:
                walk(child, _unit(child, node_unit))

    walk(lane, unit)
    for index, (clip, clip_unit) in enumerate(ordered):
        files, note_count, has_notes = _clip_content(clip)
        notes += note_count
        clips.append(
            NeutralClip(
                clip_id=f"clip-{index}",
                name=clip.get("name", ""),
                position_beats=_to_beats(_float(clip.get("time")), clip_unit, bpm),
                length_beats=_to_beats(_float(clip.get("duration")), clip_unit, bpm),
                sample_ref=files[0] if files else "",
                is_midi=has_notes and not files,
                warp_on=any(e.tag == "Warps" for e in clip.iter()),
                digest=digest_of(canonical_element(clip, frozenset({"id"}))),
            )
        )
    return clips, points, notes


def _collect_tracks(
    parent: ET.Element, group: str, depth: int, out: list[tuple[ET.Element, str]]
) -> None:
    if depth > MAX_TRACK_DEPTH:
        raise ValueError(f"DAWproject track nesting deeper than {MAX_TRACK_DEPTH}; refusing to parse")
    for track in parent.findall("Track"):
        out.append((track, group))
        _collect_tracks(track, track.get("id", ""), depth + 1, out)


def extract_dawproject(path: Path) -> NeutralProject:
    archive = _safe.open_zip(_safe.read_project_bytes(path))
    with archive:
        names = {info.filename for info in archive.infolist()}
        if PROJECT_MEMBER not in names:
            raise ValueError("not a DAWproject: archive has no project.xml")
        root = _safe.parse_xml(_safe.read_member(archive, PROJECT_MEMBER))
        if root.tag != "Project":
            raise ValueError("not a DAWproject: project.xml root is not <Project>")

        transport = root.find("Transport")
        tempo = transport.find("Tempo") if transport is not None else None
        signature = transport.find("TimeSignature") if transport is not None else None
        bpm = _float(tempo.get("value")) if tempo is not None and tempo.get("unit") == "bpm" else 0.0
        time_signature = (
            _int(signature.get("numerator"), 4) if signature is not None else 4,
            _int(signature.get("denominator"), 4) if signature is not None else 4,
        )

        structure = root.find("Structure")
        flat: list[tuple[ET.Element, str]] = []
        if structure is not None:
            _collect_tracks(structure, "", 0, flat)

        arrangement = root.find("Arrangement")
        lanes_by_track: dict[str, list[tuple[ET.Element, str]]] = {}
        markers = 0
        if arrangement is not None:
            markers = sum(1 for e in arrangement.iter("Marker"))
            top_unit = "beats"
            stack = [(e, _unit(e, top_unit)) for e in arrangement.findall("Lanes")]
            while stack:
                lane, lane_unit = stack.pop()
                ref = lane.get("track")
                if ref:
                    lanes_by_track.setdefault(ref, []).append((lane, lane_unit))
                else:
                    stack.extend((c, _unit(c, lane_unit)) for c in lane.findall("Lanes"))

        tracks: list[NeutralTrack] = []
        for position, (track, group) in enumerate(flat):
            track_id = track.get("id") or f"pos-{position}"
            channel = track.find("Channel")
            role = channel.get("role", "regular") if channel is not None else "regular"
            devices: list[str] = []
            presets: list[str] = []
            hashes: list[str] = []
            if channel is not None:
                container = channel.find("Devices")
                for device in list(container) if container is not None else []:
                    devices.append(_device_label(device))
                    state = _state_digest(archive, device, names)
                    presets.append(state)
                    hashes.append(digest_of(canonical_element(device, frozenset({"id"})), state))
            clips: list[NeutralClip] = []
            points = notes = 0
            for lane, lane_unit in lanes_by_track.get(track.get("id", ""), []):
                lane_clips, lane_points, lane_notes = _arrangement_clips(lane, lane_unit, bpm)
                for clip in lane_clips:
                    clip.clip_id = f"clip-{len(clips)}"
                    clips.append(clip)
                points += lane_points
                notes += lane_notes
            tracks.append(
                NeutralTrack(
                    track_id=track_id,
                    name=track.get("name", ""),
                    track_type="FolderTrack"
                    if track.find("Track") is not None
                    else _ROLE_TYPES.get(role, "Track"),
                    devices=tuple(devices),
                    device_presets=tuple(presets),
                    device_chain_hashes=frozenset({digest_of(hashes)}) if hashes else frozenset(),
                    clips=clips,
                    automation_points=points,
                    midi_notes=notes,
                    group_id=group,
                )
            )

    return NeutralProject(
        project_format="dawproject",
        tracks=tracks,
        bpm=bpm,
        time_signature=time_signature,
        locators=markers,
    )


def extract_dawproject_snapshot(path: Path):
    return build_snapshot(extract_dawproject(path), path)
