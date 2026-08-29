import tempfile
import unittest
from pathlib import Path

from daemon.hardware_attestation.provider import SoftwareProvider


class SoftwareProviderSealTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.provider = SoftwareProvider(key_path=Path(self._tmp.name) / "key.bin")

    def tearDown(self):
        self._tmp.cleanup()

    def test_round_trip(self):
        plaintext = b"provenance evidence payload" * 5
        sealed = self.provider.seal(plaintext)
        self.assertEqual(self.provider.unseal(sealed), plaintext)

    def test_round_trip_empty(self):
        sealed = self.provider.seal(b"")
        self.assertEqual(self.provider.unseal(sealed), b"")

    def test_repeated_32_byte_blocks_produce_distinct_ciphertext(self):
        """Regression for M-006: the old keystream repeated a single 32-byte
        HMAC block, so identical aligned plaintext blocks encrypted to
        identical ciphertext blocks (ECB-style leakage)."""
        plaintext = b"A" * 32 + b"A" * 32 + b"A" * 32
        sealed = self.provider.seal(plaintext)
        ciphertext = sealed[32:]
        block0, block1, block2 = ciphertext[0:32], ciphertext[32:64], ciphertext[64:96]
        self.assertNotEqual(block0, block1)
        self.assertNotEqual(block1, block2)
        self.assertNotEqual(block0, block2)

    def test_tamper_detected(self):
        sealed = bytearray(self.provider.seal(b"integrity matters"))
        sealed[-1] ^= 0xFF
        with self.assertRaises(ValueError):
            self.provider.unseal(bytes(sealed))

    def test_unseal_rejects_short_input(self):
        with self.assertRaises(ValueError):
            self.provider.unseal(b"too short")


if __name__ == "__main__":
    unittest.main()
