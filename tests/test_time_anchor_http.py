"""The bounded HTTP client behind the time-anchor providers (daemon/time_anchor/http.py)."""
from __future__ import annotations

import datetime
import ipaddress
import socket
import ssl
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

from daemon.time_anchor.http import fetch


def self_signed(names: list[str]) -> tuple[bytes, bytes]:
    key = ec.generate_private_key(ec.SECP256R1())
    subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, names[0])])
    alt = [x509.DNSName(n) for n in names] + [x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]
    now = datetime.datetime.now(datetime.timezone.utc)
    certificate = (
        x509.CertificateBuilder()
        .subject_name(subject).issuer_name(subject).public_key(key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - datetime.timedelta(minutes=5))
        .not_valid_after(now + datetime.timedelta(days=1))
        .add_extension(x509.SubjectAlternativeName(alt), critical=False)
        .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
        .sign(key, hashes.SHA256())
    )
    return (
        certificate.public_bytes(serialization.Encoding.PEM),
        key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                          serialization.NoEncryption()),
    )


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):  # noqa: D102
        pass

    def _reply(self):
        route = self.path.split("?")[0]
        if route == "/ok":
            body = b"hello"
            self.send_response(200)
        elif route == "/redirect":
            body = b""
            self.send_response(302)
            self.send_header("Location", "http://127.0.0.1:1/elsewhere")
        elif route == "/big":
            body = b"x" * 5000
            self.send_response(200)
        elif route == "/short":
            body = b"abc"
            self.send_response(200)
            self.send_header("Content-Length", "50")
            self.end_headers()
            self.wfile.write(body)
            self.close_connection = True
            return
        elif route == "/slow":
            time.sleep(3)
            body = b"late"
            self.send_response(200)
        else:
            body = b""
            self.send_response(404)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("X-Method", self.command)
        self.end_headers()
        self.wfile.write(body)

    do_GET = _reply

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self._reply()


def serve(context: ssl.SSLContext | None = None) -> tuple[ThreadingHTTPServer, int]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    if context is not None:
        server.socket = context.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, server.server_address[1]


class PlainHttpTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server, cls.port = serve()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def url(self, path: str) -> str:
        return f"http://127.0.0.1:{self.port}{path}"

    def test_get_and_post(self):
        got = fetch(self.url("/ok"), max_bytes=100)
        self.assertEqual((got.status, got.body), (200, b"hello"))
        posted = fetch(self.url("/ok"), method="POST", data=b"payload", max_bytes=100)
        self.assertEqual(posted.body, b"hello")

    def test_redirects_are_returned_not_followed(self):
        result = fetch(self.url("/redirect"), max_bytes=100)
        self.assertEqual(result.status, 302)

    def test_body_is_capped_one_byte_past_the_bound(self):
        self.assertEqual(len(fetch(self.url("/big"), max_bytes=100).body), 101)
        self.assertEqual(len(fetch(self.url("/big"), max_bytes=5000).body), 5000)
        self.assertEqual(len(fetch(self.url("/big"), max_bytes=4999).body), 5000)

    def test_truncated_body_and_slow_server_fail(self):
        with self.assertRaises(OSError):
            fetch(self.url("/short"), max_bytes=100)
        started = time.monotonic()
        with self.assertRaises(OSError):
            fetch(self.url("/slow"), max_bytes=100, timeout=0.5)
        self.assertLess(time.monotonic() - started, 2.5)

    def test_bad_urls_are_refused(self):
        for url in ("ftp://x/", "http://", "http://host:notaport/", "http://user@host/", "http://ho st/"):
            with self.subTest(url):
                with self.assertRaises((ValueError, OSError)):
                    fetch(url, max_bytes=10)


class TlsTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)

    def start(self, names: list[str]) -> tuple[int, bytes]:
        certificate, key = self_signed(names)
        root = Path(self.directory.name)
        (root / "cert.pem").write_bytes(certificate)
        (root / "key.pem").write_bytes(key)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(root / "cert.pem", root / "key.pem")
        server, port = serve(context)
        self.addCleanup(server.shutdown)
        return port, certificate

    def test_certificate_validation_is_on_by_default(self):
        port, _ = self.start(["localhost"])
        with self.assertRaises(OSError) as raised:
            fetch(f"https://localhost:{port}/ok", max_bytes=100)
        self.assertIn("CERTIFICATE_VERIFY_FAILED", str(raised.exception))

    def test_a_trusted_self_signed_root_is_accepted_only_when_injected(self):
        port, certificate = self.start(["localhost"])
        context = ssl.create_default_context(cadata=certificate.decode())
        result = fetch(f"https://localhost:{port}/ok", max_bytes=100, context=context)
        self.assertEqual((result.status, result.body), (200, b"hello"))

    def test_a_trusted_root_does_not_excuse_a_hostname_mismatch(self):
        port, certificate = self.start(["other.example"])
        context = ssl.create_default_context(cadata=certificate.decode())
        with self.assertRaises(OSError) as raised:
            fetch(f"https://localhost:{port}/ok", max_bytes=100, context=context)
        self.assertIn("CERTIFICATE_VERIFY_FAILED", str(raised.exception))

    def test_https_redirect_to_http_is_not_followed(self):
        port, certificate = self.start(["localhost"])
        context = ssl.create_default_context(cadata=certificate.decode())
        result = fetch(f"https://localhost:{port}/redirect", max_bytes=100, context=context)
        self.assertEqual(result.status, 302)


if __name__ == "__main__":
    unittest.main()
