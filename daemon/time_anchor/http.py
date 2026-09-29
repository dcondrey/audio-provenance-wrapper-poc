"""A small bounded HTTP client shared by the time-anchor providers.

Mirrors rust/apw-daemon/src/http.rs so both daemons behave the same:

- one overall deadline covers connect, TLS handshake, write and read;
- the body is capped, and one byte past the cap is kept so a caller can tell an
  oversized reply from one exactly at the bound;
- redirects are never followed. A 3xx is returned as a status and every caller
  treats it as a failure, so an endpoint cannot bounce a request to another host
  and https can never downgrade to http (urllib would follow both);
- certificate validation is always on. ``context`` exists for tests that trust a
  self-signed server; no command-line flag reaches it.
"""
from __future__ import annotations

import http.client
import ssl
import time
from dataclasses import dataclass
from urllib.parse import urlsplit


@dataclass(frozen=True)
class HttpResult:
    status: int
    status_line: str
    body: bytes


def fetch(
    url: str,
    *,
    method: str = "GET",
    data: bytes | None = None,
    headers: dict[str, str] | None = None,
    timeout: float = 10.0,
    max_bytes: int,
    context: ssl.SSLContext | None = None,
) -> HttpResult:
    """One request. Raises OSError/ValueError on transport failure or a bad URL."""
    parts = urlsplit(url)
    if parts.scheme not in ("http", "https"):
        raise ValueError(f"unsupported URL scheme in {url!r}: only http and https are spoken")
    if not parts.hostname or parts.username is not None or parts.password is not None:
        raise ValueError(f"invalid URL {url!r}")
    try:
        port = parts.port
    except ValueError as error:
        raise ValueError(f"invalid URL port in {url!r}") from error
    path = parts.path or "/"
    if parts.query:
        path += "?" + parts.query
    if any(ch in path for ch in "\r\n ") or any(ch in parts.hostname for ch in "\r\n \t"):
        raise ValueError(f"invalid URL {url!r}")

    deadline = time.monotonic() + timeout

    def remaining() -> float:
        left = deadline - time.monotonic()
        if left <= 0:
            raise TimeoutError("the request timed out")
        return left

    if parts.scheme == "https":
        connection: http.client.HTTPConnection = http.client.HTTPSConnection(
            parts.hostname, port, timeout=remaining(), context=context or ssl.create_default_context()
        )
    else:
        connection = http.client.HTTPConnection(parts.hostname, port, timeout=remaining())
    try:
        connection.connect()  # includes the TLS handshake for https
        if connection.sock is not None:
            connection.sock.settimeout(remaining())
        connection.request(method, path, body=data, headers={"Connection": "close", **(headers or {})})
        if connection.sock is not None:
            connection.sock.settimeout(remaining())
        response = connection.getresponse()
        body = bytearray()
        while len(body) <= max_bytes:
            if connection.sock is not None:
                connection.sock.settimeout(remaining())
            chunk = response.read(min(4096, max_bytes + 1 - len(body)))
            if not chunk:
                break
            body.extend(chunk)
        declared = response.getheader("Content-Length")
        if declared is not None and not response.chunked:
            if not declared.isdigit():
                raise OSError("the reply has an invalid Content-Length")
            # Reading stops at the cap, so a body longer than the bound is
            # truncated to just past it; only a genuinely short body is an error.
            if len(body) < int(declared) and len(body) <= max_bytes:
                raise OSError("the reply body is shorter than its Content-Length")
        version = "1.1" if response.version == 11 else "1.0"
        return HttpResult(response.status, f"HTTP/{version} {response.status} {response.reason}", bytes(body))
    except http.client.IncompleteRead as error:
        raise OSError("the reply body is shorter than its Content-Length") from error
    except http.client.HTTPException as error:
        raise OSError(f"malformed HTTP reply: {error}") from error
    finally:
        connection.close()
