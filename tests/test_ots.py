"""OpenTimestamps: codec, calendars, Bitcoin header verification and the manifest record.

Vectors: tests/fixtures/parity/ots/ holds unmodified proofs from the reference client's
examples (real). Everything else in ots_vectors.json is constructed and labelled so.
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from daemon import verify as verify_module
from daemon.time_anchor import ots

PARITY = Path(__file__).parent / "fixtures" / "parity"
OTS_DIR = PARITY / "ots"
HELLO = (OTS_DIR / "hello-world.txt.ots").read_bytes()
HEADER_358391 = bytes.fromhex(
    "02000000b96394585a281b7e5f438fd1c9ed492645a1fd61cb3802040000000000000000007ee445d23ad061af4a36b809501fab1ac4f2d7e7a739817dd0cbb7ec661b8a1e376755f58616186272def6"
)


def _generator():
    spec = importlib.util.spec_from_file_location("parity_generator", PARITY / "generate_parity_fixtures.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules["parity_generator"] = module
    spec.loader.exec_module(module)
    return module


class RealProofTests(unittest.TestCase):
    def test_every_real_proof_matches_the_reference_implementation(self):
        upstream = json.loads((PARITY / "ots_upstream.json").read_text())
        self.assertTrue(upstream["produced_by"].startswith("python-opentimestamps"))
        for entry in upstream["proofs"]:
            with self.subTest(entry["name"]):
                data = (OTS_DIR / entry["name"]).read_bytes()
                if not entry["parses"]:
                    with self.assertRaises(ots.DeserializationError):
                        ots.parse_detached(data)
                    continue
                if entry["name"] == "different-blockchains.txt.ots":
                    # Documented divergence: upstream accepts KECCAK-256, this port does not.
                    with self.assertRaisesRegex(ots.DeserializationError, "keccak256"):
                        ots.parse_detached(data)
                    continue
                proof = ots.parse_detached(data)
                self.assertEqual(proof.file_digest.hex(), entry["digest"])
                self.assertEqual(sorted({m.hex() for m in proof.timestamp.messages()}), entry["messages"])
                self.assertEqual(hashlib.sha256(proof.serialize()).hexdigest(), entry["reserialized_sha256"])
                self.assertEqual(proof.serialize() == data, entry["reserialized_equals_input"])
                mine = [(a["type"], a.get("uri", a.get("height"))) for a in ots.attestation_summary(proof.timestamp)
                        if a["type"] in ("pending", "bitcoin")]
                theirs = [(a["type"], a.get("uri", a.get("height"))) for a in entry["attestations"] if a["type"] in ("pending", "bitcoin")]
                self.assertEqual(sorted(mine, key=str), sorted(theirs, key=str))

    def test_source_files_hash_to_the_digests_in_their_proofs(self):
        for name in ("hello-world.txt", "incomplete.txt", "two-calendars.txt", "merkle3.txt", "empty", "sha1-a"):
            with self.subTest(name):
                proof_name = "sha1-a-or-b.ots" if name == "sha1-a" else f"{name}.ots" if name == "empty" else f"{name}.ots"
                proof = ots.parse_detached((OTS_DIR / proof_name).read_bytes())
                data = (OTS_DIR / name).read_bytes()
                algo = {ots.OP_SHA256: "sha256", ots.OP_SHA1: "sha1"}[proof.file_hash_op]
                self.assertEqual(hashlib.new(algo, data).digest(), proof.file_digest)

    def test_real_bitcoin_attestation_verifies_against_the_real_header(self):
        proof = ots.parse_detached(HELLO)
        header = ots.BlockHeader(HEADER_358391)
        self.assertTrue(header.meets_own_target())
        self.assertEqual(header.block_hash().hex(), "000000000000000003e892881a8cdcdc117c06d444057c98b6f04a9ee75a2319")
        checks = ots.check_bitcoin_attestations(proof.timestamp, ots.LocalHeaderSource({358391: HEADER_358391}))
        self.assertEqual([(c.height, c.status, c.block_time) for c in checks], [(358391, "verified", 1432827678)])
        # The digest is in internal byte order: the explorer's display-order root must NOT match.
        self.assertNotEqual(header.merkle_root, header.merkle_root[::-1])

    def test_a_flipped_header_bit_is_not_verified(self):
        proof = ots.parse_detached(HELLO)
        tampered = bytearray(HEADER_358391)
        tampered[40] ^= 1
        checks = ots.check_bitcoin_attestations(proof.timestamp, ots.LocalHeaderSource({358391: bytes(tampered)}))
        self.assertEqual(checks[0].status, "mismatch")


class VectorTests(unittest.TestCase):
    def test_vectors_are_current(self):
        self.assertEqual(_generator().ots_fixture(), json.loads((PARITY / "ots_vectors.json").read_text()))

    def test_vectors_cover_the_finding_codes(self):
        vectors = json.loads((PARITY / "ots_vectors.json").read_text())
        codes = {f["code"] for v in vectors["evaluate"] for f in v["expected_findings"]}
        self.assertEqual(codes, {
            "time_anchor_ots_invalid", "time_anchor_ots_unavailable", "time_anchor_ots_pending",
            "time_anchor_ots_bitcoin_unchecked", "time_anchor_ots_block_verified", "time_anchor_ots_header_unavailable",
        })


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    log: list = []
    upgrade_ready = False

    def log_message(self, *args):
        pass

    def _send(self, status, body=b""):
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        Handler.log.append(("POST", self.path, self.headers.get("Accept"), body))
        uri = b"http://127.0.0.1:%d/cal" % self.server.server_address[1]
        reply = b"\xf0\x08" + b"\x01" * 8 + b"\x08\x00" + ots.TAG_PENDING + bytes([len(uri) + 1, len(uri)]) + uri
        self._send(200, reply)

    def do_GET(self):
        Handler.log.append(("GET", self.path, self.headers.get("Accept"), b""))
        if "/timestamp/" in self.path:
            if not Handler.upgrade_ready:
                return self._send(404, b"Not found")
            return self._send(200, b"\xf1\x02\xaa\xbb\x08\x00" + ots.TAG_BITCOIN + b"\x01\x05")
        if self.path == "/block-height/358391":
            return self._send(200, ots.BlockHeader(HEADER_358391).block_hash().hex().encode())
        if self.path == "/block-height/1":
            return self._send(200, b"11" * 32)
        if self.path == f"/block/{ots.BlockHeader(HEADER_358391).block_hash().hex()}/header":
            return self._send(200, HEADER_358391.hex().encode())
        if self.path == "/block/" + "11" * 32 + "/header":
            return self._send(200, HEADER_358391.hex().encode())  # header that does not hash to that id
        self._send(404)


class NetworkTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.server.daemon_threads = True
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()
        cls.url = f"http://127.0.0.1:{cls.server.server_address[1]}"

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def setUp(self):
        Handler.log = []
        Handler.upgrade_ready = False

    def test_the_calendar_receives_the_nonce_hashed_commitment_not_the_file_digest(self):
        record = ots.OtsAnchorService([self.url], timeout=5).anchor_record(hashlib.sha256(b"export").hexdigest())
        self.assertEqual(record["status"], "pending", record)
        method, path, accept, body = Handler.log[0]
        self.assertEqual((method, path, accept), ("POST", "/digest", "application/vnd.opentimestamps.v1"))
        self.assertEqual(body.hex(), record["commitment_hex"])
        self.assertNotEqual(body.hex(), record["data_hash"])
        self.assertEqual(record["apw:proof_level"], "unknown_unobserved")
        self.assertNotIn("timestamp", record)
        self.assertEqual(record["attestations"], [{"type": "pending", "uri": f"{self.url}/cal"}])

    def test_an_unreachable_calendar_degrades_to_an_unavailable_record(self):
        record = ots.OtsAnchorService(["http://127.0.0.1:1"], timeout=2).anchor_record("ab" * 32)
        self.assertEqual(record["status"], "unavailable")
        self.assertEqual(record["calendars"][0]["status"], "failed")
        self.assertEqual(record["apw:proof_level"], "unknown_unobserved")

    def test_upgrade_contacts_only_allowed_calendars_and_merges_the_reply(self):
        service = ots.OtsAnchorService([self.url], timeout=5)
        record = service.anchor_record(hashlib.sha256(b"export").hexdigest())
        proof = ots.parse_detached(bytes.fromhex(record["proof_hex"]))

        self.assertEqual([o["status"] for o in ots.upgrade(proof, {"https://elsewhere.example"})], ["skipped"])
        self.assertEqual([entry[0] for entry in Handler.log if entry[0] == "GET"], [])

        self.assertEqual([o["status"] for o in ots.upgrade(proof, {self.url + "/cal"}, timeout=5)], ["not_ready"])
        Handler.upgrade_ready = True
        outcomes = ots.upgrade(proof, {self.url + "/cal"}, timeout=5)
        self.assertEqual([o["status"] for o in outcomes], ["upgraded"])
        kinds = {a["type"] for a in ots.attestation_summary(proof.timestamp)}
        self.assertEqual(kinds, {"pending", "bitcoin"})
        # The upgraded proof still serialises, still parses, and still commits to the same file.
        again = ots.parse_detached(proof.serialize())
        self.assertEqual(again.file_digest.hex(), record["data_hash"])

    def test_explorer_source_returns_a_header_that_hashes_to_the_named_block(self):
        source = ots.ExplorerHeaderSource(self.url, timeout=5)
        self.assertEqual(source.header_at(358391).raw, HEADER_358391)
        with self.assertRaisesRegex(LookupError, "does not hash"):
            source.header_at(1)
        with self.assertRaises(LookupError):
            source.header_at(999)
        proof = ots.parse_detached(HELLO)
        checks = ots.check_bitcoin_attestations(proof.timestamp, source)
        self.assertEqual([(c.status, c.source) for c in checks], [("verified", "explorer")])


class VerifierWiringTests(unittest.TestCase):
    def test_verify_reports_the_pending_record(self):
        vectors = json.loads((PARITY / "ots_vectors.json").read_text())["evaluate"]
        case = next(v for v in vectors if v["name"] == "real_pending_proof")
        result = verify_module.VerificationResult()
        verify_module._check_time_anchor_opentimestamps(case["manifest"], result)
        self.assertEqual([(f.severity.value if hasattr(f.severity, "value") else f.severity, f.code) for f in result.findings],
                         [("info", "time_anchor_ots_pending")])

    def test_the_verify_cli_flags_are_accepted(self):
        args = ["--help"]
        with self.assertRaises(SystemExit):
            verify_module.main(args)


if __name__ == "__main__":
    unittest.main()
