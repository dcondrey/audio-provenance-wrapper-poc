"""Format-neutral project model and its mapping onto the shared ProjectSnapshot.

Parsers fill NeutralProject; `build_snapshot` produces exactly the snapshot
shape the REAPER parser feeds the project watcher. `snapshot_to_golden` is the
deterministic JSON form committed next to each fixture.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any
from xml.etree import ElementTree as ET

from daemon.common import sha256_file
from daemon.project_differ.differ import ClipInfo, ProjectSnapshot, TrackInfo


def digest_of(*parts: Any) -> str:
    """Stable 16-hex fingerprint of JSON-serialisable parts."""
    blob = json.dumps(parts, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(blob.encode("utf-8")).hexdigest()[:16]


def canonical_element(element: ET.Element, ignore_attrs: frozenset[str] = frozenset()) -> list[Any]:
    """Iterative canonical form of an XML subtree: tag, sorted attributes, text, children."""
    root: list[Any] = []
    stack: list[tuple[ET.Element, list[Any]]] = [(element, root)]
    while stack:
        node, out = stack.pop()
        attrs = sorted((k, v) for k, v in node.attrib.items() if k not in ignore_attrs)
        children: list[Any] = []
        out.extend([node.tag, attrs, (node.text or "").strip(), children])
        for child in node:
            slot: list[Any] = []
            children.append(slot)
            stack.append((child, slot))
    return root


@dataclass
class NeutralClip:
    clip_id: str
    name: str
    position_beats: float
    length_beats: float
    sample_ref: str = ""
    is_midi: bool = False
    warp_on: bool = False
    digest: str = ""


@dataclass
class NeutralTrack:
    track_id: str
    name: str
    track_type: str = "Track"
    devices: tuple[str, ...] = ()
    device_presets: tuple[str, ...] = ()
    device_chain_hashes: frozenset[str] = frozenset()
    clips: list[NeutralClip] = field(default_factory=list)
    extra_sample_refs: tuple[str, ...] = ()
    automation_points: int = 0
    midi_notes: int = 0
    group_id: str = ""


@dataclass
class NeutralProject:
    project_format: str
    tracks: list[NeutralTrack]
    bpm: float = 0.0
    time_signature: tuple[int, int] = (4, 4)
    loop_on: bool = False
    loop_range: tuple[float, float] = (0.0, 0.0)
    locators: int = 0
    sample_rate: int | None = None


def build_snapshot(project: NeutralProject, source: Path) -> ProjectSnapshot:
    clip_hashes: set[str] = set()
    slot_hashes: dict[tuple[str, str], str] = {}
    device_hashes: set[str] = set()
    sample_refs: set[str] = set()
    infos: list[TrackInfo] = []

    for track in project.tracks:
        clips: list[ClipInfo] = []
        track_samples: list[str] = []
        for clip in track.clips:
            clip_hashes.add(clip.digest)
            slot_hashes[(track.track_id, clip.clip_id)] = clip.digest
            if clip.sample_ref and clip.sample_ref not in track_samples:
                track_samples.append(clip.sample_ref)
            clips.append(
                ClipInfo(
                    name=clip.name,
                    position_beats=clip.position_beats,
                    length_beats=clip.length_beats,
                    sample_ref=clip.sample_ref,
                    warp_on=clip.warp_on,
                    is_midi=clip.is_midi,
                )
            )
        for ref in track.extra_sample_refs:
            if ref not in track_samples:
                track_samples.append(ref)
        sample_refs.update(track_samples)
        device_hashes |= track.device_chain_hashes
        presets = track.device_presets or tuple("" for _ in track.devices)
        infos.append(
            TrackInfo(
                track_id=track.track_id,
                name=track.name,
                track_type=track.track_type,
                devices=track.devices,
                device_presets=presets,
                sample_paths=tuple(track_samples),
                clips=tuple(clips),
                clip_count=len(clips),
                automation_point_count=track.automation_points,
                midi_note_count=track.midi_notes,
                device_chain_hashes=track.device_chain_hashes,
                group_id=track.group_id,
                routing_input="",
                routing_output="",
                sends=(),
                is_frozen=False,
                color_index=-1,
            )
        )

    return ProjectSnapshot(
        file_hash=sha256_file(source),
        file_size_bytes=source.stat().st_size,
        track_count=len(infos),
        track_names=tuple(t.name for t in infos),
        tracks=tuple(infos),
        clip_count=sum(t.clip_count for t in infos),
        clip_hashes=frozenset(clip_hashes),
        clip_slot_hashes=slot_hashes,
        device_chain_hashes=frozenset(device_hashes),
        automation_point_count=sum(t.automation_point_count for t in infos),
        midi_note_count=sum(t.midi_note_count for t in infos),
        sample_refs=frozenset(sample_refs),
        transport_bpm=project.bpm,
        transport_time_signature=project.time_signature,
        transport_loop_on=project.loop_on,
        transport_loop_range=project.loop_range,
        locator_count=project.locators,
        sample_rate=project.sample_rate,
        project_format=project.project_format,
    )


def _clip_golden(clip: ClipInfo) -> dict[str, Any]:
    return {
        "name": clip.name,
        "position_beats": clip.position_beats,
        "length_beats": clip.length_beats,
        "sample_ref": clip.sample_ref,
        "warp_on": clip.warp_on,
        "is_midi": clip.is_midi,
    }


def _track_golden(track: TrackInfo) -> dict[str, Any]:
    return {
        "track_id": track.track_id,
        "name": track.name,
        "track_type": track.track_type,
        "devices": list(track.devices),
        "device_presets": list(track.device_presets),
        "sample_paths": list(track.sample_paths),
        "clips": [_clip_golden(c) for c in track.clips],
        "clip_count": track.clip_count,
        "automation_point_count": track.automation_point_count,
        "midi_note_count": track.midi_note_count,
        "device_chain_hashes": sorted(track.device_chain_hashes),
        "group_id": track.group_id,
    }


def snapshot_to_golden(snapshot: ProjectSnapshot) -> dict[str, Any]:
    """Deterministic, path-free and timestamp-free JSON form of a snapshot.

    file_hash covers the committed fixture bytes, so it is stable and lets a
    port confirm it parsed the same file.
    """
    return {
        "project_format": snapshot.project_format,
        "file_hash": snapshot.file_hash,
        "file_size_bytes": snapshot.file_size_bytes,
        "track_count": snapshot.track_count,
        "track_names": list(snapshot.track_names),
        "tracks": [_track_golden(t) for t in snapshot.tracks],
        "clip_count": snapshot.clip_count,
        "clip_hashes": sorted(snapshot.clip_hashes),
        "clip_slot_hashes": [[t, c, h] for (t, c), h in sorted(snapshot.clip_slot_hashes.items())],
        "device_chain_hashes": sorted(snapshot.device_chain_hashes),
        "automation_point_count": snapshot.automation_point_count,
        "midi_note_count": snapshot.midi_note_count,
        "sample_refs": sorted(snapshot.sample_refs),
        "transport_bpm": snapshot.transport_bpm,
        "transport_time_signature": list(snapshot.transport_time_signature),
        "transport_loop_on": snapshot.transport_loop_on,
        "transport_loop_range": list(snapshot.transport_loop_range),
        "locator_count": snapshot.locator_count,
        "sample_rate": snapshot.sample_rate,
    }


def golden_json(snapshot: ProjectSnapshot) -> str:
    return json.dumps(snapshot_to_golden(snapshot), indent=2, sort_keys=True, ensure_ascii=False) + "\n"
