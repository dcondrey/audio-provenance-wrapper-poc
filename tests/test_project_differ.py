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


class ComputeDiffTests(unittest.TestCase):
    def test_no_changes(self):
        snap = _make_snapshot()
        diff = compute_diff(snap, snap)
        self.assertFalse(diff.has_changes())

    def test_tracks_added(self):
        prev = _make_snapshot(tracks=(_make_track("Track 1", frozenset({"d1"})),))
        curr = _make_snapshot()
        diff = compute_diff(prev, curr)
        self.assertIn("Track 2", diff.tracks_added)

    def test_tracks_removed(self):
        prev = _make_snapshot()
        curr = _make_snapshot(tracks=(_make_track("Track 1", frozenset({"d1"})),))
        diff = compute_diff(prev, curr)
        self.assertIn("Track 2", diff.tracks_removed)

    def test_duplicate_named_track_deletion_is_reported(self):
        """Cmd+D duplicates share EffectiveName; deleting one must still
        surface in tracks_removed (identity is the XML Id, not the name)."""
        duplicated = (
            _make_track("Track 1", frozenset({"d1"}), track_id="10"),
            _make_track("Track 1", frozenset({"d1"}), track_id="11"),
        )
        prev = _make_snapshot(tracks=duplicated, track_names=("Track 1", "Track 1"))
        curr = _make_snapshot(tracks=duplicated[:1], track_names=("Track 1",))
        diff = compute_diff(prev, curr)
        self.assertEqual(diff.tracks_removed, ["Track 1"])
        self.assertEqual(diff.tracks_added, [])

    def test_clips_added(self):
        prev = _make_snapshot(clip_slot_hashes={("Track 1", "s0"): "c1", ("Track 1", "s1"): "c2"})
        curr = _make_snapshot(clip_slot_hashes={
            ("Track 1", "s0"): "c1", ("Track 1", "s1"): "c2", ("Track 2", "s0"): "c3",
        })
        diff = compute_diff(prev, curr)
        self.assertEqual(diff.clips_added, 1)
        self.assertEqual(diff.clips_removed, 0)

    def test_clips_removed(self):
        prev = _make_snapshot(clip_slot_hashes={
            ("Track 1", "s0"): "c1", ("Track 1", "s1"): "c2", ("Track 2", "s0"): "c3",
        })
        curr = _make_snapshot(clip_slot_hashes={("Track 1", "s0"): "c1", ("Track 1", "s1"): "c2"})
        diff = compute_diff(prev, curr)
        self.assertEqual(diff.clips_removed, 1)

    def test_single_clip_edit_is_a_modification_not_delete_plus_add(self):
        prev = _make_snapshot(clip_slot_hashes={("Track 1", "s0"): "c1", ("Track 1", "s1"): "c2"})
        curr = _make_snapshot(clip_slot_hashes={("Track 1", "s0"): "c1-edited", ("Track 1", "s1"): "c2"})
        diff = compute_diff(prev, curr)
        self.assertEqual(diff.clips_modified, 1)
        self.assertEqual(diff.clips_added, 0)
        self.assertEqual(diff.clips_removed, 0)

    def test_scene_deletion_does_not_misreport_trailing_clips(self):
        """Slots key on the ClipSlot Id, not list position, so removing one
        scene must not shift every later slot into a false modification."""
        prev = _make_snapshot(clip_slot_hashes={
            ("Track 1", "1"): "c1", ("Track 1", "2"): "c2", ("Track 1", "3"): "c3",
        })
        curr = _make_snapshot(clip_slot_hashes={
            ("Track 1", "2"): "c2", ("Track 1", "3"): "c3",
        })
        diff = compute_diff(prev, curr)
        self.assertEqual(diff.clips_removed, 1)
        self.assertEqual(diff.clips_modified, 0)
        self.assertEqual(diff.clips_added, 0)

    def test_samples_added(self):
        prev = _make_snapshot(sample_refs=frozenset({"a.wav"}))
        curr = _make_snapshot(sample_refs=frozenset({"a.wav", "b.wav"}))
        diff = compute_diff(prev, curr)
        self.assertIn("b.wav", diff.samples_added)
        self.assertTrue(diff.has_changes())

    def test_bpm_changed(self):
        prev = _make_snapshot(transport_bpm=120.0)
        curr = _make_snapshot(transport_bpm=140.0)
        diff = compute_diff(prev, curr)
        self.assertTrue(diff.bpm_changed)

    def test_midi_notes_delta(self):
        prev = _make_snapshot(midi_note_count=50)
        curr = _make_snapshot(midi_note_count=75)
        diff = compute_diff(prev, curr)
        self.assertEqual(diff.midi_notes_delta, 25)

    def test_devices_changed(self):
        prev = _make_snapshot(
            device_chain_hashes=frozenset({"d1"}),
            tracks=(
                _make_track("Track 1", frozenset({"d1"})),
                _make_track("Track 2", frozenset({"d2"})),
            ),
        )
        curr = _make_snapshot(
            device_chain_hashes=frozenset({"d1", "d3"}),
            tracks=(
                _make_track("Track 1", frozenset({"d3"})),
                _make_track("Track 2", frozenset({"d2"})),
            ),
        )
        diff = compute_diff(prev, curr)
        self.assertEqual(diff.devices_changed, ["Track 1"])

    def test_devices_changed_scoped_to_touched_track(self):
        """Only the track whose device chain actually changed is named,
        not every track, even though the project-wide hash sets differ."""
        prev = _make_snapshot(
            device_chain_hashes=frozenset({"d1", "d2"}),
            tracks=(
                _make_track("Track 1", frozenset({"d1"})),
                _make_track("Track 2", frozenset({"d2"})),
            ),
        )
        curr = _make_snapshot(
            device_chain_hashes=frozenset({"d1", "d3"}),
            tracks=(
                _make_track("Track 1", frozenset({"d1"})),
                _make_track("Track 2", frozenset({"d3"})),
            ),
        )
        diff = compute_diff(prev, curr)
        self.assertEqual(diff.devices_changed, ["Track 2"])


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


class ProjectDiffHasChangesTests(unittest.TestCase):
    def test_empty_diff(self):
        diff = ProjectDiff(timestamp_ms=0, previous_file_hash="a", current_file_hash="a")
        self.assertFalse(diff.has_changes())

    def test_any_nonzero_field_means_changes(self):
        diff = ProjectDiff(timestamp_ms=0, previous_file_hash="a", current_file_hash="b", clips_added=1)
        self.assertTrue(diff.has_changes())


if __name__ == "__main__":
    unittest.main()
