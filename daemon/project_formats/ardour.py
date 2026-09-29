"""Ardour session (.ardour) parser.

Ardour session files are XML written through its XMLTree. The layout is defined
by Ardour's own source (GPL-2+, https://github.com/Ardour/ardour, libs/ardour
and libs/temporal), which this parser follows:

  Session@version/name/sample-rate            Session::state
  Sources/Source@id,name,type,origin          Source::get_state, AudioFileSource::get_state
  Routes/Route@id,name,audio-playlist,
      midi-playlist, PresentationInfo@flags,
      Processor@type,unique-id (plug-ins)     Route::state, Track::state, PluginInsert::state
  Playlists/Playlist/Region@length,name,
      source-N,type                           Playlist::state, Region::state
  Locations/Location@flags,start,end          Location::get_state
  TempoMap@superclocks-per-second,
      Tempos/Tempo@npm,note-type,
      Meters/Meter@divisions-per-bar,note-value   TempoMap::get_state, Tempo::get_state

Timestamps are `timepos_t`/`timecnt_t` strings (libs/temporal/timeline.cc): `a<n>`
is superclock ticks, `b<n>` is musical ticks at 1920 per beat, and a bare
integer is a legacy sample count. A region's `length` is a timecnt
`<a|b><distance>@<a|b><position>`, so it carries both length and position.

Not extracted: automation lists, MIDI note counts (MIDI data lives in external
files), and plug-in state. Tempo changes are ignored; beats are derived from the
first tempo.

Input is untrusted: size, XML depth, element count and DTDs are bounded by
`_safe.parse_xml`.
"""

from __future__ import annotations

import re
from pathlib import Path
from xml.etree import ElementTree as ET

from . import _safe
from ._snapshot import NeutralClip, NeutralProject, NeutralTrack, build_snapshot, canonical_element, digest_of

TICKS_PER_BEAT = 1920  # libs/temporal/temporal/types.h ticks_per_beat

_TIMEPOS = re.compile(r"^([ab])(-?\d+)$")


def _int(value: str | None) -> int | None:
    try:
        return int(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return None


def _float(value: str | None) -> float | None:
    try:
        return float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return None


class _Clock:
    def __init__(self, superclock_rate: int | None, sample_rate: int | None, bpm: float) -> None:
        self.superclock_rate = superclock_rate
        self.sample_rate = sample_rate
        self.bpm = bpm

    def beats(self, text: str | None) -> float:
        """Convert a timepos string to beats; unparseable input becomes 0.0."""
        if not text:
            return 0.0
        text = text.strip()
        match = _TIMEPOS.match(text)
        if match:
            amount = int(match.group(2))
            if match.group(1) == "b":
                return amount / TICKS_PER_BEAT
            if self.superclock_rate:
                return amount / self.superclock_rate * self.bpm / 60.0
            return 0.0
        legacy = _int(text)
        if legacy is not None and self.sample_rate:
            return legacy / self.sample_rate * self.bpm / 60.0
        return 0.0

    def length_and_position(self, text: str | None) -> tuple[float, float]:
        """Split a timecnt `<distance>@<position>` into (length, position) in beats."""
        if not text:
            return 0.0, 0.0
        distance, _, position = text.partition("@")
        return self.beats(distance), self.beats(position)


def _tempo(root: ET.Element) -> tuple[float, tuple[int, int], int | None]:
    tempo_map = root.find("TempoMap")
    if tempo_map is None:
        return 0.0, (4, 4), None
    rate = _int(tempo_map.get("superclocks-per-second"))
    bpm = 0.0
    tempo = tempo_map.find("Tempos/Tempo")
    if tempo is not None:
        npm = _float(tempo.get("npm")) or 0.0
        note_type = _float(tempo.get("note-type")) or 4.0
        bpm = npm * 4.0 / note_type if note_type > 0 else 0.0
    meter = tempo_map.find("Meters/Meter")
    signature = (4, 4)
    if meter is not None:
        beats_per_bar = _float(meter.get("divisions-per-bar"))
        note_value = _float(meter.get("note-value"))
        if beats_per_bar and note_value and beats_per_bar > 0 and note_value > 0:
            signature = (int(beats_per_bar), int(note_value))
    return bpm, signature, rate if rate and rate > 0 else None


def _region_clips(playlist: ET.Element, sources: dict[str, str], clock: _Clock) -> list[NeutralClip]:
    clips: list[NeutralClip] = []
    for index, region in enumerate(playlist.findall("Region")):
        length, position = clock.length_and_position(region.get("length"))
        if region.get("length", "")[:1].isdigit() and region.get("position"):
            position = clock.beats(region.get("position"))  # legacy layout stored position separately
        source_id = region.get("source-0", "")
        clips.append(
            NeutralClip(
                clip_id=region.get("id") or f"pos-{index}",
                name=region.get("name", ""),
                position_beats=position,
                length_beats=length,
                sample_ref=sources.get(source_id, ""),
                is_midi=(region.get("type") or playlist.get("type") or "") == "midi",
                digest=digest_of(canonical_element(region, frozenset({"id"}))),
            )
        )
    return clips


def extract_ardour(data: bytes) -> NeutralProject:
    if data[:2] == b"\x1f\x8b":
        raise ValueError("compressed .ardour files are not supported")
    root = _safe.parse_xml(data)
    if root.tag != "Session":
        raise ValueError("not an Ardour session: root element is not <Session>")

    sample_rate = _int(root.get("sample-rate"))
    bpm, signature, rate = _tempo(root)
    clock = _Clock(rate, sample_rate if sample_rate and sample_rate > 0 else None, bpm)

    sources: dict[str, str] = {}
    for source in root.findall("Sources/Source"):
        if source.get("id") and source.get("type") in ("audio", None):
            sources[source.get("id")] = source.get("name", "")  # type: ignore[index]

    playlists = {p.get("id"): p for p in root.findall("Playlists/Playlist") if p.get("id")}

    tracks: list[NeutralTrack] = []
    for position, route in enumerate(root.findall("Routes/Route")):
        info = route.find("PresentationInfo")
        flags = info.get("flags", "") if info is not None else ""
        playlist_ids = [route.get("audio-playlist"), route.get("midi-playlist")]
        is_track = any(playlist_ids)
        clips: list[NeutralClip] = []
        for pid in playlist_ids:
            playlist = playlists.get(pid) if pid else None
            if playlist is not None:
                clips.extend(_region_clips(playlist, sources, clock))
        devices: list[str] = []
        hashes: list[str] = []
        for processor in route.findall("Processor"):
            unique_id = processor.get("unique-id")
            if unique_id:  # only PluginInsert::state writes unique-id
                devices.append(f"{processor.get('type', '')}: {processor.get('name', '')}")
                hashes.append(digest_of(processor.get("type"), unique_id, processor.get("active")))
        tracks.append(
            NeutralTrack(
                track_id=route.get("id") or f"pos-{position}",
                name=route.get("name", ""),
                track_type=("Track" if is_track else "Bus") if not flags else flags,
                devices=tuple(devices),
                device_chain_hashes=frozenset({digest_of(hashes)}) if hashes else frozenset(),
                clips=clips,
            )
        )

    locators = 0
    loop = (0.0, 0.0)
    loop_on = False
    for location in root.findall("Locations/Location"):
        flags = {f.strip() for f in location.get("flags", "").split(",")}
        if flags & {"IsMark", "IsRangeMarker"}:
            locators += 1
        if "IsAutoLoop" in flags:
            loop_on = True
            loop = (clock.beats(location.get("start")), clock.beats(location.get("end")))

    return NeutralProject(
        project_format="ardour",
        tracks=tracks,
        bpm=bpm,
        time_signature=signature,
        loop_on=loop_on,
        loop_range=loop,
        locators=locators,
        sample_rate=clock.sample_rate,
    )


def extract_ardour_snapshot(path: Path):
    return build_snapshot(extract_ardour(_safe.read_project_bytes(path)), path)
