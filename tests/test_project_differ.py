import gzip
import tempfile
import unittest
from pathlib import Path

from daemon.project_differ.differ import (
    ProjectDiff,
    ProjectSnapshot,
    TrackInfo,
    compute_diff,
    extract_snapshot,
)

_ALS_WITH_CHILDLESS_AUDIO_CLIP = b"""<?xml version="1.0"?>
<Ableton>
  <LiveSet>
    <Tracks>
      <AudioTrack>
        <Name><EffectiveName Value="Track 1"/></Name>
        <ClipSlot><AudioClip/></ClipSlot>
      </AudioTrack>
    </Tracks>
    <Transport/>
  </LiveSet>
</Ableton>
"""


def _make_track(
    name: str, device_chain_hashes: frozenset[str], track_id: str | None = None
) -> TrackInfo:
    return TrackInfo(
        track_id=track_id if track_id is not None else name,
        name=name,
        track_type="AudioTrack",
        devices=(),
        device_presets=(),
        sample_paths=(),
        clips=(),
        clip_count=0,
        automation_point_count=0,
        midi_note_count=0,
        device_chain_hashes=device_chain_hashes,
        group_id="",
        routing_input="",
        routing_output="",
        sends=(),
        is_frozen=False,
        color_index=-1,
    )


def _make_snapshot(**overrides) -> ProjectSnapshot:
    defaults = dict(
        file_hash="aaa",
        file_size_bytes=1000,
        track_count=2,
        track_names=("Track 1", "Track 2"),
        tracks=(
            _make_track("Track 1", frozenset({"d1"})),
            _make_track("Track 2", frozenset({"d2"})),
        ),
        clip_count=4,
        clip_hashes=frozenset({"c1", "c2", "c3", "c4"}),
        clip_slot_hashes={
            ("Track 1", "s0"): "c1", ("Track 1", "s1"): "c2",
            ("Track 2", "s0"): "c3", ("Track 2", "s1"): "c4",
        },
        device_chain_hashes=frozenset({"d1", "d2"}),
        automation_point_count=10,
        midi_note_count=50,
        sample_refs=frozenset({"sample_a.wav"}),
        transport_bpm=120.0,
        transport_time_signature=(4, 4),
        transport_loop_on=False,
        transport_loop_range=(0.0, 0.0),
        locator_count=2,
    )
    defaults.update(overrides)
    return ProjectSnapshot(**defaults)


def _slots(*keys: tuple[str, str]) -> dict[tuple[str, str], str]:
    return {key: f"c{i}" for i, key in enumerate(keys)}


class ComputeDiffTests(unittest.TestCase):
    def test_has_changes_reflects_any_nonzero_field(self):
        snap = _make_snapshot()
        self.assertFalse(compute_diff(snap, snap).has_changes())
        self.assertFalse(ProjectDiff(timestamp_ms=0, previous_file_hash="a", current_file_hash="a").has_changes())
        self.assertTrue(
            ProjectDiff(timestamp_ms=0, previous_file_hash="a", current_file_hash="b", clips_added=1).has_changes()
        )

    def test_tracks_added_and_removed(self):
        one = _make_snapshot(tracks=(_make_track("Track 1", frozenset({"d1"})),))
        two = _make_snapshot()
        self.assertEqual(compute_diff(one, two).tracks_added, ["Track 2"])
        self.assertEqual(compute_diff(two, one).tracks_removed, ["Track 2"])

    def test_duplicate_named_track_deletion_is_reported(self):
        # Cmd+D duplicates share EffectiveName; identity is the XML Id, not the name
        duplicated = (
            _make_track("Track 1", frozenset({"d1"}), track_id="10"),
            _make_track("Track 1", frozenset({"d1"}), track_id="11"),
        )
        prev = _make_snapshot(tracks=duplicated, track_names=("Track 1", "Track 1"))
        curr = _make_snapshot(tracks=duplicated[:1], track_names=("Track 1",))
        diff = compute_diff(prev, curr)
        self.assertEqual(diff.tracks_removed, ["Track 1"])
        self.assertEqual(diff.tracks_added, [])

    def test_clip_slot_changes_are_classified(self):
        s0, s1, other = ("Track 1", "s0"), ("Track 1", "s1"), ("Track 2", "s0")
        cases = {
            "added": (_slots(s0, s1), _slots(s0, s1, other), (1, 0, 0)),
            "removed": (_slots(s0, s1, other), _slots(s0, s1), (0, 1, 0)),
            "edit is a modification, not delete plus add": (
                {s0: "c1", s1: "c2"}, {s0: "c1-edited", s1: "c2"}, (0, 0, 1)),
            # slots key on the ClipSlot Id, so removing one scene must not shift later slots into false edits
            "scene deletion leaves trailing clips alone": (
                {("Track 1", "1"): "c1", ("Track 1", "2"): "c2", ("Track 1", "3"): "c3"},
                {("Track 1", "2"): "c2", ("Track 1", "3"): "c3"}, (0, 1, 0)),
        }
        for name, (prev, curr, expected) in cases.items():
            with self.subTest(name):
                diff = compute_diff(_make_snapshot(clip_slot_hashes=prev), _make_snapshot(clip_slot_hashes=curr))
                self.assertEqual((diff.clips_added, diff.clips_removed, diff.clips_modified), expected)

    def test_sample_tempo_and_note_changes(self):
        diff = compute_diff(
            _make_snapshot(sample_refs=frozenset({"a.wav"}), transport_bpm=120.0, midi_note_count=50),
            _make_snapshot(sample_refs=frozenset({"a.wav", "b.wav"}), transport_bpm=140.0, midi_note_count=75),
        )
        self.assertEqual(list(diff.samples_added), ["b.wav"])
        self.assertTrue(diff.bpm_changed)
        self.assertEqual(diff.midi_notes_delta, 25)

    def test_devices_changed_names_only_the_touched_track(self):
        def snapshot(first: str, second: str) -> ProjectSnapshot:
            return _make_snapshot(
                device_chain_hashes=frozenset({first, second}),
                tracks=(_make_track("Track 1", frozenset({first})), _make_track("Track 2", frozenset({second}))),
            )

        self.assertEqual(compute_diff(snapshot("d1", "d2"), snapshot("d3", "d2")).devices_changed, ["Track 1"])
        self.assertEqual(compute_diff(snapshot("d1", "d2"), snapshot("d1", "d3")).devices_changed, ["Track 2"])


class ExtractSnapshotTests(unittest.TestCase):
    def test_childless_audio_clip_is_not_dropped(self):
        """A child-less <AudioClip/> is a valid Element that is falsy under
        bool(); an `or`-based lookup treats it as absent and misclassifies
        the clip. It must be found via an explicit `is not None` check."""
        with tempfile.TemporaryDirectory() as tmp:
            als_path = Path(tmp) / "project.als"
            with gzip.open(als_path, "wb") as f:
                f.write(_ALS_WITH_CHILDLESS_AUDIO_CLIP)

            snapshot = extract_snapshot(als_path)

        self.assertEqual(snapshot.clip_count, 1)
        self.assertEqual(len(snapshot.tracks), 1)
        self.assertEqual(len(snapshot.tracks[0].clips), 1)
        self.assertFalse(snapshot.tracks[0].clips[0].is_midi)


if __name__ == "__main__":
    unittest.main()
