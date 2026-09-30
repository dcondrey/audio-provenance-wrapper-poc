"""MilkyTracker modules (.xm, .mod) and VCV Rack patches (.vcv).

The shared corpus (tests/fixtures/parity/project_modules_corpus.json) holds the
Python oracle's verdict for thousands of inputs; the Rust port is tested against
the same file (rust/apw-daemon/tests/project_modules_parity.rs). Fixture
provenance is in docs/PROJECT_FORMATS.md: test.xm/test.mod are OpenMPT's real
test modules, vcv/basic.vcv is constructed. Inputs below are constructed.
"""

import json
import sys
import unittest
from pathlib import Path
from unittest import mock

import zstandard

FIXTURES = Path(__file__).parent / "fixtures" / "projects"
sys.path.insert(0, str(FIXTURES))

import module_builders as mb  # noqa: E402
import module_corpus  # noqa: E402

from daemon.project_formats import _safe, _tarzst, parse_project, tracker, vcv  # noqa: E402
from tests.support import TmpMixin  # noqa: E402


class ParseMixin(TmpMixin):
    def parse(self, name: str, data: bytes):
        return parse_project(self.write(name, data))


class CorpusTests(unittest.TestCase):
    def test_the_python_parsers_reproduce_every_committed_verdict(self):
        document = json.loads((Path(__file__).parent / "fixtures/parity/project_modules_corpus.json").read_text())
        cases = document["cases"]
        self.assertGreater(len(cases), 6000)
        stale = [c["name"] for c in cases if module_corpus.evaluate(c, document["bases"]) != c["expect"]]
        self.assertEqual(stale, [], "corpus is stale: rerun tests/fixtures/parity/generate_project_fixtures.py")
        accepted = sum(1 for c in cases if "ok" in c["expect"])
        self.assertGreater(accepted, 1000)
        self.assertGreater(len(cases) - accepted, 1000)


class TrackerTests(ParseMixin):
    def test_real_xm_reports_header_instruments_and_samples(self):
        snapshot = parse_project(FIXTURES / "milkytracker/test.xm")
        module = snapshot.tracks[0]
        self.assertEqual(module.name, "Test Module")
        self.assertIn("tracker: OpenMPT 1.32.00.32", module.devices)
        self.assertIn("channels: 2", module.devices)
        self.assertIn("bpm: 139", module.devices)
        self.assertEqual(snapshot.transport_bpm, 139.0)
        self.assertEqual(snapshot.project_format, "milkytracker")
        self.assertEqual(snapshot.tracks[2].devices[0], "Pulse Sample (16 bytes)")

    def test_real_mod_reports_tag_channels_and_sample(self):
        snapshot = parse_project(FIXTURES / "milkytracker/test.mod")
        module = snapshot.tracks[0]
        self.assertEqual(module.name, "MOD_Test___________X")
        self.assertIn("format: MOD M.K.", module.devices)
        self.assertIn("channels: 4", module.devices)
        self.assertEqual(snapshot.transport_bpm, 0.0)
        self.assertEqual(snapshot.tracks[2].devices, ("Sample_1_____________X (1244 bytes)",))

    def test_sample_or_pattern_edits_change_the_hashes_and_renames_do_not_touch_them(self):
        base = mb.xm_bytes(instruments=[{"name": "a", "samples": [("s", 8, 8)]}], patterns=[(2, b"\x80\x80")])
        edited_sample = base[:-1] + bytes([base[-1] ^ 1])
        edited_pattern = mb.xm_bytes(instruments=[{"name": "a", "samples": [("s", 8, 8)]}], patterns=[(2, b"\x81\x80")])
        one, two, three = (self.parse(f"{i}.xm", d) for i, d in enumerate((base, edited_sample, edited_pattern)))
        self.assertEqual(one.tracks[1].device_chain_hashes, two.tracks[1].device_chain_hashes)
        self.assertNotEqual(one.tracks[2].device_chain_hashes, two.tracks[2].device_chain_hashes)
        self.assertNotEqual(one.tracks[1].device_chain_hashes, three.tracks[1].device_chain_hashes)

    def test_format_maxima_have_exact_boundaries(self):
        def xm(**kw):
            return self.parse("b.xm", mb.xm_bytes(**kw))

        for limit, build in (
            (tracker.MAX_XM_CHANNELS, lambda n: {"channels": n}),
            (tracker.MAX_XM_PATTERNS, lambda n: {"patterns": [(1, b"")] * n}),
            (tracker.MAX_XM_INSTRUMENTS, lambda n: {"instruments": [{"name": "i"}] * n}),
            (tracker.MAX_XM_SAMPLES_PER_INSTRUMENT, lambda n: {"instruments": [{"name": "i", "samples": [("s", 1, 1)] * n}]}),
            (tracker.MAX_XM_ROWS, lambda n: {"patterns": [(n, b"")]}),
        ):
            with self.subTest(limit=limit):
                xm(**build(limit - 1))
                xm(**build(limit))
                with self.assertRaises(ValueError):
                    xm(**build(limit + 1))
        # The order table is clamped, not refused, past its 256 entries.
        self.assertIn("song length: 256", xm(order_count=257).tracks[0].devices)
        with self.assertRaises(ValueError):
            xm(channels=0)

    def test_truncated_sample_data_and_missing_instruments_are_tolerated_and_flagged(self):
        short = self.parse("s.xm", mb.xm_bytes(instruments=[{"name": "x", "samples": [("s", 100, 10)]}]))
        self.assertIn("truncated", short.tracks[0].devices)
        self.assertEqual(short.tracks[2].devices, ("s (100 bytes)",))
        fewer = self.parse("f.xm", mb.xm_bytes(instruments=[{"name": "x"}], instrument_count=4))
        self.assertIn("truncated", fewer.tracks[0].devices)
        self.assertEqual(len(fewer.tracks), 3)
        with self.assertRaises(ValueError):
            self.parse("p.xm", mb.xm_bytes(patterns=[(1, b"\x80\x80\x80")])[:-2])

    def test_unsupported_versions_and_untagged_modules_are_refused(self):
        for version in (0x0102, 0x0103, 0x0105):
            with self.assertRaises(ValueError):
                self.parse("v.xm", mb.xm_bytes(version=version))
        with self.assertRaises(ValueError):
            self.parse("t.mod", mb.mod_bytes(tag=b"\x00\x00\x00\x00"))
        for tag, channels in ((b"M.K.", 4), (b"FLT8", 8), (b"6CHN", 6), (b"32CH", 32), (b"9CHN", 9)):
            snapshot = self.parse("t.mod", mb.mod_bytes(tag=tag, channels=channels))
            self.assertIn(f"channels: {channels}", snapshot.tracks[0].devices)

    def test_text_fields_are_latin1_cut_at_nul_and_right_trimmed(self):
        self.assertEqual(tracker._text(b"ab  \x00zz"), "ab")
        self.assertEqual(tracker._text(b"\xe9 "), "é")


class TarZstdTests(ParseMixin):
    def test_bomb_boundary_at_the_default_cap(self):
        """RLE blocks expand 1 byte to 128 KiB; 2048 of them are exactly the 256 MiB cap."""

        def frame(blocks: int) -> bytes:
            body = b""
            for index in range(blocks):
                last = 1 if index == blocks - 1 else 0
                body += (((1 << 17) << 3) | (1 << 1) | last).to_bytes(3, "little") + b"\x00"
            return zstandard.FRAME_HEADER + bytes([0x00, 17 << 3]) + body

        cap = _safe.MAX_TAR_BYTES
        self.assertEqual(cap, 2048 * (1 << 17))
        self.assertEqual(len(_tarzst.inflate_zstd(frame(2048))), cap)
        with self.assertRaises(ValueError) as caught:
            _tarzst.inflate_zstd(frame(2049))
        self.assertIn("decompresses past", str(caught.exception))

    def test_member_count_and_size_caps_have_exact_boundaries(self):
        fixture = (FIXTURES / "vcv/basic.vcv").read_bytes()
        tar = _tarzst.inflate_zstd(fixture)
        members = len(_tarzst.read_tar(tar))
        self.assertEqual(members, 2)  # patch.json and the asset; directories are not files but do count below
        entries = 5
        for limit, ok in ((entries - 1, False), (entries, True), (entries + 1, True)):
            with mock.patch.object(_safe, "MAX_TAR_MEMBERS", limit):
                if ok:
                    _tarzst.read_tar(tar)
                else:
                    with self.assertRaises(ValueError):
                        _tarzst.read_tar(tar)
        for limit, ok in ((len(tar) - 1, False), (len(tar), True), (len(tar) + 1, True)):
            with mock.patch.object(_safe, "MAX_TAR_BYTES", limit):
                if ok:
                    self.assertEqual(_tarzst.inflate_zstd(fixture), tar)
                else:
                    with self.assertRaises(ValueError):
                        _tarzst.inflate_zstd(fixture)

    def test_unsafe_archives_are_refused_and_nothing_is_written(self):
        patch = mb.patch_json_bytes(mb.constructed_patch())
        for label, entry in (
            ("traversal", mb.tar_entry("../evil", b"x")),
            ("absolute", mb.tar_entry("/evil", b"x")),
            ("symlink", mb.tar_entry("./l", kind=b"2")),
            ("hardlink", mb.tar_entry("./l", kind=b"1")),
            ("device", mb.tar_entry("./d", kind=b"3")),
        ):
            with self.subTest(label):
                with self.assertRaises(ValueError):
                    vcv.extract_vcv(mb.zstd_stream(mb.tar_bytes([entry, mb.tar_entry("./patch.json", patch)])))
        self.assertEqual(list(self.tmp.iterdir()), [])

    def test_frames_with_dictionaries_trailing_bytes_or_bad_checksums_are_refused(self):
        tar = mb.rack_tar(mb.patch_json_bytes(mb.constructed_patch()))
        for label, data in (
            ("dictionary", mb.zstd_raw_frame(tar, descriptor_or=1)),
            ("trailing", mb.zstd_stream(tar) + b"\x00"),
            ("two frames", mb.zstd_stream(tar) * 2),
            ("window", mb.zstd_raw_frame(tar, single_segment=False, window_descriptor=18 << 3)),
        ):
            with self.subTest(label), self.assertRaises(ValueError):
                _tarzst.inflate_zstd(data)
        flipped = bytearray(mb.zstd_stream(tar, checksum=True))
        flipped[-1] ^= 1
        with self.assertRaises(ValueError):
            _tarzst.inflate_zstd(bytes(flipped))


class VcvTests(ParseMixin):
    def test_fixture_lists_modules_cables_params_data_and_bypass(self):
        snapshot = parse_project(FIXTURES / "vcv/basic.vcv")
        self.assertEqual(snapshot.project_format, "vcv_rack")
        self.assertEqual(snapshot.transport_bpm, 0.0)
        self.assertEqual(snapshot.tracks[0].devices[:2], ("format: VCV Rack patch (tar+zstd)", "rack version: 2.6.6"))
        self.assertIn("modules: 5", snapshot.tracks[0].devices)
        self.assertIn("cables: 3", snapshot.tracks[0].devices)
        by_id = {t.track_id: t for t in snapshot.tracks}
        self.assertEqual(by_id["module-103"].devices, ("Core/AudioInterface2", "version: 2.6.6", "params: 1", "data: present"))
        self.assertIn("bypassed", by_id["module-104"].devices)
        self.assertEqual(by_id["cables"].devices[0], "101:0 -> 102:3")

    def test_moving_a_module_or_recoloring_a_cable_is_not_an_edit_but_a_param_change_is(self):
        def build(mutate):
            patch = mb.constructed_patch()
            mutate(patch)
            return self.parse("p.vcv", mb.zstd_stream(mb.rack_tar(mb.patch_json_bytes(patch))))

        base = build(lambda p: None)
        moved = build(lambda p: (p["modules"][0].update(pos=[3, 4]), p["cables"][0].update(color="#000000", id=99)))
        turned = build(lambda p: p["modules"][0]["params"][0].update(value=0.75))
        self.assertEqual(base.device_chain_hashes, moved.device_chain_hashes)
        self.assertNotEqual(base.device_chain_hashes, turned.device_chain_hashes)

    def test_legacy_json_patch_is_read_and_module_data_is_never_interpreted(self):
        text = json.dumps({"version": "1.1.6", "modules": [{"plugin": "p", "model": "m", "data": {"path": "/etc/passwd", "__import__": "os"}}], "wires": []})
        snapshot = self.parse("l.vcv", text.encode())
        self.assertIn("format: VCV Rack patch (json (legacy))", snapshot.tracks[0].devices)
        self.assertIn("data: present", snapshot.tracks[1].devices)
        self.assertEqual(snapshot.sample_refs, frozenset())

    def test_non_patches_are_refused(self):
        for label, data in (("list", b"[1]"), ("no version", b"{}"), ("nan", b'{"version": "1", "x": NaN}'), ("empty", b"")):
            with self.subTest(label), self.assertRaises(ValueError):
                self.parse("x.vcv", data)
        with self.assertRaises(ValueError):
            self.parse("x.vcv", mb.zstd_stream(mb.tar_bytes([mb.tar_entry("./other.json", b"{}")])))

    def test_json_depth_boundary(self):
        depth_ok, depth_bad = module_corpus._nested(125), module_corpus._nested(126)
        self.assertIsNotNone(self.parse("ok.vcv", depth_ok))
        with self.assertRaises(ValueError):
            self.parse("bad.vcv", depth_bad)


if __name__ == "__main__":
    unittest.main()
