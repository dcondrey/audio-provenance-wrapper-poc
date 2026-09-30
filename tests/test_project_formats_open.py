"""Parsers for the open project formats: DAWproject, Ardour, LMMS, Pure Data, Max.

Fixture provenance (details and licences in docs/PROJECT_FORMATS.md):
  dawproject/basic.dawproject  project.xml as saved by Bitwig Studio 5.0, from the
                               MIT-licensed README at
                               https://github.com/bitwig/dawproject/blob/ee4dcdde75940f30e14e55401a26955a58b8322b/README.md
                               zipped here; not a real .dawproject file (marked constructed-only).
  puredata/A01.sinewave.pd     real example shipped with Pure Data (BSD), unmodified:
                               https://github.com/pure-data/pure-data/blob/43a5b8922a35b6f473f54ad336e643d3c04639ff/doc/3.audio.examples/A01.sinewave.pd
  ardour/basic.ardour          constructed from Ardour source (GPL, not vendored):
                               https://github.com/Ardour/ardour/tree/4e6e9fd887392d751742424568dc50c1cdb135cb/libs/ardour
                               and .../libs/temporal
  lmms/basic.mmp, basic.mmpz   constructed from LMMS source (GPL, not vendored):
                               https://github.com/LMMS/lmms/tree/a2f57e70ce9c3468b4b6d21955bbe65a0989048a/src/core
                               (plus src/tracks, plugins/AudioFileProcessor)
  maxpat/basic.maxpat          constructed from the structure of
                               https://github.com/Cycling74/max-sdk/blob/15b6fe17eedc7c8a8b4ee249706d3aaf21e192fa/help/dummy.maxhelp
                               (Max 7 patcher; no formal spec exists; file not vendored).
  milkytracker/test.xm, test.mod  the OpenMPT project's own test modules (BSD-3-Clause, unmodified):
                               https://github.com/OpenMPT/openmpt/tree/f83cedb0cd5446e4dfaa83ac97e3087107e26767/test
                               (licence text in LICENSE.third-party beside them). Real files.
  vcv/basic.vcv                constructed (tests/fixtures/projects/module_builders.py) from Rack v2.6.6
                               src/patch.cpp, engine/Module.cpp and engine/Cable.cpp (GPL, not vendored).
Each fixture has a golden snapshot beside it that the Rust port can reuse.
"""

import json
import struct
import tempfile
import unittest
import zipfile
import zlib
from pathlib import Path
from unittest import mock

from daemon.project_differ.differ import compute_diff
from daemon.project_formats import (
    CONSTRUCTED_ONLY,
    REAL_FILES,
    SUPPORTED,
    UNSUPPORTED,
    detect_format,
    parse_project,
    registered_formats,
    registry,
)
from daemon.project_formats import _safe, ardour, dawproject, lmms, maxpat, puredata
from daemon.project_formats._snapshot import snapshot_to_golden

FIXTURES = Path(__file__).parent / "fixtures" / "projects"
ALL_FIXTURES = (
    "dawproject/basic.dawproject",
    "ardour/basic.ardour",
    "lmms/basic.mmp",
    "lmms/basic.mmpz",
    "puredata/A01.sinewave.pd",
    "maxpat/basic.maxpat",
    "milkytracker/test.xm",
    "milkytracker/test.mod",
    "vcv/basic.vcv",
)


def make_zip(members, path: Path) -> Path:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, data in members:
            info = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data)
    return path


class TmpMixin(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.tmp = Path(self._tmp.name)

    def write(self, name: str, data) -> Path:
        path = self.tmp / name
        path.write_bytes(data if isinstance(data, bytes) else data.encode())
        return path


class GoldenTests(unittest.TestCase):
    def test_every_fixture_matches_its_golden_snapshot(self):
        for relative in ALL_FIXTURES:
            path = FIXTURES / relative
            golden = json.loads((path.parent / (path.name + ".golden.json")).read_text(encoding="utf-8"))
            with self.subTest(relative):
                self.assertEqual(snapshot_to_golden(parse_project(path)), golden)

    def test_goldens_carry_no_paths_or_timestamps(self):
        for relative in ALL_FIXTURES:
            path = FIXTURES / relative
            text = (path.parent / (path.name + ".golden.json")).read_text(encoding="utf-8")
            with self.subTest(relative):
                self.assertNotIn(str(FIXTURES), text)
                self.assertNotIn("timestamp", text)

    def test_size_cap_boundary_limit_minus_one_limit_plus_one(self):
        for relative in ALL_FIXTURES:
            path = FIXTURES / relative
            size = path.stat().st_size
            with self.subTest(relative):
                with mock.patch.object(registry, "MAX_PROJECT_FILE_BYTES", size - 1):
                    with self.assertRaises(ValueError):
                        parse_project(path)
                for limit in (size, size + 1):
                    with mock.patch.object(registry, "MAX_PROJECT_FILE_BYTES", limit):
                        self.assertEqual(parse_project(path).file_size_bytes, size)


class RegistryTests(unittest.TestCase):
    def test_supported_formats_state_how_they_were_validated(self):
        supported = {f.format_id: f for f in registered_formats() if f.status == SUPPORTED}
        for expected in ("dawproject", "ardour", "lmms", "pure_data", "max_patcher", "reaper_rpp", "milkytracker", "vcv_rack"):
            self.assertIn(expected, supported)
        for fmt in supported.values():
            self.assertIn(fmt.validation, (REAL_FILES, CONSTRUCTED_ONLY), fmt.format_id)
        self.assertEqual(supported["pure_data"].validation, REAL_FILES)
        self.assertEqual(supported["milkytracker"].validation, REAL_FILES)
        for constructed in ("dawproject", "ardour", "lmms", "max_patcher", "vcv_rack"):
            self.assertEqual(supported[constructed].validation, CONSTRUCTED_ONLY)

    def test_unsupported_formats_have_reasons_and_name_dawproject_where_exportable(self):
        unsupported = {f.format_id: f for f in registered_formats() if f.status == UNSUPPORTED}
        for fmt in unsupported.values():
            self.assertTrue(fmt.reason, fmt.format_id)
            self.assertIsNone(fmt.parser)
        for exports in ("cubase", "bitwig", "studio_one"):
            self.assertIn(".dawproject", unsupported[exports].reason)
        for fmt_id in ("renoise", "premiere", "sibelius", "dorico", "reaktor", "audiomulch"):
            self.assertIn(fmt_id, unsupported)
        self.assertEqual(detect_format(Path("x.vcv")).format_id, "vcv_rack")
        self.assertEqual(detect_format(Path("x.XM")).format_id, "milkytracker")
        self.assertEqual(detect_format(Path("x.mod")).format_id, "milkytracker")
        self.assertIn("no XSD", unsupported["renoise"].reason)

    def test_extensions_do_not_collide(self):
        seen = {}
        for fmt in registered_formats():
            for extension in fmt.extensions:
                self.assertNotIn(extension, seen)
                seen[extension] = fmt.format_id


class SafeHelperTests(TmpMixin):
    def test_member_name_rules(self):
        for bad in ("/etc/passwd", "../x", "a/../../x", "a\\..\\x", "C:/x", "c:\\x", "\\abs", "a\x00b", ""):
            with self.subTest(bad), self.assertRaises(ValueError):
                _safe.check_member_name(bad)
        for good in ("project.xml", "audio/a b.wav", "dir/..name/x"):
            _safe.check_member_name(good)

    def test_zip_member_count_boundary(self):
        members = [(f"m{i}", b"x") for i in range(5)]
        data = make_zip(members, self.tmp / "z.zip").read_bytes()
        for limit, ok in ((4, False), (5, True), (6, True)):
            with self.subTest(limit), mock.patch.object(_safe, "MAX_ZIP_MEMBERS", limit):
                if ok:
                    _safe.open_zip(data).close()
                else:
                    with self.assertRaises(ValueError):
                        _safe.open_zip(data)

    def test_zip_total_size_boundary(self):
        data = make_zip([("a", b"x" * 60), ("b", b"y" * 40)], self.tmp / "z.zip").read_bytes()
        for limit, ok in ((99, False), (100, True), (101, True)):
            with self.subTest(limit), mock.patch.object(_safe, "MAX_ZIP_TOTAL_BYTES", limit):
                if ok:
                    _safe.open_zip(data).close()
                else:
                    with self.assertRaises(ValueError):
                        _safe.open_zip(data)

    def test_member_read_cap_boundary(self):
        data = make_zip([("a", b"x" * 50)], self.tmp / "z.zip").read_bytes()
        with _safe.open_zip(data) as archive:
            with self.assertRaises(ValueError):
                _safe.read_member(archive, "a", 49)
            self.assertEqual(len(_safe.read_member(archive, "a", 50)), 50)
            self.assertEqual(len(_safe.read_member(archive, "a", 51)), 50)

    def test_duplicate_members_and_bad_archives_rejected(self):
        path = self.tmp / "dup.zip"
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr("a", b"1")
            with self.assertWarns(UserWarning):
                archive.writestr("a", b"2")
        with self.assertRaises(ValueError):
            _safe.open_zip(path.read_bytes())
        with self.assertRaises(ValueError):
            _safe.open_zip(b"not a zip")

    def test_zlib_bomb_is_bounded(self):
        bomb = zlib.compress(b"\0" * 5_000_000, 9)
        self.assertLess(len(bomb), 10_000)
        with self.assertRaises(ValueError):
            _safe.inflate_zlib(bomb, 1_000)
        exact = zlib.compress(b"a" * 100)
        for limit, ok in ((99, False), (100, True), (101, True)):
            if ok:
                self.assertEqual(len(_safe.inflate_zlib(exact, limit)), 100)
            else:
                with self.assertRaises(ValueError):
                    _safe.inflate_zlib(exact, limit)
        with self.assertRaises(ValueError):
            _safe.inflate_zlib(b"garbage")

    def test_xml_rejects_dtd_entities_and_external_references(self):
        laughs = (
            b'<?xml version="1.0"?><!DOCTYPE l [<!ENTITY a "aaaa"><!ENTITY b "&a;&a;&a;&a;">]><l>&b;</l>'
        )
        external = b'<?xml version="1.0"?><!DOCTYPE l SYSTEM "file:///etc/passwd"><l/>'
        bare = b"<!DOCTYPE lmms-project><lmms-project/>"
        for data in (laughs, external, bare):
            with self.subTest(data[:40]), self.assertRaises(ValueError):
                _safe.parse_xml(data)
        _safe.parse_xml(bare, allowed_doctype="lmms-project")
        with self.assertRaises(ValueError):
            _safe.parse_xml(laughs, allowed_doctype="l")
        with self.assertRaises(ValueError):
            _safe.parse_xml(b'<!DOCTYPE lmms-project SYSTEM "x"><a/>', allowed_doctype="lmms-project")

    def test_doctype_text_inside_cdata_or_comment_is_not_a_dtd(self):
        root = _safe.parse_xml(b"<a><![CDATA[<!DOCTYPE html>]]><!-- <!ENTITY x> --></a>")
        self.assertIn("<!DOCTYPE html>", root.text)

    def test_encoding_tricks_cannot_hide_a_dtd(self):
        text = '<?xml version="1.0" encoding="UTF-16"?><!DOCTYPE a [<!ENTITY x "y">]><a>&x;</a>'
        for encoding in ("utf-16", "utf-16-le", "utf-16-be"):
            with self.subTest(encoding), self.assertRaises(ValueError):
                _safe.parse_xml(text.encode(encoding))

    def test_xml_depth_and_element_boundaries(self):
        def nested(depth):
            return b"<a>" * depth + b"</a>" * depth

        with mock.patch.object(_safe, "MAX_XML_DEPTH", 10):
            _safe.parse_xml(nested(9))
            _safe.parse_xml(nested(10))
            with self.assertRaises(ValueError):
                _safe.parse_xml(nested(11))
        flat = lambda n: b"<r>" + b"<c/>" * (n - 1) + b"</r>"  # n elements in total
        with mock.patch.object(_safe, "MAX_XML_ELEMENTS", 20):
            _safe.parse_xml(flat(19))
            _safe.parse_xml(flat(20))
            with self.assertRaises(ValueError):
                _safe.parse_xml(flat(21))

    def test_xml_size_and_malformed(self):
        with mock.patch.object(_safe, "MAX_XML_BYTES", 10):
            _safe.parse_xml(b"<a></a>")
            with self.assertRaises(ValueError):
                _safe.parse_xml(b"<a>" + b"x" * 8 + b"</a>")
        for bad in (b"", b"<a>", b"not xml", b"<a></b>"):
            with self.subTest(bad), self.assertRaises(ValueError):
                _safe.parse_xml(bad)

    def test_json_bounds(self):
        with mock.patch.object(_safe, "MAX_JSON_DEPTH", 5):
            _safe.parse_json(b"[[[[1]]]]")
            _safe.parse_json(b"[[[[[1]]]]]")
            with self.assertRaises(ValueError):
                _safe.parse_json(b"[[[[[[1]]]]]]")
        with mock.patch.object(_safe, "MAX_JSON_NODES", 4):
            _safe.parse_json(b"[1,2,3]")
            with self.assertRaises(ValueError):
                _safe.parse_json(b"[1,2,3,4]")
        with self.assertRaises(ValueError):
            _safe.parse_json(b"[" * 200_000)
        with self.assertRaises(ValueError):
            _safe.parse_json(b"{not json")


class DawprojectTests(TmpMixin):
    def parse(self):
        return dawproject.extract_dawproject(FIXTURES / "dawproject/basic.dawproject")

    def test_transport_and_tracks(self):
        project = self.parse()
        self.assertEqual(project.bpm, 149.0)
        self.assertEqual(project.time_signature, (4, 4))
        self.assertEqual([t.name for t in project.tracks], ["Bass", "Drumloop", "Master"])
        bass, drums, master = project.tracks
        self.assertEqual(bass.devices, ("ClapPlugin: Surge XT",))
        self.assertEqual(bass.midi_notes, 9)
        self.assertEqual(master.track_type, "MasterTrack")

    def test_arrangement_clips_ignore_nested_audio_event_clips(self):
        drums = self.parse().tracks[1]
        self.assertEqual(len(drums.clips), 1)  # id25 is the arrangement clip, id26 its content
        clip = drums.clips[0]
        self.assertEqual(clip.name, "Drumfunk3 170bpm")
        self.assertEqual(clip.sample_ref, "audio/Drumfunk3 170bpm.wav")
        self.assertTrue(clip.warp_on)

    def test_seconds_timeline_converted_with_tempo(self):
        xml = (FIXTURES / "dawproject/project.xml").read_text().replace(
            '<Lanes timeUnit="beats" id="id20">', '<Lanes timeUnit="seconds" id="id20">'
        )
        path = make_zip([("project.xml", xml)], self.tmp / "s.dawproject")
        clip = dawproject.extract_dawproject(path).tracks[0].clips[0]
        self.assertAlmostEqual(clip.length_beats, 8.0 * 149.0 / 60.0)

    def test_nested_tracks_get_group_and_folder_type(self):
        xml = (
            '<Project version="1.0"><Application name="x" version="1"/><Structure>'
            '<Track id="g" name="Group"><Track id="c" name="Child"/></Track></Structure></Project>'
        )
        project = dawproject.extract_dawproject(make_zip([("project.xml", xml)], self.tmp / "n.dawproject"))
        group, child = project.tracks
        self.assertEqual((group.track_type, child.group_id), ("FolderTrack", "g"))

    def test_plugin_state_member_is_fingerprinted(self):
        base = (FIXTURES / "dawproject/project.xml").read_bytes()
        first = make_zip(
            [("project.xml", base), ("plugins/d19b1f6e-bbb6-42fe-a6c9-54b41d97a05d.clap-preset", b"one")],
            self.tmp / "a.dawproject",
        )
        second = make_zip(
            [("project.xml", base), ("plugins/d19b1f6e-bbb6-42fe-a6c9-54b41d97a05d.clap-preset", b"two")],
            self.tmp / "b.dawproject",
        )
        a, b = parse_project(first), parse_project(second)
        self.assertNotEqual(a.tracks[0].device_chain_hashes, b.tracks[0].device_chain_hashes)
        self.assertEqual(compute_diff(a, b).devices_changed, ["Bass"])

    def test_edit_is_visible_to_the_differ_and_ids_are_not_content(self):
        base = (FIXTURES / "dawproject/project.xml").read_text()
        before = parse_project(FIXTURES / "dawproject/basic.dawproject")
        renumbered = base.replace('id="id25"', 'id="zz25"')
        same = parse_project(make_zip([("project.xml", renumbered)], self.tmp / "r.dawproject"))
        self.assertEqual(compute_diff(before, same).clips_modified, 0)
        edited = base.replace('value="149.000000"', 'value="120.000000"').replace('key="65"', 'key="66"', 1)
        after = parse_project(make_zip([("project.xml", edited)], self.tmp / "e.dawproject"))
        diff = compute_diff(before, after)
        self.assertTrue(diff.bpm_changed)
        self.assertEqual(diff.clips_modified, 1)

    def test_rejects_bad_containers(self):
        cases = {
            "not_zip.dawproject": b"plain",
            "empty.dawproject": make_zip([], self.tmp / "e0.dawproject").read_bytes(),
            "no_project.dawproject": make_zip([("other.xml", "<Project/>")], self.tmp / "e1.dawproject").read_bytes(),
            "wrong_root.dawproject": make_zip([("project.xml", "<Other/>")], self.tmp / "e2.dawproject").read_bytes(),
            "bad_xml.dawproject": make_zip([("project.xml", "<Project>")], self.tmp / "e3.dawproject").read_bytes(),
        }
        for name, data in cases.items():
            with self.subTest(name), self.assertRaises(ValueError):
                parse_project(self.write(name, data))

    def test_rejects_traversal_absolute_and_dtd(self):
        good = (FIXTURES / "dawproject/project.xml").read_bytes()
        for name in ("../evil", "/abs/evil", "a\\..\\evil", "C:/evil"):
            path = make_zip([("project.xml", good), (name, b"x")], self.tmp / "t.dawproject")
            with self.subTest(name), self.assertRaises(ValueError):
                parse_project(path)
        dtd = b'<!DOCTYPE Project [<!ENTITY x "y">]><Project version="1.0"><Application name="&x;" version="1"/></Project>'
        with self.assertRaises(ValueError):
            parse_project(make_zip([("project.xml", dtd)], self.tmp / "d.dawproject"))

    def test_member_count_and_decompressed_size_limits(self):
        good = (FIXTURES / "dawproject/project.xml").read_bytes()
        path = make_zip([("project.xml", good), ("a", b"1"), ("b", b"2")], self.tmp / "c.dawproject")
        with mock.patch.object(_safe, "MAX_ZIP_MEMBERS", 2):
            with self.assertRaises(ValueError):
                parse_project(path)
        with mock.patch.object(_safe, "MAX_ZIP_MEMBERS", 3):
            parse_project(path)
        total = len(good) + 2
        with mock.patch.object(_safe, "MAX_ZIP_TOTAL_BYTES", total - 1):
            with self.assertRaises(ValueError):
                parse_project(path)
        with mock.patch.object(_safe, "MAX_ZIP_TOTAL_BYTES", total):
            parse_project(path)
        bomb = make_zip([("project.xml", b"<Project version='1'>" + b" " * 5_000_000 + b"</Project>")], self.tmp / "bomb.dawproject")
        self.assertLess(bomb.stat().st_size, 50_000)
        with mock.patch.object(_safe, "MAX_XML_BYTES", 1_000_000):
            with self.assertRaises(ValueError):
                parse_project(bomb)

    def test_project_xml_read_cap_boundary(self):
        good = (FIXTURES / "dawproject/project.xml").read_bytes()
        path = make_zip([("project.xml", good)], self.tmp / "b.dawproject")
        for limit, ok in ((len(good) - 1, False), (len(good), True), (len(good) + 1, True)):
            with self.subTest(limit), mock.patch.object(_safe, "MAX_XML_BYTES", limit):
                if ok:
                    parse_project(path)
                else:
                    with self.assertRaises(ValueError):
                        parse_project(path)

    def test_track_nesting_is_bounded(self):
        def nested(depth):
            return (
                '<Project version="1.0"><Application name="x" version="1"/><Structure>'
                + '<Track id="t">' * depth
                + "</Track>" * depth
                + "</Structure></Project>"
            )

        limit = dawproject.MAX_TRACK_DEPTH
        dawproject.extract_dawproject(make_zip([("project.xml", nested(limit))], self.tmp / "ok.dawproject"))
        with self.assertRaises(ValueError):
            dawproject.extract_dawproject(make_zip([("project.xml", nested(limit + 2))], self.tmp / "deep.dawproject"))


class ArdourTests(TmpMixin):
    def setUp(self):
        super().setUp()
        self.text = (FIXTURES / "ardour/basic.ardour").read_text()

    def test_tracks_regions_and_timing(self):
        project = ardour.extract_ardour(self.text.encode())
        self.assertEqual(project.bpm, 120.0)
        self.assertEqual(project.time_signature, (3, 4))
        self.assertEqual(project.sample_rate, 48000)
        master, drums, lead, bus = project.tracks
        self.assertEqual((master.track_type, bus.track_type), ("MasterOut", "AudioBus,OrderSet"))
        self.assertEqual(drums.devices, ("lv2: a-Reverb",))  # Amp has no unique-id, so it is not a plug-in
        kick, pad = drums.clips
        self.assertEqual((kick.position_beats, kick.length_beats, kick.sample_ref), (4.0, 8.0, "kick.wav"))
        self.assertEqual((pad.position_beats, pad.length_beats), (0.0, 16.0))
        riff = lead.clips[0]
        self.assertEqual((riff.position_beats, riff.length_beats, riff.is_midi), (2.0, 4.0, True))

    def test_locations(self):
        project = ardour.extract_ardour(self.text.encode())
        self.assertEqual(project.locators, 2)  # IsMark and IsRangeMarker; session range and loop excluded
        self.assertTrue(project.loop_on)
        self.assertEqual(project.loop_range, (0.0, 8.0))

    def test_region_edit_and_id_change(self):
        before = parse_project(FIXTURES / "ardour/basic.ardour")
        moved = self.text.replace("a2032128000@a1016064000", "a2032128000@a2032128000")
        after = parse_project(self.write("m.ardour", moved))
        self.assertEqual(compute_diff(before, after).clips_modified, 1)
        renamed = self.text.replace('Region id="601"', 'Region id="601"').replace('id="601" name="kick"', 'id="601" name="kick2"')
        self.assertEqual(compute_diff(before, parse_project(self.write("n.ardour", renamed))).clips_modified, 1)

    def test_legacy_sample_positions_use_sample_rate(self):
        legacy = self.text.replace("a2032128000@a1016064000", "96000").replace('start="a0" first-edit="nothing" source-0="101"', 'start="0" position="96000" first-edit="nothing" source-0="101"')
        kick = ardour.extract_ardour(legacy.encode()).tracks[1].clips[0]
        self.assertEqual((kick.length_beats, kick.position_beats), (4.0, 4.0))

    def test_rejects_bad_input(self):
        import gzip

        for name, data in (
            ("gz", gzip.compress(self.text.encode())),
            ("root", b"<Other/>"),
            ("xml", b"<Session"),
            ("dtd", b'<!DOCTYPE Session [<!ENTITY a "b">]><Session version="1"/>'),
            ("empty", b""),
        ):
            with self.subTest(name), self.assertRaises(ValueError):
                ardour.extract_ardour(data)

    def test_superclock_position_without_rate_is_zero_not_a_guess(self):
        stripped = self.text.replace(' superclocks-per-second="508032000"', "")
        kick = ardour.extract_ardour(stripped.encode()).tracks[1].clips[0]
        self.assertEqual(kick.position_beats, 0.0)


class LmmsTests(TmpMixin):
    def setUp(self):
        super().setUp()
        self.raw = (FIXTURES / "lmms/basic.mmp").read_bytes()

    def test_plain_and_compressed_parse_identically(self):
        plain = lmms.extract_lmms(self.raw)
        packed = lmms.extract_lmms((FIXTURES / "lmms/basic.mmpz").read_bytes())
        self.assertEqual(plain, packed)

    def test_project_content(self):
        project = lmms.extract_lmms(self.raw)
        self.assertEqual((project.bpm, project.time_signature), (140.0, (3, 4)))
        lead, drums, pattern, automation, sampler = project.tracks
        self.assertEqual(sampler.extra_sample_refs, ("drums/ride01.ogg",))
        self.assertEqual(lead.devices, ("instrument: tripleoscillator", "effect: bassbooster"))
        self.assertEqual(lead.midi_notes, 2)
        self.assertEqual((lead.clips[0].position_beats, lead.clips[0].length_beats), (2.0, 4.0))  # 96 and 192 ticks at 48/beat
        self.assertEqual(drums.clips[0].sample_ref, "drums/kick.wav")
        self.assertEqual(automation.automation_points, 3)
        self.assertEqual((project.loop_on, project.loop_range), (True, (0.0, 8.0)))

    def test_automated_tempo_and_legacy_names(self):
        text = self.raw.decode().replace('<head bpm="140"', '<head><bpm value="150" id="1"/><x').replace(
            'mastervol="100" masterpitch="0"/>', "/></head><!--"
        ).replace("<song>", "--><song>", 1)
        legacy = self.raw.decode().replace("midiclip", "pattern").replace("sampleclip", "sampletco").replace(
            "automationclip", "automationpattern"
        )
        project = lmms.extract_lmms(legacy.encode())
        self.assertEqual([len(t.clips) for t in project.tracks], [1, 1, 1, 1, 0])
        self.assertEqual(project.tracks[0].midi_notes, 2)
        self.assertEqual(lmms.extract_lmms(text.encode()).bpm, 150.0)

    def test_nested_pattern_track_containers_are_walked(self):
        nested = self.raw.decode().replace(
            "<patterntrack/>",
            '<patterntrack/><trackcontainer><track type="0" name="Inner"><instrumenttrack/></track></trackcontainer>',
        )
        names = [t.name for t in lmms.extract_lmms(nested.encode()).tracks]
        self.assertIn("Inner", names)

    def test_notes_cdata_containing_a_doctype_is_accepted(self):
        text = self.raw.decode().replace(
            "<timeline", '<projectnotes><![CDATA[<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.0//EN">]]></projectnotes><timeline'
        )
        self.assertEqual(len(lmms.extract_lmms(text.encode()).tracks), 5)

    def test_edit_seen_by_differ(self):
        before = parse_project(FIXTURES / "lmms/basic.mmp")
        edited = self.raw.decode().replace('<note key="60"', '<note key="61"')
        diff = compute_diff(before, parse_project(self.write("e.mmp", edited)))
        self.assertEqual(diff.clips_modified, 1)

    def test_rejects_bad_input(self):
        good_z = (FIXTURES / "lmms/basic.mmpz").read_bytes()
        cases = {
            "wrong_root": b'<?xml version="1.0"?><other/>',
            "other_type": self.raw.replace(b'type="song"', b'type="instrumenttracksettings"'),
            "custom_dtd": self.raw.replace(b"<!DOCTYPE lmms-project>", b'<!DOCTYPE lmms-project [<!ENTITY a "b">]>'),
            "other_doctype": self.raw.replace(b"<!DOCTYPE lmms-project>", b"<!DOCTYPE other>"),
            "truncated_zlib": good_z[: len(good_z) // 2],
            "short": b"\x00\x00",
            "garbage": b"\x00\x00\x00\x10" + b"not zlib at all",
            "length_mismatch": struct.pack(">I", 5) + good_z[4:],
            "malformed": b"<lmms-project><head></lmms-project>",
        }
        for name, data in cases.items():
            with self.subTest(name), self.assertRaises(ValueError):
                lmms.extract_lmms(data)

    def test_declared_length_and_bomb_bounds(self):
        payload = b"<lmms-project type='song'><song>" + b" " * 2_000_000 + b"</song></lmms-project>"
        packed = struct.pack(">I", len(payload)) + zlib.compress(payload, 9)
        self.assertLess(len(packed), 10_000)
        with mock.patch.object(_safe, "MAX_XML_BYTES", 1_000_000):
            with self.assertRaises(ValueError):
                lmms.extract_lmms(packed)
        liar = struct.pack(">I", 100) + zlib.compress(payload, 9)  # declares little, inflates a lot
        with self.assertRaises(ValueError):
            lmms.extract_lmms(liar)
        exact = len(self.raw)
        packed = struct.pack(">I", exact) + zlib.compress(self.raw, 9)
        for limit, ok in ((exact - 1, False), (exact, True), (exact + 1, True)):
            with self.subTest(limit), mock.patch.object(_safe, "MAX_XML_BYTES", limit):
                if ok:
                    lmms.extract_lmms(packed)
                else:
                    with self.assertRaises(ValueError):
                        lmms.extract_lmms(packed)


class PureDataTests(TmpMixin):
    def test_real_example(self):
        project = puredata.extract_pd((FIXTURES / "puredata/A01.sinewave.pd").read_text())
        self.assertEqual(len(project.tracks), 1)
        self.assertEqual(project.tracks[0].devices[:3], ("osc~ 440", "dac~", "*~ 0.05"))

    def test_tokenizer_escapes(self):
        statements = puredata.tokenize("#X msg 1 2 a \\; b \\, c \\$1 x, f 4;\n#X text 3 4 hi;")
        self.assertEqual(statements[0], ["#X", "msg", "1", "2", "a", ";", "b", ",", "c", "$1", "x", ",", "f", "4"])
        self.assertEqual(statements[1][-1], "hi")

    PATCH = (
        "#N canvas 0 0 450 300 12;\n"
        "#X obj 10 10 osc~ 220;\n"
        "#N canvas 0 0 300 200 sub 0;\n"
        "#X obj 5 5 readsf~ 2 loops/a.wav;\n"
        "#N canvas 0 0 200 100 inner 0;\n"
        "#X obj 1 1 *~ 2;\n"
        "#X restore 5 30 pd inner;\n"
        "#X restore 10 40 pd sub;\n"
        "#X obj 10 70 dac~;\n"
        "#X connect 0 0 2 0;\n"
    )

    def test_subpatches_are_tracks_with_parent_group(self):
        project = puredata.extract_pd(self.PATCH)
        self.assertEqual([(t.track_id, t.name, t.group_id) for t in project.tracks], [
            ("canvas-0", "main", ""), ("canvas-1", "sub", "canvas-0"), ("canvas-2", "inner", "canvas-1"),
        ])
        self.assertEqual(project.tracks[0].devices, ("osc~ 220", "dac~"))
        self.assertEqual(project.tracks[1].extra_sample_refs, ("loops/a.wav",))

    def test_moving_a_box_is_not_an_edit_but_changing_one_is(self):
        before = parse_project(self.write("a.pd", self.PATCH))
        moved = parse_project(self.write("b.pd", self.PATCH.replace("#X obj 10 10 osc~ 220", "#X obj 99 99 osc~ 220")))
        changed = parse_project(self.write("c.pd", self.PATCH.replace("osc~ 220", "osc~ 330")))
        self.assertEqual(compute_diff(before, moved).devices_changed, [])
        self.assertEqual(compute_diff(before, changed).devices_changed, ["main"])

    def test_rejects_bad_input(self):
        for text in ("", "hello", "#X obj 1 2 x;", "#N canvas 0 0 1 1 12;\n#X restore 1 1 pd x;", "#N canvas 0 0 1 1 12;\n#N canvas 0 0 1 1 12;\n#X restore 1 1 pd x;\n#X restore 1 1 pd y;\n#N canvas 0 0 1 1 12;"):
            with self.subTest(text), self.assertRaises(ValueError):
                puredata.extract_pd(text)

    def test_canvas_depth_boundary(self):
        def nest(levels):  # root plus `levels` subpatches
            return "#N canvas 0 0 1 1 12;\n" + "#N canvas 0 0 1 1 s 0;\n" * levels + "#X restore 0 0 pd s;\n" * levels

        limit = puredata.MAX_CANVAS_DEPTH
        puredata.extract_pd(nest(limit - 1))
        with self.assertRaises(ValueError):
            puredata.extract_pd(nest(limit))

    def test_statement_and_canvas_count_boundaries(self):
        base = "#N canvas 0 0 1 1 12;\n" + "#X obj 0 0 a;\n" * 4  # 5 statements
        with mock.patch.object(puredata, "MAX_STATEMENTS", 5):
            puredata.extract_pd(base)
            with self.assertRaises(ValueError):
                puredata.extract_pd(base + "#X obj 0 0 b;\n")
        canvases = "#N canvas 0 0 1 1 12;\n" + "#N canvas 0 0 1 1 s 0;\n#X restore 0 0 pd s;\n" * 3
        with mock.patch.object(puredata, "MAX_CANVASES", 4):
            puredata.extract_pd(canvases)
            with self.assertRaises(ValueError):
                puredata.extract_pd(canvases + "#N canvas 0 0 1 1 s 0;\n#X restore 0 0 pd s;\n")


class MaxpatTests(TmpMixin):
    def test_patchers_devices_and_samples(self):
        project = maxpat.extract_maxpat((FIXTURES / "maxpat/basic.maxpat").read_bytes())
        root, sub = project.tracks
        self.assertEqual(root.devices, ("cycle~ 440", "sfplay~ drums/kick.wav", "ezdac~", "p voice"))  # comment excluded
        self.assertEqual(root.extra_sample_refs, ("drums/kick.wav",))
        self.assertEqual((sub.name, sub.group_id, sub.devices), ("p voice", "patcher-0", ("*~ 0.5",)))

    def test_layout_changes_are_not_edits(self):
        text = (FIXTURES / "maxpat/basic.maxpat").read_text()
        before = parse_project(FIXTURES / "maxpat/basic.maxpat")
        moved = parse_project(self.write("m.maxpat", text.replace("[30.0, 40.0, 70.0, 22.0]", "[90.0, 90.0, 70.0, 22.0]")))
        edited = parse_project(self.write("e.maxpat", text.replace("cycle~ 440", "cycle~ 880")))
        self.assertEqual(compute_diff(before, moved).devices_changed, [])
        self.assertEqual(compute_diff(before, edited).devices_changed, ["main"])

    def test_rejects_bad_input(self):
        for data in (b"", b"{", b"[]", b'{"patcher": 1}', b'{"other": {}}', b"\xff\xfe"):
            with self.subTest(data), self.assertRaises(ValueError):
                maxpat.extract_maxpat(data)

    def test_tolerates_missing_and_odd_members(self):
        project = maxpat.extract_maxpat(b'{"patcher": {"boxes": [1, {"box": 2}, {"box": {"maxclass": "toggle"}}], "lines": 3}}')
        self.assertEqual(project.tracks[0].devices, ("toggle",))

    def test_patcher_nesting_boundary(self):
        def nested(levels):
            doc = '{"boxes": []}'
            for _ in range(levels):
                doc = '{"boxes": [{"box": {"maxclass": "newobj", "text": "p s", "patcher": %s}}]}' % doc
            return ('{"patcher": %s}' % doc).encode()

        # each nesting level adds four JSON containers (patcher, boxes list, entry, box)
        with mock.patch.object(_safe, "MAX_JSON_DEPTH", 4 * 3 + 3):
            self.assertEqual(len(maxpat.extract_maxpat(nested(3)).tracks), 4)
            with self.assertRaises(ValueError):
                maxpat.extract_maxpat(nested(4))

    def test_patcher_count_boundary(self):
        def many(n):
            boxes = ",".join('{"box": {"maxclass": "newobj", "text": "p s", "patcher": {"boxes": []}}}' for _ in range(n))
            return ('{"patcher": {"boxes": [%s]}}' % boxes).encode()

        with mock.patch.object(maxpat, "MAX_PATCHERS", 4):
            maxpat.extract_maxpat(many(3))
            with self.assertRaises(ValueError):
                maxpat.extract_maxpat(many(4))


if __name__ == "__main__":
    unittest.main()
