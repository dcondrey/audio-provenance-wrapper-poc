import unittest

from daemon.hardware_attestation.provider import SoftwareProvider
from tests.support import TmpMixin


class SoftwareProviderTests(TmpMixin):
    def setUp(self):
        super().setUp()
        self.key_path = self.tmp / "key.bin"
        self.provider = SoftwareProvider(key_path=self.key_path)

    def test_seal_round_trips(self):
        for plaintext in (b"provenance evidence payload" * 5, b""):
            with self.subTest(len(plaintext)):
                self.assertEqual(self.provider.unseal(self.provider.seal(plaintext)), plaintext)

    def test_repeated_32_byte_blocks_produce_distinct_ciphertext(self):
        # M-006: the old keystream repeated one 32-byte HMAC block, leaking equal aligned blocks
        sealed = self.provider.seal(b"A" * 96)
        blocks = {sealed[32 + i:64 + i] for i in (0, 32, 64)}
        self.assertEqual(len(blocks), 3)

    def test_unseal_rejects_tampered_and_truncated_input(self):
        sealed = bytearray(self.provider.seal(b"integrity matters"))
        sealed[-1] ^= 0xFF
        for bad in (bytes(sealed), b"too short"):
            with self.subTest(len(bad)), self.assertRaises(ValueError):
                self.provider.unseal(bad)

    def test_signatures_verify_only_their_message_and_across_instances(self):
        signature = self.provider.sign(b"data")
        reopened = SoftwareProvider(key_path=self.key_path)
        self.assertTrue(reopened.verify(b"data", signature))
        self.assertFalse(reopened.verify(b"tampered", signature))
        self.assertEqual(reopened.device_identity().device_id, self.provider.device_identity().device_id)

    def test_monotonic_counter_increments(self):
        self.assertEqual([self.provider.monotonic_counter() for _ in range(3)], [1, 2, 3])


if __name__ == "__main__":
    unittest.main()
