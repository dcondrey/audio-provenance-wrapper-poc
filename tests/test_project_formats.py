import json
import tempfile
import unittest
from pathlib import Path

from daemon.project_differ.differ import compute_diff, diff_to_event
from daemon.project_formats import (
    UnsupportedProjectFormat,
    detect_format,
    parse_project,
    registered_formats,
    unsupported_format_event,
)
from daemon.project_formats import reaper

FIXTURES = Path(__file__).parent / "fixtures" / "reaper"


class ReaperParserTests(unittest.TestCase):
    def setUp(self):
        self.project = reaper.extract_reaper_project((FIXTURES / "basic.rpp").read_text())

    def test_project_level_fields(self):
        self.assertEqual(self.project.sample_rate, 48000)
        self.assertEqual(self.project.tempo_bpm, 90.0)
        self.assertEqual(self.project.time_signature, (3, 4))
        self.assertTrue(self.project.loop_on)
        self.assertTrue(self.project.tempo_envelope)
        self.assertEqual((self.project.markers, self.project.regions), (1, 2))
        self.assertEqual([t.name for t in self.project.tracks], ["Drums", "Kick and Snare"])

    def test_track_structure(self):
        drums, kick = self.project.tracks
        self.assertTrue(drums.is_folder)
        self.assertEqual(drums.devices, ("VST3: ReaEQ (Cockos)", "utility/gain"))
        self.assertEqual(drums.envelope_points, 3)
        self.assertEqual(len(drums.items), 0)
        self.assertEqual(len(kick.items), 3)
        self.assertEqual(kick.midi_note_ons, 2)  # velocity-0 note-on and note-off excluded

    def test_items_and_sources(self):
        kick = self.project.tracks[1]
        first, midi, section = kick.items
        self.assertEqual((first.name, first.position_seconds, first.length_seconds), ("kick loop", 1.5, 3.0))
        self.assertEqual(first.sources[0].file, "Media/kick loop.wav")
        self.assertEqual(midi.sources[0].source_type, "MIDI")
        self.assertEqual(midi.item_id, "{22222222-0000-0000-0000-000000000002}")
        self.assertEqual(section.item_id, "{22222222-0000-0000-0000-000000000003}")
        self.assertEqual([s.source_type for s in section.sources], ["SECTION", "WAVE"])
        self.assertEqual(section.sources[1].file, "/abs/path/snare.flac")

    def test_snapshot_and_diff(self):
        before = parse_project(FIXTURES / "basic.rpp")
        after = parse_project(FIXTURES / "edited.rpp")
        self.assertEqual(before.project_format, "reaper_rpp")
        self.assertEqual(before.sample_rate, 48000)
        self.assertEqual(before.clip_count, 3)
        self.assertEqual(before.sample_refs, {"Media/kick loop.wav", "/abs/path/snare.flac"})
        # 1.5 s at 90 bpm = 2.25 beats
        clip = before.tracks[1].clips[0]
        self.assertAlmostEqual(clip.position_beats, 2.25)
        diff = compute_diff(before, after)
        self.assertEqual(diff.clips_added, 1)
        self.assertEqual(diff.clips_removed, 0)
        self.assertEqual(diff.clips_modified, 0)
        self.assertTrue(diff.bpm_changed)
        self.assertEqual(diff.samples_added, {"Media/new.wav"})
        event = diff_to_event(diff)
        self.assertNotIn("project_format", event)  # watcher adds it for non-.als only

    def test_rejects_non_rpp_and_malformed(self):
        for text in ("hello", "<OTHER\n>", "<REAPER_PROJECT 0.1\n  <TRACK\n", "<REAPER_PROJECT 0.1\n>\n>"):
            with self.assertRaises(ValueError, msg=text):
                reaper.parse_rpp(text)

    def test_depth_and_size_limits(self):
        deep = "<REAPER_PROJECT 0.1\n" + "<X\n" * reaper.MAX_DEPTH + ">\n" * (reaper.MAX_DEPTH + 1)
        with self.assertRaises(ValueError):
            reaper.parse_rpp(deep)
        ok = "<REAPER_PROJECT 0.1\n" + "<X\n" * (reaper.MAX_DEPTH - 1) + ">\n" * reaper.MAX_DEPTH
        reaper.parse_rpp(ok)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "big.rpp"
            path.write_bytes(b"<REAPER_PROJECT\n>\n")
            original = reaper.MAX_PROJECT_FILE_BYTES
            try:
                reaper.MAX_PROJECT_FILE_BYTES = path.stat().st_size - 1
                with self.assertRaises(ValueError):
                    reaper.read_rpp_text(path)
                reaper.MAX_PROJECT_FILE_BYTES = path.stat().st_size
                reaper.read_rpp_text(path)
            finally:
                reaper.MAX_PROJECT_FILE_BYTES = original

    def test_xml_entities_are_inert(self):
        text = '<REAPER_PROJECT 0.1\n  <TRACK {G}\n    NAME "&lt;!DOCTYPE x [<!ENTITY a "b">]&gt;"\n  >\n>\n'
        project = reaper.extract_reaper_project(text)
        self.assertIn("&lt;", project.tracks[0].name)


class RegistryTests(unittest.TestCase):
    UNSUPPORTED = {
        ".logicx": "Logic Pro",
        ".cpr": "Cubase/Nuendo",
        ".flp": "FL Studio",
        ".ptx": "Pro Tools",
        ".bwproject": "Bitwig Studio",
    }

    def test_supported_formats(self):
        self.assertTrue(detect_format(Path("a.als")).supported)
        self.assertTrue(detect_format(Path("A.RPP")).supported)
        self.assertIsNone(detect_format(Path("a.unknown")))

    def test_unsupported_formats_report_without_structure(self):
        for extension, host in self.UNSUPPORTED.items():
            fmt = detect_format(Path("song" + extension))
            self.assertFalse(fmt.supported, extension)
            self.assertEqual(fmt.host, host)
            with tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp) / ("song" + extension)
                path.write_bytes(b"\x00\x01 not parsed")
                with self.assertRaises(UnsupportedProjectFormat):
                    parse_project(path)
                event = unsupported_format_event(fmt, path)
            self.assertEqual(event["proof_level"], "unknown_unobserved")
            self.assertEqual(event["event_type"], "project_format_unsupported")
            self.assertEqual(set(event) & {"tracks_added", "clips_added", "file_hash"}, set())
            json.dumps(event)

    def test_logic_bundle_directory_is_detected_by_suffix(self):
        with tempfile.TemporaryDirectory() as tmp:
            bundle = Path(tmp) / "Song.logicx"
            bundle.mkdir()
            with self.assertRaises(UnsupportedProjectFormat):
                parse_project(bundle)

    def test_extensions_are_unique(self):
        seen = [e for f in registered_formats() for e in f.extensions]
        self.assertEqual(len(seen), len(set(seen)))

    def test_watcher_records_unsupported_and_returns(self):
        from daemon.project_differ.differ import ProjectWatcher

        with tempfile.TemporaryDirectory() as tmp:
            project = Path(tmp) / "x.cpr"
            project.write_bytes(b"binary")
            evidence = Path(tmp) / "out.jsonl"
            ProjectWatcher(project, evidence, poll_interval_seconds=0.01).run_forever()
            record = json.loads(evidence.read_text().splitlines()[0])
        self.assertEqual(record["project_format"], "cubase")
        self.assertEqual(record["proof_level"], "unknown_unobserved")


if __name__ == "__main__":
    unittest.main()
