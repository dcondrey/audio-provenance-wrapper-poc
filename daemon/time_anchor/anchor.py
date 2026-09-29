from __future__ import annotations

import abc
import logging
import secrets
from dataclasses import dataclass
from datetime import datetime, timezone

from daemon.time_anchor.http import fetch as http_fetch

log = logging.getLogger(__name__)

TSA_TIMEOUT_SECONDS = 10
MAX_TSA_RESPONSE_BYTES = 64 * 1024
DEFAULT_TSA_URL = "http://timestamp.digicert.com"

# ASN.1 DER constants for RFC 3161 (see RFC 3161 §2.4.1/§2.4.2).
_SHA256_ALGORITHM_OID = bytes.fromhex("0609608648016503040201")  # 2.16.840.1.101.3.4.2.1
_TAG_SEQUENCE = 0x30
_TAG_SET = 0x31
_TAG_INTEGER = 0x02
_TAG_OCTET_STRING = 0x04
_TAG_NULL = 0x05
_TAG_OID = 0x06
_TAG_GENERALIZED_TIME = 0x18
_TAG_CONTEXT_0 = 0xA0


@dataclass(frozen=True)
class TimeProof:
    """An externally-verifiable timestamp from an independent time source."""

    source: str
    timestamp_ms: int
    nonce_hex: str
    response_hex: str
    certificate_chain_hex: str | None


class TimeAnchorProvider(abc.ABC):
    """Abstract interface for external time anchoring."""

    @abc.abstractmethod
    def anchor(self, data_hash: str, nonce: bytes) -> TimeProof:
        """Request a timestamp proof for the given data hash."""

    @abc.abstractmethod
    def verify(self, proof: TimeProof, data_hash: str) -> bool:
        """Verify a timestamp proof against the original data hash."""


def _der_length(length: int) -> bytes:
    if length < 0x80:
        return bytes([length])
    body = length.to_bytes((length.bit_length() + 7) // 8, "big")
    return bytes([0x80 | len(body)]) + body


def _der(tag: int, content: bytes) -> bytes:
    return bytes([tag]) + _der_length(len(content)) + content


def _der_integer(value: int) -> bytes:
    body = value.to_bytes((value.bit_length() + 8) // 8 or 1, "big")
    return _der(_TAG_INTEGER, body)


def _der_read(data: bytes, offset: int) -> tuple[int, bytes, int]:
    """Read one TLV; return (tag, content, offset past it). Raises ValueError on malformed input."""
    if offset + 2 > len(data):
        raise ValueError("DER value overruns the buffer")
    tag = data[offset]
    first = data[offset + 1]
    if first < 0x80:
        length, header = first, 2
    else:
        n = first & 0x7F
        if n == 0 or n > 4:
            raise ValueError("unsupported DER length encoding")
        if offset + 2 + n > len(data):
            raise ValueError("DER length overruns the buffer")
        length = int.from_bytes(data[offset + 2 : offset + 2 + n], "big")
        header = 2 + n
    start = offset + header
    end = start + length
    if end > len(data):
        raise ValueError("DER value overruns the buffer")
    return tag, data[start:end], end


def _der_children(content: bytes) -> list[tuple[int, bytes]]:
    children: list[tuple[int, bytes]] = []
    offset = 0
    while offset < len(content):
        tag, inner, offset = _der_read(content, offset)
        children.append((tag, inner))
    return children


def encode_timestamp_request(data_hash_hex: str, nonce: bytes) -> bytes:
    """RFC 3161 TimeStampReq: v1, SHA-256 imprint, nonce, certReq TRUE."""
    hashed_message = bytes.fromhex(data_hash_hex)
    if len(hashed_message) != 32:
        raise ValueError("data hash must be a 64-character SHA-256 hex digest")
    message_imprint = _der(
        _TAG_SEQUENCE,
        _der(_TAG_SEQUENCE, _SHA256_ALGORITHM_OID + _der(_TAG_NULL, b""))
        + _der(_TAG_OCTET_STRING, hashed_message),
    )
    return _der(
        _TAG_SEQUENCE,
        _der_integer(1)
        + message_imprint
        + _der_integer(int.from_bytes(nonce, "big"))
        + _der(0x01, b"\xff"),
    )


@dataclass(frozen=True)
class _ParsedTimestampResponse:
    granted: bool
    status: int
    gentime_ms: int | None
    imprint_hash_hex: str | None
    nonce: int | None


def _expect(children: list[tuple[int, bytes]], index: int, tag: int, what: str) -> bytes:
    if index >= len(children) or children[index][0] != tag:
        raise ValueError(f"malformed TimeStampResp: expected {what}")
    return children[index][1]


def parse_timestamp_response(data: bytes) -> _ParsedTimestampResponse:
    """Extract PKIStatus, genTime, message imprint, and nonce from a TimeStampResp.

    This is a structural parse only; the CMS signature over the token is NOT
    verified here and remains for downstream auditors with the TSA chain.
    """
    tag, response_content, _ = _der_read(data, 0)
    if tag != _TAG_SEQUENCE:
        raise ValueError("malformed TimeStampResp: not a SEQUENCE")
    response_children = _der_children(response_content)
    status_children = _der_children(_expect(response_children, 0, _TAG_SEQUENCE, "PKIStatusInfo"))
    status = int.from_bytes(_expect(status_children, 0, _TAG_INTEGER, "PKIStatus"), "big")
    granted = status in (0, 1)  # granted / grantedWithMods
    if not granted or len(response_children) < 2:
        return _ParsedTimestampResponse(
            granted=False, status=status, gentime_ms=None, imprint_hash_hex=None, nonce=None
        )

    # token: ContentInfo{ OID signedData, [0]{ SignedData{ version, digestAlgorithms,
    #   encapContentInfo{ OID tstInfo, [0]{ OCTET STRING{ TSTInfo } } }, ... } } }
    content_info = _der_children(_expect(response_children, 1, _TAG_SEQUENCE, "TimeStampToken"))
    _expect(content_info, 0, _TAG_OID, "signedData OID")
    signed_data_wrap = _der_children(_expect(content_info, 1, _TAG_CONTEXT_0, "SignedData wrapper"))
    signed_data = _der_children(_expect(signed_data_wrap, 0, _TAG_SEQUENCE, "SignedData"))
    _expect(signed_data, 0, _TAG_INTEGER, "SignedData version")
    _expect(signed_data, 1, _TAG_SET, "digestAlgorithms")
    encap_children = _der_children(_expect(signed_data, 2, _TAG_SEQUENCE, "encapContentInfo"))
    _expect(encap_children, 0, _TAG_OID, "TSTInfo OID")
    econtent = _der_children(_expect(encap_children, 1, _TAG_CONTEXT_0, "eContent wrapper"))
    tstinfo_der = _expect(econtent, 0, _TAG_OCTET_STRING, "TSTInfo octets")
    tst_tag, tstinfo_content, _ = _der_read(tstinfo_der, 0)
    if tst_tag != _TAG_SEQUENCE:
        raise ValueError("malformed TimeStampResp: TSTInfo is not a SEQUENCE")
    tstinfo = _der_children(tstinfo_content)

    # TSTInfo: version, policy, messageImprint, serialNumber, genTime,
    #          accuracy?, ordering?, nonce?, tsa?, extensions?
    imprint_children = _der_children(_expect(tstinfo, 2, _TAG_SEQUENCE, "messageImprint"))
    imprint_algorithm = _expect(imprint_children, 0, _TAG_SEQUENCE, "imprint AlgorithmIdentifier")
    if not imprint_algorithm.startswith(_SHA256_ALGORITHM_OID):
        raise ValueError("TSTInfo message imprint does not use SHA-256")
    imprint_hash_hex = _expect(imprint_children, 1, _TAG_OCTET_STRING, "hashedMessage").hex()
    gentime_index = next(
        (i for i, (child_tag, _) in enumerate(tstinfo) if child_tag == _TAG_GENERALIZED_TIME),
        None,
    )
    if gentime_index is None:
        raise ValueError("malformed TimeStampResp: TSTInfo has no genTime")
    text = tstinfo[gentime_index][1].decode("ascii")
    base, _sep, _fraction = text.rstrip("Z").partition(".")
    parsed = datetime.strptime(base, "%Y%m%d%H%M%S").replace(tzinfo=timezone.utc)
    gentime_ms = int(parsed.timestamp() * 1000)
    # serialNumber sits before genTime; the only INTEGER after it is the nonce.
    nonce = next(
        (
            int.from_bytes(content, "big")
            for child_tag, content in tstinfo[gentime_index + 1 :]
            if child_tag == _TAG_INTEGER
        ),
        None,
    )
    return _ParsedTimestampResponse(
        granted=True,
        status=status,
        gentime_ms=gentime_ms,
        imprint_hash_hex=imprint_hash_hex,
        nonce=nonce,
    )


class RFC3161Provider(TimeAnchorProvider):
    """RFC 3161 Time-Stamp Authority client over plain HTTP POST.

    The DER TimeStampResp is retained verbatim in the proof so downstream
    auditors can verify the CMS signature against the TSA chain; this client
    checks only that the token's message imprint matches the anchored hash.
    """

    def __init__(self, tsa_url: str = DEFAULT_TSA_URL) -> None:
        self.tsa_url = tsa_url

    def anchor(self, data_hash: str, nonce: bytes) -> TimeProof:
        request_der = encode_timestamp_request(data_hash, nonce)
        response = http_fetch(
            self.tsa_url,
            method="POST",
            data=request_der,
            headers={"Content-Type": "application/timestamp-query", "Accept": "application/timestamp-reply"},
            timeout=TSA_TIMEOUT_SECONDS,
            max_bytes=MAX_TSA_RESPONSE_BYTES,
        )
        # Redirects are refused, not followed: a 3xx lands here as an error.
        if response.status != 200:
            raise ValueError(f"the TSA answered with HTTP status line {response.status_line!r}")
        body = response.body
        if len(body) > MAX_TSA_RESPONSE_BYTES:
            raise ValueError("TSA response exceeds the size bound")
        parsed = parse_timestamp_response(body)
        if not parsed.granted:
            raise ValueError(f"TSA refused the timestamp request (PKIStatus={parsed.status})")
        if parsed.imprint_hash_hex != data_hash:
            raise ValueError("TSA token message imprint does not match the anchored hash")
        # RFC 3161 §2.4.2: the nonce echo is the replay protection; a token
        # without our nonce could be a replayed old token.
        if parsed.nonce != int.from_bytes(nonce, "big"):
            raise ValueError("TSA token nonce does not match the request nonce")
        # IMPORTANT: never fall back to the local clock. A record labelled with a
        # TSA source must carry the TSA's own time, not this machine's.
        if parsed.gentime_ms is None:
            raise ValueError("TSA token has no genTime")
        return TimeProof(
            source=f"rfc3161:{self.tsa_url}",
            timestamp_ms=parsed.gentime_ms,
            nonce_hex=nonce.hex(),
            response_hex=body.hex(),
            certificate_chain_hex=None,
        )

    def verify(self, proof: TimeProof, data_hash: str) -> bool:
        try:
            parsed = parse_timestamp_response(bytes.fromhex(proof.response_hex))
            expected_nonce = int.from_bytes(bytes.fromhex(proof.nonce_hex), "big")
        except (ValueError, IndexError, UnicodeDecodeError):
            return False
        return (
            parsed.granted
            and parsed.imprint_hash_hex == data_hash
            and parsed.nonce == expected_nonce
            # Bind the reported time to the retained token: a timestamp_ms edited
            # apart from the DER must not verify.
            and parsed.gentime_ms == proof.timestamp_ms
        )


class TimeAnchorService:
    """Anchors a data hash at an external RFC 3161 TSA and renders the
    manifest-ready record."""

    def __init__(self, tsa: TimeAnchorProvider | None = None) -> None:
        self.tsa = tsa or RFC3161Provider()

    def anchor_record(self, data_hash: str) -> dict[str, object]:
        """Manifest-ready time anchor for *data_hash*; degrades to an explicit
        unavailable record instead of raising."""
        # REQUIRED: the nonce is the replay protection, so it must be
        # unpredictable; a hash of known inputs lets an attacker pre-fetch a
        # backdated token carrying the predicted nonce.
        nonce = secrets.token_bytes(16)
        try:
            proof = self.tsa.anchor(data_hash, nonce)
        except Exception as exc:
            log.warning("Time anchoring failed: %s", exc)
            return {
                "status": "unavailable",
                "data_hash": data_hash,
                "reason": str(exc),
                "scope": "No timestamp token was obtained; no external time is asserted.",
                "apw:proof_level": "unknown_unobserved",
            }
        return {
            "status": "anchored",
            "data_hash": data_hash,
            "source": proof.source,
            "timestamp_ms": proof.timestamp_ms,
            "timestamp": datetime.fromtimestamp(proof.timestamp_ms / 1000, timezone.utc)
            .isoformat()
            .replace("+00:00", "Z"),
            "nonce_hex": proof.nonce_hex,
            "response_der_hex": proof.response_hex,
            "cms_signature_verified": False,
            "scope": (
                "The retained DER TimeStampResp is externally verifiable against the "
                "TSA certificate chain; this daemon checked only that the token's "
                "message imprint and nonce match the request. It did NOT verify the "
                "token's CMS signature, so the time is a relayed third-party "
                "assertion, not a firsthand or cryptographically authenticated "
                "observation. Over plaintext HTTP an on-path attacker can substitute "
                "a forged token; verify the CMS signature against the TSA chain "
                "downstream before relying on the timestamp."
            ),
            # IMPORTANT: not directly_observed. The daemon relayed a third-party
            # time assertion it did not cryptographically authenticate; inferred
            # is the honest ceiling until the CMS signature is verified.
            "apw:proof_level": "inferred",
        }
