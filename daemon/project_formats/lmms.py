"""LMMS project (.mmp / .mmpz) parser.

Layout follows LMMS's own source (GPL-2+, https://github.com/LMMS/lmms):

  DataFile.cpp    <lmms-project version type creator creatorversion> with <head>
                  and the content element named for the type ("song"); the
                  document carries a bodyless `<!DOCTYPE lmms-project>`.
                  `loadData` tries plain XML first and falls back to qUncompress.
  Song.cpp        head@bpm, head@timesig_numerator/denominator
  Timeline.cpp    <timeline lp0pos lp1pos lpstate> in ticks (loop range)
  Track.cpp       <track type name muted solo> containing <instrumenttrack>,
                  <sampletrack>, <patterntrack> or <automationtrack> settings
                  and the track's clips as sibling elements
  MidiClip.cpp    <midiclip pos len name> with <note key vol pan len pos>
  SampleClip.cpp  <sampleclip pos len src>
  AutomationClip  <automationclip pos len name> with <time pos value ...>
  EffectChain.cpp <fxchain><effect name=...>; InstrumentTrack.cpp <instrument name>
  AudioFileProcessor.cpp  <audiofileprocessor src> inside <instrument> (sampler file)

`qCompress` output is a 4-byte big-endian uncompressed length followed by a zlib
stream; the declared length and the real output are both capped. Positions are
ticks: 192 per 4/4 bar, so 48 per quarter note (TimePos.h DefaultTicksPerBar).
Legacy element names (`pattern`, `sampletco`, `automationpattern`) are accepted.

The input is untrusted: the only DOCTYPE accepted is the bodyless `lmms-project` one.
"""

from __future__ import annotations

import struct
from pathlib import Path
from xml.etree import ElementTree as ET

from . import _safe
from ._snapshot import NeutralClip, NeutralProject, NeutralTrack, build_snapshot, canonical_element, digest_of

TICKS_PER_BEAT = 48
LMMS_DOCTYPE = "lmms-project"

_TRACK_TYPES = {
    0: "InstrumentTrack",
    1: "PatternTrack",
    2: "SampleTrack",
    3: "EventTrack",
    4: "VideoTrack",
    5: "AutomationTrack",
    6: "HiddenAutomationTrack",
}
_MIDI_CLIPS = ("midiclip", "pattern")
_SAMPLE_CLIPS = ("sampleclip", "sampletco")
_AUTOMATION_CLIPS = ("automationclip", "automationpattern")
_PATTERN_CLIPS = ("bbtco",)  # legacy name of the pattern-track clip
_SETTINGS = ("instrumenttrack", "sampletrack", "patterntrack", "bbtrack", "automationtrack")


def _int(value: str | None, default: int = 0) -> int:
    try:
        return int(float(value))  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return default


def _float(value: str | None) -> float:
    try:
        return float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return 0.0


def load_lmms_xml(data: bytes) -> ET.Element:
    """Plain XML first, then qUncompress, mirroring DataFile::loadData."""
    stripped = data.lstrip()
    if stripped.startswith(b"<"):
        return _safe.parse_xml(data, allowed_doctype=LMMS_DOCTYPE)
    if len(data) < 5:
        raise ValueError("not an LMMS project")
    (declared,) = struct.unpack(">I", data[:4])
    if declared > _safe.MAX_XML_BYTES:
        raise ValueError(f"LMMS project declares {declared} bytes, past {_safe.MAX_XML_BYTES}; refusing to parse")
    inflated = _safe.inflate_zlib(data[4:], min(declared, _safe.MAX_XML_BYTES))
    if len(inflated) != declared:
        raise ValueError("LMMS compressed length does not match its header")
    return _safe.parse_xml(inflated, allowed_doctype=LMMS_DOCTYPE)


def _head_value(head: ET.Element | None, name: str) -> str | None:
    """Model values are attributes, or a child element carrying `value` when automated."""
    if head is None:
        return None
    if head.get(name) is not None:
        return head.get(name)
    child = head.find(name)
    return child.get("value") if child is not None else None


def _clip_for(element: ET.Element, index: int) -> tuple[NeutralClip, int, int]:
    """Returns (clip, note_count, automation_points)."""
    tag = element.tag
    notes = sum(1 for child in element if child.tag == "note")
    points = sum(1 for child in element if child.tag == "time")
    sample = element.get("src", "") if tag in _SAMPLE_CLIPS else ""
    clip = NeutralClip(
        clip_id=f"clip-{index}",
        name=element.get("name", ""),
        position_beats=_int(element.get("pos")) / TICKS_PER_BEAT,
        length_beats=_int(element.get("len")) / TICKS_PER_BEAT,
        sample_ref=sample,
        is_midi=tag in _MIDI_CLIPS,
        digest=digest_of(canonical_element(element)),
    )
    return clip, notes, points


def _track(element: ET.Element, position: int) -> NeutralTrack:
    kind = _int(element.get("type"), -1)
    settings = next((c for c in element if c.tag in _SETTINGS), None)
    devices: list[str] = []
    hashes: list[str] = []
    instrument_samples: list[str] = []
    if settings is not None:
        instrument = settings.find("instrument")
        if instrument is not None:
            devices.append(f"instrument: {instrument.get('name', '')}")
            hashes.append(digest_of(canonical_element(instrument)))
            # AudioFileProcessor::saveSettings writes the loaded file as src on its own element.
            sampler = instrument.find("audiofileprocessor")
            if sampler is not None and sampler.get("src"):
                instrument_samples.append(sampler.get("src"))  # type: ignore[arg-type]
        chain = settings.find("fxchain")
        for effect in chain.findall("effect") if chain is not None else []:
            devices.append(f"effect: {effect.get('name', '')}")
            hashes.append(digest_of(canonical_element(effect)))
    clips: list[NeutralClip] = []
    note_total = point_total = 0
    for child in element:
        if child.tag in _MIDI_CLIPS + _SAMPLE_CLIPS + _AUTOMATION_CLIPS + _PATTERN_CLIPS:
            clip, notes, points = _clip_for(child, len(clips))
            clips.append(clip)
            note_total += notes
            point_total += points
    return NeutralTrack(
        track_id=f"pos-{position}",
        name=element.get("name", ""),
        track_type=_TRACK_TYPES.get(kind, "Track"),
        devices=tuple(devices),
        device_chain_hashes=frozenset({digest_of(hashes)}) if hashes else frozenset(),
        clips=clips,
        extra_sample_refs=tuple(instrument_samples),
        automation_points=point_total,
        midi_notes=note_total,
    )


def extract_lmms(data: bytes) -> NeutralProject:
    root = load_lmms_xml(data)
    if root.tag != "lmms-project":
        raise ValueError("not an LMMS project: root element is not <lmms-project>")
    if root.get("type") not in (None, "song"):
        raise ValueError(f"unsupported LMMS file type {root.get('type')!r}; only song projects are read")

    head = root.find("head")
    bpm = _float(_head_value(head, "bpm"))
    signature = (
        _int(_head_value(head, "timesig_numerator"), 4) or 4,
        _int(_head_value(head, "timesig_denominator"), 4) or 4,
    )

    tracks: list[NeutralTrack] = []
    loop_on = False
    loop_range = (0.0, 0.0)
    song = root.find("song")
    if song is not None:
        # Pattern tracks nest their own trackcontainer, so walk every <track> in
        # document order; depth is bounded by the XML depth cap.
        for index, element in enumerate(song.iter("track")):
            tracks.append(_track(element, index))
        timeline = song.find("timeline")
        if timeline is not None:
            loop_on = _int(timeline.get("lpstate")) == 1
            loop_range = (_int(timeline.get("lp0pos")) / TICKS_PER_BEAT, _int(timeline.get("lp1pos")) / TICKS_PER_BEAT)
    return NeutralProject(
        project_format="lmms",
        tracks=tracks,
        bpm=bpm,
        time_signature=signature,
        loop_on=loop_on,
        loop_range=loop_range,
    )


def extract_lmms_snapshot(path: Path):
    return build_snapshot(extract_lmms(_safe.read_project_bytes(path)), path)
