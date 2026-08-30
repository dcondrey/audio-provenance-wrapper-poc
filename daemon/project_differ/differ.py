from __future__ import annotations

import argparse
import gzip
import hashlib
import logging
import time
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from pathlib import Path

from daemon.common import append_jsonl, sha256_file

log = logging.getLogger(__name__)


@dataclass(frozen=True)
class ClipInfo:
    """Per-clip metadata extracted from the .als XML."""

    name: str
    position_beats: float
    length_beats: float
    sample_ref: str
    warp_on: bool
    is_midi: bool


@dataclass(frozen=True)
class SendInfo:
    """Send/return routing for a track."""

    target: str
    level: float


@dataclass(frozen=True)
class TrackInfo:
    """Rich per-track metadata extracted from the .als XML."""

    # IMPORTANT: display names are not unique (Cmd+D duplicates share
    # EffectiveName); identity comparisons key on the XML Id attribute.
    track_id: str
    name: str
    track_type: str
    devices: tuple[str, ...]
    device_presets: tuple[str, ...]
    sample_paths: tuple[str, ...]
    clips: tuple[ClipInfo, ...]
    clip_count: int
    automation_point_count: int
    midi_note_count: int
    device_chain_hashes: frozenset[str]
    group_id: str
    routing_input: str
    routing_output: str
    sends: tuple[SendInfo, ...]
    is_frozen: bool
    color_index: int


@dataclass(frozen=True)
class ProjectSnapshot:
    """Structural fingerprint of an Ableton .als project at a point in time."""

    file_hash: str
    file_size_bytes: int
    track_count: int
    track_names: tuple[str, ...]
    tracks: tuple[TrackInfo, ...]
    clip_count: int
    clip_hashes: frozenset[str]
    clip_slot_hashes: dict[tuple[str, str], str]
    device_chain_hashes: frozenset[str]
    automation_point_count: int
    midi_note_count: int
    sample_refs: frozenset[str]
    transport_bpm: float
    transport_time_signature: tuple[int, int]
    transport_loop_on: bool
    transport_loop_range: tuple[float, float]
    locator_count: int


@dataclass
class ProjectDiff:
    """Structural diff between two project snapshots."""

    timestamp_ms: int
    previous_file_hash: str
    current_file_hash: str
    tracks_added: list[str] = field(default_factory=list)
    tracks_removed: list[str] = field(default_factory=list)
    clips_added: int = 0
    clips_removed: int = 0
    clips_modified: int = 0
    devices_changed: list[str] = field(default_factory=list)
    automation_points_delta: int = 0
    midi_notes_delta: int = 0
    samples_added: frozenset[str] = field(default_factory=frozenset)
    samples_removed: frozenset[str] = field(default_factory=frozenset)
    bpm_changed: bool = False
    loop_changed: bool = False
    locators_delta: int = 0

    def has_changes(self) -> bool:
        return (
            bool(self.tracks_added)
            or bool(self.tracks_removed)
            or self.clips_added > 0
            or self.clips_removed > 0
            or self.clips_modified > 0
            or bool(self.devices_changed)
            or self.automation_points_delta != 0
            or self.midi_notes_delta != 0
            or bool(self.samples_added)
            or bool(self.samples_removed)
            or self.bpm_changed
            or self.loop_changed
            or self.locators_delta != 0
        )


# A watched .als is untrusted input: bound the gzip expansion and refuse DTDs
# (no DOCTYPE means no entity expansion; Ableton never emits one).
MAX_ALS_DECOMPRESSED_BYTES = 256 * 1024 * 1024
_DOCTYPE_MARKER = b"<!DOCTYPE"


def parse_als(path: Path) -> ET.Element:
    """Decompress and parse an Ableton .als file.

    Raises on invalid, oversized, or DTD-carrying files.
    """
    parser = ET.XMLParser(target=ET.TreeBuilder())
    total = 0
    carry = b""
    with gzip.open(path, "rb") as f:
        while True:
            chunk = f.read(1024 * 1024)
            if not chunk:
                break
            total += len(chunk)
            if total > MAX_ALS_DECOMPRESSED_BYTES:
                raise ValueError(
                    f"{path} decompresses past {MAX_ALS_DECOMPRESSED_BYTES} bytes; refusing to parse"
                )
            if _DOCTYPE_MARKER in carry + chunk:
                raise ValueError(f"{path} contains a DTD declaration; refusing to parse")
            carry = chunk[-(len(_DOCTYPE_MARKER) - 1):]
            parser.feed(chunk)
    root = parser.close()
    if not isinstance(root, ET.Element):
        raise ValueError(f"{path} did not parse to an XML element")
    return root


def extract_snapshot(path: Path) -> ProjectSnapshot:
    """Extract a structural snapshot from an .als file.

    Stub: the XML traversal paths below reflect the known Ableton .als
    structure but are not exhaustively tested across versions.
    """
    root = parse_als(path)
    live_set = root.find("LiveSet")
    if live_set is None:
        raise ValueError(f"No LiveSet element in {path}")

    tracks_el = live_set.find("Tracks")
    track_names: list[str] = []
    clip_hashes: set[str] = set()
    device_hashes: set[str] = set()
    sample_refs: set[str] = set()
    clip_count = 0
    automation_count = 0
    midi_note_count = 0

    track_infos: list[TrackInfo] = []

    clip_slot_hashes: dict[tuple[str, str], str] = {}

    if tracks_el is not None:
        for track_index, track in enumerate(tracks_el):
            track_id = track.get("Id") or f"pos-{track_index}"
            name_el = track.find("Name")
            name_val = ""
            if name_el is not None:
                eff = name_el.find("EffectiveName")
                if eff is not None:
                    name_val = eff.get("Value", "")
            track_names.append(name_val)

            track_type = track.tag
            track_clips = 0
            track_auto = 0
            track_midi = 0
            track_devices: list[str] = []
            track_presets: list[str] = []
            track_samples: list[str] = []
            track_clip_infos: list[ClipInfo] = []
            track_device_hashes: set[str] = set()

            for slot_index, clip_slot in enumerate(track.iter("ClipSlot")):
                clip_count += 1
                track_clips += 1
                slot_hash = hashlib.sha256(ET.tostring(clip_slot)).hexdigest()[:16]
                clip_hashes.add(slot_hash)
                # Slot identity mirrors track identity: the element's own Id,
                # so a scene insert/delete does not shift every later slot's key.
                slot_id = clip_slot.get("Id") or f"pos-{slot_index}"
                clip_slot_hashes[(track_id, slot_id)] = slot_hash

                audio_clip_el = clip_slot.find(".//AudioClip")
                midi_clip_el = clip_slot.find(".//MidiClip")
                clip_el = audio_clip_el if audio_clip_el is not None else midi_clip_el
                if clip_el is not None:
                    clip_name_el = clip_el.find("Name")
                    clip_name = clip_name_el.get("Value", "") if clip_name_el is not None else ""
                    pos_el = clip_el.find("CurrentStart")
                    pos = float(pos_el.get("Value", "0")) if pos_el is not None else 0.0
                    end_el = clip_el.find("CurrentEnd")
                    end = float(end_el.get("Value", "0")) if end_el is not None else 0.0
                    warp_el = clip_el.find(".//WarpMode")
                    warp_on = warp_el is not None
                    clip_sample = ""
                    clip_file_ref = clip_el.find(".//FileRef/Path")
                    if clip_file_ref is not None:
                        clip_sample = clip_file_ref.get("Value", "")
                    track_clip_infos.append(ClipInfo(
                        name=clip_name,
                        position_beats=pos,
                        length_beats=end - pos,
                        sample_ref=clip_sample,
                        warp_on=warp_on,
                        is_midi=clip_el.tag == "MidiClip",
                    ))

            for device_chain in track.iter("DeviceChain"):
                chain_hash = hashlib.sha256(ET.tostring(device_chain)).hexdigest()[:16]
                device_hashes.add(chain_hash)
                track_device_hashes.add(chain_hash)
                devices_el = device_chain.find(".//Devices")
                if devices_el is not None:
                    for device in devices_el:
                        dev_name = device.tag
                        user_name_el = device.find(".//UserName")
                        if user_name_el is not None and user_name_el.get("Value"):
                            dev_name = user_name_el.get("Value", dev_name)
                        track_devices.append(dev_name)
                        preset_name = ""
                        preset_el = device.find(".//SelectedPresetName")
                        if preset_el is not None:
                            preset_name = preset_el.get("Value", "")
                        track_presets.append(preset_name)

            for file_ref in track.iter("FileRef"):
                rel = file_ref.find("RelativePath")
                if rel is not None:
                    val = rel.get("Value", "")
                    if val:
                        sample_refs.add(val)
                        track_samples.append(val)
                abs_el = file_ref.find("Path")
                if abs_el is not None:
                    val = abs_el.get("Value", "")
                    if val:
                        sample_refs.add(val)
                        if val not in track_samples:
                            track_samples.append(val)

            for notes_el in track.iter("Notes"):
                for key_track in notes_el.iter("KeyTrack"):
                    c = sum(1 for _ in key_track.iter("MidiNoteEvent"))
                    midi_note_count += c
                    track_midi += c

            track_auto = sum(1 for _ in track.iter("AutomationPoint"))
            automation_count += track_auto

            group_id = ""
            group_el = track.find("TrackGroupId")
            if group_el is not None:
                group_id = group_el.get("Value", "")

            routing_in = ""
            routing_out = ""
            input_routing = track.find(".//AudioInputRouting/Target")
            if input_routing is not None:
                routing_in = input_routing.get("Value", "")
            output_routing = track.find(".//AudioOutputRouting/Target")
            if output_routing is not None:
                routing_out = output_routing.get("Value", "")

            sends: list[SendInfo] = []
            for send_holder in track.iter("TrackSendHolder"):
                send_target = ""
                send_level = 0.0
                target_el = send_holder.find(".//Send/Target")
                if target_el is not None:
                    send_target = target_el.get("Value", "")
                level_el = send_holder.find(".//Send/Manual")
                if level_el is not None:
                    try:
                        send_level = float(level_el.get("Value", "0"))
                    except ValueError:
                        pass
                if send_target:
                    sends.append(SendInfo(target=send_target, level=send_level))

            frozen = False
            freeze_el = track.find("Freeze")
            if freeze_el is not None:
                frozen = freeze_el.get("Value", "false") == "true"

            color_index = -1
            color_el = track.find("ColorIndex")
            if color_el is not None:
                try:
                    color_index = int(color_el.get("Value", "-1"))
                except ValueError:
                    pass

            track_infos.append(TrackInfo(
                track_id=track_id,
                name=name_val,
                track_type=track_type,
                devices=tuple(track_devices),
                device_presets=tuple(track_presets),
                sample_paths=tuple(track_samples),
                clips=tuple(track_clip_infos),
                clip_count=track_clips,
                automation_point_count=track_auto,
                midi_note_count=track_midi,
                device_chain_hashes=frozenset(track_device_hashes),
                group_id=group_id,
                routing_input=routing_in,
                routing_output=routing_out,
                sends=tuple(sends),
                is_frozen=frozen,
                color_index=color_index,
            ))

    transport = live_set.find("Transport")
    bpm = 120.0
    loop_on = False
    loop_start = 0.0
    loop_length = 0.0
    if transport is not None:
        tempo_el = transport.find(".//Tempo/Manual")
        if tempo_el is not None:
            bpm = float(tempo_el.get("Value", "120"))
        loop_on_el = transport.find("LoopOn")
        if loop_on_el is not None:
            loop_on = loop_on_el.get("Value", "false") == "true"
        loop_start_el = transport.find("LoopStart")
        if loop_start_el is not None:
            loop_start = float(loop_start_el.get("Value", "0"))
        loop_len_el = transport.find("LoopLength")
        if loop_len_el is not None:
            loop_length = float(loop_len_el.get("Value", "0"))

    time_sig_num = 4
    time_sig_den = 4
    ts_el = live_set.find(".//TimeSignature")
    if ts_el is not None:
        num_el = ts_el.find(".//Numerator")
        den_el = ts_el.find(".//Denominator")
        if num_el is not None:
            try:
                time_sig_num = int(num_el.get("Value", "4"))
            except ValueError:
                pass
        if den_el is not None:
            try:
                time_sig_den = int(den_el.get("Value", "4"))
            except ValueError:
                pass

    locator_count = sum(1 for _ in live_set.iter("Locator"))

    stat = path.stat()

    return ProjectSnapshot(
        file_hash=sha256_file(path),
        file_size_bytes=stat.st_size,
        track_count=len(track_names),
        track_names=tuple(track_names),
        tracks=tuple(track_infos),
        clip_count=clip_count,
        clip_hashes=frozenset(clip_hashes),
        clip_slot_hashes=clip_slot_hashes,
        device_chain_hashes=frozenset(device_hashes),
        automation_point_count=automation_count,
        midi_note_count=midi_note_count,
        sample_refs=frozenset(sample_refs),
        transport_bpm=bpm,
        transport_time_signature=(time_sig_num, time_sig_den),
        transport_loop_on=loop_on,
        transport_loop_range=(loop_start, loop_length),
        locator_count=locator_count,
    )


def compute_diff(previous: ProjectSnapshot, current: ProjectSnapshot) -> ProjectDiff:
    # Identity comparisons key on track_id / (track_id, slot): display names
    # collide on duplicated tracks, and hash-set algebra reported every
    # single-clip edit as a delete+add pair with clips_modified stuck at 0.
    prev_by_id = {t.track_id: t for t in previous.tracks}
    curr_by_id = {t.track_id: t for t in current.tracks}

    prev_slots = previous.clip_slot_hashes
    curr_slots = current.clip_slot_hashes
    shared_slots = prev_slots.keys() & curr_slots.keys()
    modified_clips = sum(1 for key in shared_slots if prev_slots[key] != curr_slots[key])

    devices_changed = sorted(
        curr_by_id[track_id].name
        for track_id in curr_by_id.keys() & prev_by_id.keys()
        if curr_by_id[track_id].device_chain_hashes != prev_by_id[track_id].device_chain_hashes
    )

    return ProjectDiff(
        timestamp_ms=int(time.time() * 1000),
        previous_file_hash=previous.file_hash,
        current_file_hash=current.file_hash,
        tracks_added=sorted(
            curr_by_id[track_id].name for track_id in curr_by_id.keys() - prev_by_id.keys()
        ),
        tracks_removed=sorted(
            prev_by_id[track_id].name for track_id in prev_by_id.keys() - curr_by_id.keys()
        ),
        clips_added=len(curr_slots.keys() - prev_slots.keys()),
        clips_removed=len(prev_slots.keys() - curr_slots.keys()),
        clips_modified=modified_clips,
        devices_changed=devices_changed,
        automation_points_delta=current.automation_point_count - previous.automation_point_count,
        midi_notes_delta=current.midi_note_count - previous.midi_note_count,
        samples_added=current.sample_refs - previous.sample_refs,
        samples_removed=previous.sample_refs - current.sample_refs,
        bpm_changed=current.transport_bpm != previous.transport_bpm,
        loop_changed=(current.transport_loop_on != previous.transport_loop_on
                      or current.transport_loop_range != previous.transport_loop_range),
        locators_delta=current.locator_count - previous.locator_count,
    )


def diff_to_event(diff: ProjectDiff) -> dict[str, object]:
    """Serialize a diff as the project_diff evidence event both the daemon
    and the standalone watcher write."""
    return {
        "event_type": "project_diff",
        "proof_level": "inferred",
        "source_timestamp_ms": diff.timestamp_ms,
        "timestamp_ms": diff.timestamp_ms,
        "daemon_observed_monotonic_ms": int(time.monotonic_ns() // 1_000_000),
        "clips_added": diff.clips_added,
        "clips_removed": diff.clips_removed,
        "clips_modified": diff.clips_modified,
        "tracks_added": diff.tracks_added,
        "tracks_removed": diff.tracks_removed,
        "devices_changed": diff.devices_changed,
        "samples_added": sorted(diff.samples_added),
        "samples_removed": sorted(diff.samples_removed),
        "midi_notes_delta": diff.midi_notes_delta,
        "automation_points_delta": diff.automation_points_delta,
        "bpm_changed": diff.bpm_changed,
    }


class ProjectWatcher:
    """Watches an Ableton .als file for saves and emits structural diffs.

    Stub: the extract_snapshot parser still needs validation against real
    .als files across Ableton versions.
    """

    def __init__(
        self,
        project_path: Path,
        evidence_path: Path = Path("evidence/project_diff_events.jsonl"),
        poll_interval_seconds: float = 2.0,
    ) -> None:
        self.project_path = project_path.expanduser()
        self.evidence_path = evidence_path.expanduser()
        self.poll_interval_seconds = poll_interval_seconds
        self._previous_snapshot: ProjectSnapshot | None = None
        self._previous_mtime_ns: int = 0

    def run_forever(self) -> None:
        """Poll the project file and write a project_diff event on each save."""
        log.info("Watching %s for saves; writing %s", self.project_path, self.evidence_path)

        while True:
            try:
                stat = self.project_path.stat()
            except OSError:
                time.sleep(self.poll_interval_seconds)
                continue

            if stat.st_mtime_ns != self._previous_mtime_ns:
                self._previous_mtime_ns = stat.st_mtime_ns
                try:
                    snapshot = extract_snapshot(self.project_path)
                except Exception:
                    log.exception("Failed to parse %s", self.project_path)
                    time.sleep(self.poll_interval_seconds)
                    continue

                if self._previous_snapshot is not None:
                    diff = compute_diff(self._previous_snapshot, snapshot)
                    if diff.has_changes():
                        append_jsonl(self.evidence_path, diff_to_event(diff))
                        log.info(
                            "Project diff: +%d/-%d/%d~ clips, %d samples added",
                            diff.clips_added,
                            diff.clips_removed,
                            diff.clips_modified,
                            len(diff.samples_added),
                        )

                self._previous_snapshot = snapshot

            time.sleep(self.poll_interval_seconds)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Watch an Ableton .als project file and emit structural diffs.",
    )
    parser.add_argument("project_path", type=Path, help="Path to the .als file.")
    parser.add_argument(
        "--evidence-file",
        type=Path,
        default=Path("evidence/project_diff_events.jsonl"),
        help="JSONL output path.",
    )
    parser.add_argument(
        "--poll-interval",
        type=float,
        default=2.0,
        help="Seconds between file checks.",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")
    args = parse_args(argv)
    watcher = ProjectWatcher(
        project_path=args.project_path,
        evidence_path=args.evidence_file,
        poll_interval_seconds=args.poll_interval,
    )
    try:
        watcher.run_forever()
    except KeyboardInterrupt:
        return 0
    return 0
