import json
import unittest
from pathlib import Path

from daemon.time_anchor.anchor import (
    RFC3161Provider,
    TimeAnchorService,
    TimeProof,
    encode_timestamp_request,
    parse_timestamp_response,
)

FIXTURE_DIR = Path(__file__).parent / "fixtures"


def _load_fixture() -> tuple[bytes, str, str]:
    response = bytes.fromhex(
        (FIXTURE_DIR / "rfc3161_digicert_response.hex").read_text().strip()
    )
    request = json.loads((FIXTURE_DIR / "rfc3161_digicert_request.json").read_text())
    return response, request["data_hash"], request["nonce_hex"]


class TimestampParseTests(unittest.TestCase):
    """Fixture provenance: a live DigiCert TSA response, not our own encoder."""

    def test_parses_real_tsa_response(self):
        response, data_hash, nonce_hex = _load_fixture()
        parsed = parse_timestamp_response(response)
        self.assertTrue(parsed.granted)
        self.assertEqual(parsed.imprint_hash_hex, data_hash)
        self.assertEqual(parsed.nonce, int.from_bytes(bytes.fromhex(nonce_hex), "big"))
        self.assertIsNotNone(parsed.gentime_ms)
        self.assertGreater(parsed.gentime_ms or 0, 1_700_000_000_000)

    def test_malformed_responses_raise_value_error(self):
        response, _hash, _nonce = _load_fixture()
        for malformed in (b"", b"\x30", b"\x00\x00\x00", response[:40], response[:-11]):
            with self.assertRaises(ValueError, msg=repr(malformed[:8])):
                parse_timestamp_response(malformed)

    def test_verify_binds_hash_and_nonce(self):
        response, data_hash, nonce_hex = _load_fixture()
        gentime_ms = parse_timestamp_response(response).gentime_ms
        proof = TimeProof(
            source="rfc3161:fixture",
            timestamp_ms=gentime_ms,
            nonce_hex=nonce_hex,
            response_hex=response.hex(),
            certificate_chain_hex=None,
        )
        provider = RFC3161Provider()
        self.assertTrue(provider.verify(proof, data_hash))
        self.assertFalse(provider.verify(proof, "0" * 64))
        replayed = TimeProof(
            source=proof.source,
            timestamp_ms=gentime_ms,
            nonce_hex="00" * 16,
            response_hex=response.hex(),
            certificate_chain_hex=None,
        )
        self.assertFalse(provider.verify(replayed, data_hash))
        # A timestamp edited apart from the retained token must not verify.
        edited = TimeProof(
            source=proof.source,
            timestamp_ms=(gentime_ms or 0) - 86_400_000,
            nonce_hex=nonce_hex,
            response_hex=response.hex(),
            certificate_chain_hex=None,
        )
        self.assertFalse(provider.verify(edited, data_hash))


class RequestEncodeTests(unittest.TestCase):
    def test_request_encoding_is_stable(self):
        # This exact structure was accepted by a live DigiCert TSA on 2026-08-28.
        der = encode_timestamp_request("ab" * 32, bytes.fromhex("11" * 16))
        self.assertEqual(
            der.hex(),
            "304b0201013031300d060960864801650304020105000420"
            + "ab" * 32
            + "0210" + "11" * 16
            + "0101ff",
        )

    def test_rejects_non_sha256_hash(self):
        with self.assertRaises(ValueError):
            encode_timestamp_request("abcd", b"\x01" * 16)


class AnchorRecordTests(unittest.TestCase):
    def test_unreachable_tsa_degrades_to_unavailable(self):
        service = TimeAnchorService(RFC3161Provider("http://127.0.0.1:1/tsa"))
        record = service.anchor_record("ab" * 32)
        self.assertEqual(record["status"], "unavailable")
        self.assertEqual(record["apw:proof_level"], "unknown_unobserved")
        self.assertIn("reason", record)

    def test_anchored_record_is_inferred_not_directly_observed(self):
        response, data_hash, nonce_hex = _load_fixture()

        class _FixtureProvider(RFC3161Provider):
            def anchor(self, data_hash, nonce):
                return TimeProof(
                    source="rfc3161:fixture",
                    timestamp_ms=1_787_000_000_000,
                    nonce_hex=nonce_hex,
                    response_hex=response.hex(),
                    certificate_chain_hex=None,
                )

        record = TimeAnchorService(_FixtureProvider()).anchor_record(data_hash)
        self.assertEqual(record["status"], "anchored")
        # The daemon relayed a time it did not cryptographically authenticate.
        self.assertEqual(record["apw:proof_level"], "inferred")
        self.assertIs(record["cms_signature_verified"], False)


if __name__ == "__main__":
    unittest.main()


class VerifierTimeAnchorTests(unittest.TestCase):
    """verify_manifest must re-check the retained token, not trust the record."""

    def _record(self):
        from daemon.verify import VerificationResult

        response, data_hash, nonce_hex = _load_fixture()
        record = {
            "status": "anchored",
            "data_hash": data_hash,
            "source": "rfc3161:fixture",
            "timestamp_ms": parse_timestamp_response(response).gentime_ms,
            "nonce_hex": nonce_hex,
            "response_der_hex": response.hex(),
            "cms_signature_verified": False,
            "apw:proof_level": "inferred",
        }
        return VerificationResult(), {"export": {"sha256": data_hash}, "time_anchor": record}

    def _codes(self, mutate):
        from daemon.verify import _check_time_anchor

        result, data = self._record()
        mutate(data)
        _check_time_anchor(data, result)
        return {finding.code for finding in result.findings}

    def test_consistent_token_passes(self):
        self.assertEqual(self._codes(lambda d: None), {"time_anchor_consistent"})

    def test_each_tamper_is_rejected(self):
        tampers = {
            "timestamp": lambda d: d["time_anchor"].update(timestamp_ms=d["time_anchor"]["timestamp_ms"] + 1),
            "data_hash": lambda d: d["export"].update(sha256="0" * 64),
            "non_hex_token": lambda d: d["time_anchor"].update(response_der_hex="zz"),
            "overclaimed_label": lambda d: d["time_anchor"].update(**{"apw:proof_level": "directly_observed"}),
            "claimed_cms": lambda d: d["time_anchor"].update(cms_signature_verified=True),
            "bool_timestamp": lambda d: d["time_anchor"].update(timestamp_ms=True),
        }
        for name, mutate in tampers.items():
            with self.subTest(name):
                self.assertEqual(self._codes(mutate), {"time_anchor_invalid"})

    def test_unavailable_is_informational(self):
        codes = self._codes(lambda d: d["time_anchor"].update(status="unavailable"))
        self.assertEqual(codes, {"time_anchor_unavailable"})
