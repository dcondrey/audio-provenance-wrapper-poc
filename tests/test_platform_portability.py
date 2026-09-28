import json
import struct
import tempfile
import unittest
import wave
from pathlib import Path
from unittest import mock

from daemon.hardware_attestation import provider as hw
from daemon.input_capture import observer as input_observer
from daemon.sample_watcher import watcher

SYSTEMS = ("Darwin", "Linux", "Windows", "FreeBSD", "")


def _ext80(rate: int) -> bytes:
    exponent = 16383 + rate.bit_length() - 1
    mantissa = rate << (64 - rate.bit_length())
    return struct.pack(">HQ", exponent, mantissa)


def write_aiff(path: Path, rate: int, channels: int, frames: int) -> None:
    comm = struct.pack(">hIh", channels, frames, 16) + _ext80(rate)
    ssnd_data = b"\x00" * (frames * channels * 2)
    ssnd = struct.pack(">II", 0, 0) + ssnd_data
    body = b"AIFF" + b"COMM" + struct.pack(">I", len(comm)) + comm + b"SSND" + struct.pack(">I", len(ssnd)) + ssnd
    path.write_bytes(b"FORM" + struct.pack(">I", len(body)) + body)


def write_float_wav(path: Path, rate: int, channels: int, frames: int) -> None:
    fmt = struct.pack("<HHIIHH", 3, channels, rate, rate * channels * 4, channels * 4, 32)
    data = b"\x00" * (frames * channels * 4)
    body = b"WAVE" + b"fmt " + struct.pack("<I", 16) + fmt + b"data" + struct.pack("<I", len(data)) + data
    path.write_bytes(b"RIFF" + struct.pack("<I", len(body)) + body)


class SampleMetadataPortabilityTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        patcher = mock.patch.object(watcher.subprocess, "run", side_effect=AssertionError("afinfo must not run"))
        self.run = patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(self._tmp.cleanup)

    def test_aiff_and_float_wav_without_platform_tools(self):
        for system in SYSTEMS:
            with mock.patch.object(watcher.platform, "system", return_value=system):
                aiff = self.root / "a.aiff"
                write_aiff(aiff, 44100, 2, 4410)
                meta = watcher.extract_audio_metadata(aiff)
                self.assertEqual((meta["sample_rate"], meta["channels"]), (44100, 2))
                self.assertAlmostEqual(meta["duration_seconds"], 0.1)
                flt = self.root / "f.wav"
                write_float_wav(flt, 96000, 1, 9600)
                meta = watcher.extract_audio_metadata(flt)
                self.assertEqual((meta["sample_rate"], meta["channels"]), (96000, 1))
                self.assertAlmostEqual(meta["duration_seconds"], 0.1)

    def test_unreadable_format_is_null_not_afinfo_off_darwin(self):
        mp3 = self.root / "x.mp3"
        mp3.write_bytes(b"ID3" + b"\x00" * 64)
        for system in ("Linux", "Windows"):
            with mock.patch.object(watcher.platform, "system", return_value=system):
                self.assertEqual(watcher.extract_audio_metadata(mp3), watcher.empty_audio_metadata())
                event = watcher.build_sample_file_event(mp3)
            self.assertIn("Audio metadata unavailable", " ".join(event["notes"]))
            self.assertIsNone(event["audio_metadata"]["sample_rate"])

    def test_truncated_headers_do_not_raise(self):
        bad = self.root / "bad.aiff"
        bad.write_bytes(b"FORM\x00\x00\x00\x04AIFF")
        with mock.patch.object(watcher.platform, "system", return_value="Linux"):
            self.assertEqual(watcher.extract_audio_metadata(bad), watcher.empty_audio_metadata())
            wav = self.root / "bad.wav"
            wav.write_bytes(b"RIFF\x00\x00\x00\x04WAVE")
            self.assertEqual(watcher.extract_audio_metadata(wav), watcher.empty_audio_metadata())

    def test_stdlib_wav_unchanged(self):
        path = self.root / "s.wav"
        with wave.open(str(path), "wb") as handle:
            handle.setnchannels(1)
            handle.setsampwidth(2)
            handle.setframerate(8000)
            handle.writeframes(b"\x00\x00" * 2000)
        with mock.patch.object(watcher.platform, "system", return_value="Windows"):
            self.assertEqual(watcher.extract_audio_metadata(path)["sample_rate"], 8000)


class AttestationFallbackTests(unittest.TestCase):
    def test_every_platform_is_software_and_unattested(self):
        for system in SYSTEMS:
            with mock.patch("platform.system", return_value=system), tempfile.TemporaryDirectory() as tmp:
                status = hw.attestation_status()
                self.assertFalse(status["hardware_attested"])
                self.assertEqual(status["proof_level"], "unknown_unobserved")
                self.assertEqual(status["provider"], "SoftwareProvider")
                provider = hw.detect_provider(Path(tmp) / "k.bin")
                self.assertIs(type(provider), hw.SoftwareProvider, system)

    def test_linux_tpm_presence_is_not_claimed_as_integration(self):
        with mock.patch.object(hw.Path, "exists", return_value=True):
            status = hw.attestation_status("Linux")
        self.assertIn("not integrated", status["hardware_candidate"])
        self.assertFalse(status["hardware_attested"])

    def test_windows_key_file_does_not_chmod(self):
        with tempfile.TemporaryDirectory() as tmp:
            key = Path(tmp) / "k.bin"
            key.write_bytes(b"\x01" * 32)
            key.chmod(0o666)
            with mock.patch.object(hw.os, "name", "nt"), mock.patch.object(hw.os, "chmod") as chmod:
                hw.SoftwareProvider._restrict(key)
            chmod.assert_not_called()


class UnsupportedObserverTests(unittest.TestCase):
    def test_input_and_screen_report_unsupported_on_every_os(self):
        from daemon.screen_observer import observer as screen_observer

        for module in (input_observer, screen_observer):
            for system in SYSTEMS:
                status = module.platform_support(system)
                self.assertFalse(status["supported"])
                self.assertEqual(status["status"], "unsupported_platform")
                self.assertEqual(status["proof_level"], "unknown_unobserved")

    def test_main_exits_nonzero_with_json_status(self):
        from daemon.screen_observer import observer as screen_observer

        for module in (input_observer, screen_observer):
            with mock.patch("builtins.print") as printed:
                self.assertEqual(module.main([]), 2)
            self.assertFalse(json.loads(printed.call_args[0][0])["supported"])


if __name__ == "__main__":
    unittest.main()
