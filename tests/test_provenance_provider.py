import datetime as dt
import json
import math
import struct
import tempfile
import unittest
import wave
from pathlib import Path

import pytest
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID

from daemon.common import canonical_json_bytes
from daemon.provenance import VerificationState, detect_provider
from daemon.provenance.local_reference import LocalReferenceProvider, validate_certificate_chain
from daemon.provenance.provider import NOTHING_FOUND_NORMATIVE_NOTE, RevokedKeyError


def _write_wav(path: Path, seed: float = 220.0, seconds: float = 3.0) -> Path:
    rate = 44100
    frames = bytearray()
    for index in range(int(rate * seconds)):
        value = 0.4 * math.sin(2 * math.pi * seed * index / rate)
        value += 0.2 * math.sin(2 * math.pi * (seed * 2.7) * index / rate)
        frames += struct.pack("<h", int(value * 32767))
    with wave.open(str(path), "wb") as handle:
        handle.setnchannels(1)
        handle.setsampwidth(2)
        handle.setframerate(rate)
        handle.writeframes(bytes(frames))
    return path


def _self_signed_leaf() -> bytes:
    key = ec.generate_private_key(ec.SECP256R1())
    subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Self Signed Creator")])
    now = dt.datetime.now(dt.timezone.utc)
    cert = (
        x509.CertificateBuilder()
        .subject_name(subject)
        .issuer_name(subject)
        .public_key(key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - dt.timedelta(minutes=5))
        .not_valid_after(now + dt.timedelta(days=30))
        .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
        .add_extension(
            x509.KeyUsage(
                digital_signature=True, content_commitment=False, key_encipherment=False,
                data_encipherment=False, key_agreement=False, key_cert_sign=False,
                crl_sign=False, encipher_only=False, decipher_only=False,
            ),
            critical=True,
        )
        .add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.EMAIL_PROTECTION]), critical=True)
        .sign(key, hashes.SHA256())
    )
    return cert.public_bytes(serialization.Encoding.PEM)


class ProvenanceProviderTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.provider = LocalReferenceProvider(store_dir=self.root / "store")
        self.asset = _write_wav(self.root / "take.wav")

    def tearDown(self):
        self._tmp.cleanup()

    def _register(self, asset: Path) -> tuple[dict, bytes]:
        mark = self.provider.embed_mark(asset, {"title": "take one"})
        from daemon.common import sha256_file

        manifest = {"content_sha256": sha256_file(asset), "mark_id": mark["mark_id"]}
        self.provider.register(manifest)
        return manifest, canonical_json_bytes(manifest)

    def test_issued_chain_validates_and_self_signed_leaf_does_not(self):
        ok, reason = validate_certificate_chain(
            self.provider.chain_pem(), self.provider.trust_anchor_pem()
        )
        self.assertTrue(ok, reason)

        rejected, why = validate_certificate_chain(
            _self_signed_leaf(), self.provider.trust_anchor_pem()
        )
        self.assertFalse(rejected)
        self.assertIn("self-signed", why)

        foreign = LocalReferenceProvider(store_dir=self.root / "other")
        untrusted, anchor_reason = validate_certificate_chain(
            foreign.chain_pem(), self.provider.trust_anchor_pem()
        )
        self.assertFalse(untrusted)
        self.assertIn("trust anchor", anchor_reason)

    def test_revocation_blocks_signing_but_retains_history(self):
        self.provider.sign_claim(b"first claim")
        self.provider.sign_claim(b"second claim")
        key_id = self.provider.key_id

        record = self.provider.revoke(key_id)
        self.assertTrue(self.provider.identity()["revoked"])
        self.assertEqual(record["retained_signing_records"], 2)
        self.assertEqual(self.provider.key_id, key_id)

        history = self.provider.signing_history(key_id)
        self.assertEqual(len(history), 2)
        self.assertTrue(all(entry["signature_hex"] for entry in history))

        with self.assertRaises(RevokedKeyError):
            self.provider.sign_claim(b"third claim")
        with self.assertRaises(RevokedKeyError):
            self.provider.issue_signing_material()

        self.assertEqual(len(self.provider.signing_history(key_id)), 2)

    def test_four_state_mapping(self):
        unknown = self.provider.verify(self.asset, None)
        self.assertEqual(unknown["state"], VerificationState.NOTHING_FOUND.value)

        _, manifest_bytes = self._register(self.asset)
        self.assertEqual(
            self.provider.verify(self.asset, manifest_bytes)["state"],
            VerificationState.VERIFIED.value,
        )

        unrelated = _write_wav(self.root / "unrelated.wav", seed=440.0)
        self.assertIsNone(self.provider.recover_mark(unrelated))
        self.assertEqual(
            self.provider.verify(unrelated, None)["state"],
            VerificationState.NOTHING_FOUND.value,
        )

        altered = self.root / "altered.wav"
        raw = bytearray(self.asset.read_bytes())
        raw[-1] ^= 0x01
        altered.write_bytes(bytes(raw))
        changed = self.provider.verify(altered, None)
        self.assertEqual(changed["state"], VerificationState.REGISTERED_BUT_CHANGED.value)
        self.assertEqual(changed["matched_by"], "mark_id")
        self.assertEqual(changed["apw:proof_level"], "inferred")

        self.provider.revoke(self.provider.key_id)
        untrusted = self.provider.verify(self.asset, manifest_bytes)
        self.assertEqual(
            untrusted["state"], VerificationState.MARK_FOUND_CLAIM_NOT_TRUSTED.value
        )
        self.assertIn("revoked", untrusted["reason"])

    def test_nothing_found_is_not_an_assertion_of_synthetic_origin(self):
        result = self.provider.verify(self.asset, None)
        self.assertEqual(result["state"], VerificationState.NOTHING_FOUND.value)
        self.assertEqual(result["normative_note"], NOTHING_FOUND_NORMATIVE_NOTE)
        self.assertIn("not proof of synthetic origin", result["normative_note"])
        self.assertEqual(result["apw:proof_level"], "unknown_unobserved")
        self.assertIsNone(result["registry_id"])
        self.assertNotIn("synthetic", result["reason"])

    def test_issued_material_is_accepted_by_the_real_c2pa_signer(self):
        """The AKI-less leaf that c2pa-rs rejects passes every local structural
        check, so this invariant can only be pinned by the real signer."""
        c2pa = pytest.importorskip("c2pa")
        material = self.provider.issue_signing_material()
        c2pa.load_settings(json.dumps({
            "trust": {"trust_anchors": material.trust_anchor_pem.decode()},
            "verify": {"verify_trust": True},
        }))
        signer = c2pa.Signer.from_info(c2pa.C2paSignerInfo(
            alg=b"es256",
            sign_cert=material.certificate_chain_pem,
            private_key=material.private_key_handle,
            ta_url=None,
        ))
        signed = self.root / "signed.wav"
        c2pa.Builder({
            "claim_generator_info": [{"name": "apw-test", "version": "0.1"}],
            "title": "provenance seam",
            "assertions": [{"label": "c2pa.actions.v2", "data": {"actions": [{
                "action": "c2pa.created",
                "digitalSourceType":
                    "http://cv.iptc.org/newscodes/digitalsourcetype/digitalCapture",
                "softwareAgent": {"name": "apw-test", "version": "0.1"},
            }]}}],
        }).sign_file(str(self.asset), str(signed), signer)
        with open(signed, "rb") as handle:
            reader = c2pa.Reader("audio/wav", handle)
            self.assertEqual(reader.get_validation_state(), "Trusted")

    def test_detect_provider_defaults_to_local_reference(self):
        self.assertIsInstance(
            detect_provider(store_dir=self.root / "detected"), LocalReferenceProvider
        )


if __name__ == "__main__":
    unittest.main()
