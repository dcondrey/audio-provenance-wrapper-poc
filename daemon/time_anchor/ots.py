"""OpenTimestamps: detached timestamp file codec, calendar client, Bitcoin header
verification and the manifest record.

The wire format follows the published python-opentimestamps source
(opentimestamps/core/{serialize,op,notary,timestamp}.py) and the calendar protocol
in opentimestamps/calendar.py. This module is independent of that package; its
output is cross-checked against it by tests/fixtures/parity/ots_upstream.json.

Deliberate differences from upstream, mirrored in rust/apw-core/src/ots.rs:
- KECCAK-256 (0x67) is rejected as an unsupported operation (upstream needs
  pycryptodome for it). RIPEMD-160 is used through hashlib, so it needs an
  OpenSSL build that still provides it.
- LEB128 integers are bounded to 63 bits; upstream's reader is unbounded.

HONESTY: a calendar receipt promises a later Bitcoin attestation; it asserts no
time. A completed proof's time is the miner-set timestamp of the attesting block,
an upper bound ("existed no later than"), never an earlier time.
"""
from __future__ import annotations

import concurrent.futures
import hashlib
import logging
import secrets
import time
from dataclasses import dataclass, field
from typing import Callable, Protocol

from daemon.time_anchor.http import fetch as http_fetch

log = logging.getLogger(__name__)

HEADER_MAGIC = b"\x00OpenTimestamps\x00\x00Proof\x00\xbf\x89\xe2\xe8\x84\xe8\x92\x94"
MAJOR_VERSION = 1
MAX_MSG_LENGTH = 4096
MAX_RESULT_LENGTH = 4096
MAX_HEXLIFY_MSG_LENGTH = MAX_RESULT_LENGTH // 2
MAX_ATTESTATION_PAYLOAD = 8192
MAX_URI_LENGTH = 1000
RECURSION_LIMIT = 256
MAX_VARUINT_BITS = 63
ALLOWED_URI_CHARS = frozenset(
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._/:"
)

TAG_PENDING = bytes.fromhex("83dfe30d2ef90c8e")
TAG_BITCOIN = bytes.fromhex("0588960d73d71901")
TAG_LITECOIN = bytes.fromhex("06869a0d73d71b45")
TAG_ETHEREUM = bytes.fromhex("30fe8087b5c7ead7")

OP_SHA1 = 0x02
OP_RIPEMD160 = 0x03
OP_SHA256 = 0x08
OP_KECCAK256 = 0x67
OP_APPEND = 0xF0
OP_PREPEND = 0xF1
OP_REVERSE = 0xF2
OP_HEXLIFY = 0xF3
_HASH_NAMES = {OP_SHA1: ("sha1", 20), OP_RIPEMD160: ("ripemd160", 20), OP_SHA256: ("sha256", 32)}
_OP_NAMES = {
    OP_SHA1: "sha1", OP_RIPEMD160: "ripemd160", OP_SHA256: "sha256", OP_KECCAK256: "keccak256",
    OP_APPEND: "append", OP_PREPEND: "prepend", OP_REVERSE: "reverse", OP_HEXLIFY: "hexlify",
}

DEFAULT_CALENDARS = (
    "https://alice.btc.calendar.opentimestamps.org",
    "https://bob.btc.calendar.opentimestamps.org",
    "https://finney.calendar.eternitywall.com",
)
CALENDAR_TIMEOUT_SECONDS = 10
MAX_CALENDAR_RESPONSE_BYTES = 10_000
USER_AGENT = "audio-provenance-daemon"
CALENDAR_HEADERS = {"Accept": "application/vnd.opentimestamps.v1", "User-Agent": USER_AGENT}

PROOF_LEVELS = frozenset({"inferred", "unknown_unobserved"})


class DeserializationError(ValueError):
    """The proof is malformed or exceeds a bound."""


# ------------------------------- byte reader/writer -------------------------------
class _Reader:
    def __init__(self, data: bytes) -> None:
        self.data = data
        self.offset = 0

    def bytes(self, count: int) -> bytes:
        end = self.offset + count
        if count < 0 or end > len(self.data):
            raise DeserializationError(
                f"Tried to read {count} bytes but got only {max(0, len(self.data) - self.offset)} bytes"
            )
        chunk = self.data[self.offset:end]
        self.offset = end
        return chunk

    def uint8(self) -> int:
        return self.bytes(1)[0]

    def varuint(self) -> int:
        value = shift = 0
        while True:
            byte = self.uint8()
            value |= (byte & 0x7F) << shift
            if value >> MAX_VARUINT_BITS:
                raise DeserializationError("varuint exceeds 63 bits")
            if not byte & 0x80:
                return value
            shift += 7

    def varbytes(self, max_length: int, min_length: int = 0) -> bytes:
        length = self.varuint()
        if length > max_length:
            raise DeserializationError(f"varbytes max length exceeded; {length} > {max_length}")
        if length < min_length:
            raise DeserializationError(f"varbytes min length not met; {length} < {min_length}")
        return self.bytes(length)

    def assert_eof(self) -> None:
        if self.offset != len(self.data):
            raise DeserializationError("Trailing garbage found after end of deserialized data")


def _varuint(value: int) -> bytes:
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def _varbytes(value: bytes) -> bytes:
    return _varuint(len(value)) + value


# ------------------------------------ operations ------------------------------------
Op = tuple  # (tag: int, arg: bytes | None)


def _apply(op: Op, msg: bytes) -> bytes:
    tag, arg = op
    if len(msg) > (MAX_HEXLIFY_MSG_LENGTH if tag == OP_HEXLIFY else MAX_MSG_LENGTH):
        raise DeserializationError(f"Message too long for {_OP_NAMES[tag]}; {len(msg)}")
    if tag == OP_APPEND:
        result = msg + arg
    elif tag == OP_PREPEND:
        result = arg + msg
    elif tag == OP_REVERSE:
        if not msg:
            raise DeserializationError("Can't reverse an empty message")
        result = msg[::-1]
    elif tag == OP_HEXLIFY:
        if not msg:
            raise DeserializationError("Can't hexlify an empty message")
        result = msg.hex().encode()
    elif tag in _HASH_NAMES:
        try:
            result = hashlib.new(_HASH_NAMES[tag][0], msg).digest()
        except ValueError as error:
            raise DeserializationError(f"{_OP_NAMES[tag]} is not available in this Python build") from error
    else:
        raise DeserializationError(f"Unknown operation tag 0x{tag:02x}")
    if len(result) > MAX_RESULT_LENGTH:
        raise DeserializationError(f"Result too long; {len(result)} > {MAX_RESULT_LENGTH}")
    return result


def _read_op(reader: _Reader, tag: int) -> Op:
    if tag in (OP_APPEND, OP_PREPEND):
        return (tag, reader.varbytes(MAX_RESULT_LENGTH, min_length=1))
    if tag in (OP_REVERSE, OP_HEXLIFY, OP_SHA1, OP_RIPEMD160, OP_SHA256):
        return (tag, None)
    if tag == OP_KECCAK256:
        raise DeserializationError("keccak256 is not supported")
    raise DeserializationError(f"Unknown operation tag 0x{tag:02x}")


def _write_op(op: Op) -> bytes:
    tag, arg = op
    return bytes([tag]) + (_varbytes(arg) if arg is not None else b"")


def _op_key(op: Op) -> tuple:
    return (op[0], op[1] or b"")


# ---------------------------------- attestations ----------------------------------
@dataclass(frozen=True)
class Attestation:
    """kind: pending | bitcoin | litecoin | ethereum | unknown."""

    kind: str
    tag: bytes
    uri: str = ""
    height: int = 0
    payload: bytes = b""

    def sort_key(self) -> tuple:
        if self.kind == "pending":
            return (self.tag, self.uri.encode())
        if self.kind == "unknown":
            return (self.tag, self.payload)
        return (self.tag, self.height)

    def serialize(self) -> bytes:
        if self.kind == "pending":
            payload = _varbytes(self.uri.encode())
        elif self.kind == "unknown":
            payload = self.payload
        else:
            payload = _varuint(self.height)
        return self.tag + _varbytes(payload)


def _read_attestation(reader: _Reader) -> Attestation:
    tag = reader.bytes(8)
    payload = reader.varbytes(MAX_ATTESTATION_PAYLOAD)
    inner = _Reader(payload)
    if tag == TAG_PENDING:
        uri = inner.varbytes(MAX_URI_LENGTH)
        if any(byte not in ALLOWED_URI_CHARS for byte in uri):
            raise DeserializationError("Invalid URI: contains a disallowed character")
        inner.assert_eof()
        return Attestation("pending", tag, uri=uri.decode())
    for kind, known in (("bitcoin", TAG_BITCOIN), ("litecoin", TAG_LITECOIN), ("ethereum", TAG_ETHEREUM)):
        if tag == known:
            height = inner.varuint()
            inner.assert_eof()
            return Attestation(kind, tag, height=height)
    return Attestation("unknown", tag, payload=payload)


def pending(uri: str) -> Attestation:
    encoded = uri.encode()
    if len(encoded) > MAX_URI_LENGTH or any(byte not in ALLOWED_URI_CHARS for byte in encoded):
        raise ValueError("calendar URI is too long or uses characters an OpenTimestamps proof cannot carry")
    return Attestation("pending", TAG_PENDING, uri=uri)


# ------------------------------------ timestamp ------------------------------------
@dataclass
class Timestamp:
    msg: bytes
    attestations: set = field(default_factory=set)
    ops: dict = field(default_factory=dict)  # op key -> (op, Timestamp)

    def add_op(self, op: Op) -> "Timestamp":
        key = _op_key(op)
        if key not in self.ops:
            self.ops[key] = (op, Timestamp(_apply(op, self.msg)))
        return self.ops[key][1]

    def merge(self, other: "Timestamp") -> None:
        if self.msg != other.msg:
            raise ValueError("Can't merge timestamps for different messages together")
        self.attestations |= other.attestations
        for key, (op, child) in other.ops.items():
            self.add_op(op).merge(child)

    def serialize(self) -> bytes:
        if not self.attestations and not self.ops:
            raise ValueError("An empty timestamp can't be serialized")
        out = bytearray()
        attestations = sorted(self.attestations, key=Attestation.sort_key)
        for attestation in attestations[:-1]:
            out += b"\xff\x00" + attestation.serialize()
        if not self.ops:
            out += b"\x00" + attestations[-1].serialize()
        else:
            if attestations:
                out += b"\xff\x00" + attestations[-1].serialize()
            ordered = sorted(self.ops.items(), key=lambda item: item[0])
            for _key, (op, child) in ordered[:-1]:
                out += b"\xff" + _write_op(op) + child.serialize()
            _key, (op, child) = ordered[-1]
            out += _write_op(op) + child.serialize()
        return bytes(out)

    def walk(self):
        """Every (msg, attestation) leaf, depth first."""
        for attestation in sorted(self.attestations, key=Attestation.sort_key):
            yield self.msg, attestation
        for _key, (_op, child) in sorted(self.ops.items(), key=lambda item: item[0]):
            yield from child.walk()

    def messages(self):
        yield self.msg
        for _key, (_op, child) in self.ops.items():
            yield from child.messages()


def _read_timestamp(reader: _Reader, msg: bytes, limit: int = RECURSION_LIMIT) -> Timestamp:
    if not limit:
        raise DeserializationError("Reached timestamp recursion depth limit while deserializing")
    if len(msg) > MAX_MSG_LENGTH:
        raise DeserializationError(f"Message exceeds Op length limit; {len(msg)} > {MAX_MSG_LENGTH}")
    stamp = Timestamp(bytes(msg))

    def one(tag: int) -> None:
        if tag == 0x00:
            stamp.attestations.add(_read_attestation(reader))
            return
        op = _read_op(reader, tag)
        result = _apply(op, msg)
        child = _read_timestamp(reader, result, limit - 1)
        key = _op_key(op)
        stamp.ops[key] = (op, child)

    tag = reader.uint8()
    while tag == 0xFF:
        one(reader.uint8())
        tag = reader.uint8()
    one(tag)
    return stamp


def parse_timestamp(data: bytes, msg: bytes) -> Timestamp:
    """A bare timestamp (a calendar reply) for the known message *msg*."""
    reader = _Reader(data)
    stamp = _read_timestamp(reader, msg)
    reader.assert_eof()
    return stamp


@dataclass
class DetachedTimestampFile:
    file_hash_op: int
    timestamp: Timestamp

    @property
    def file_digest(self) -> bytes:
        return self.timestamp.msg

    def serialize(self) -> bytes:
        name, length = _HASH_NAMES[self.file_hash_op]
        if len(self.timestamp.msg) != length:
            raise ValueError("Timestamp message length and file_hash_op digest length differ")
        return HEADER_MAGIC + bytes([MAJOR_VERSION, self.file_hash_op]) + self.timestamp.msg + self.timestamp.serialize()


def parse_detached(data: bytes) -> DetachedTimestampFile:
    if data[: len(HEADER_MAGIC)] != HEADER_MAGIC:
        raise DeserializationError("Expected magic bytes for an OpenTimestamps detached proof")
    reader = _Reader(data)
    reader.bytes(len(HEADER_MAGIC))
    major = reader.uint8()
    if major != MAJOR_VERSION:
        raise DeserializationError(f"Version {major} detached timestamp files are not supported")
    op_tag = reader.uint8()
    if op_tag not in _HASH_NAMES:
        raise DeserializationError(f"Unknown file hash operation tag 0x{op_tag:02x}")
    digest = reader.bytes(_HASH_NAMES[op_tag][1])
    stamp = _read_timestamp(reader, digest)
    reader.assert_eof()
    return DetachedTimestampFile(op_tag, stamp)


def attestation_summary(stamp: Timestamp) -> list[dict[str, object]]:
    """The distinct attestations in serialization order, as the manifest records them."""
    seen: set = set()
    out: list[dict[str, object]] = []
    for _msg, attestation in stamp.walk():
        key = (attestation.tag, attestation.sort_key())
        if key in seen:
            continue
        seen.add(key)
        if attestation.kind == "pending":
            out.append({"type": "pending", "uri": attestation.uri})
        elif attestation.kind in ("bitcoin", "litecoin", "ethereum"):
            out.append({"type": attestation.kind, "height": attestation.height})
        else:
            out.append({"type": "unknown", "tag": attestation.tag.hex()})
    return out


# ------------------------------- Bitcoin block headers -------------------------------
def double_sha256(data: bytes) -> bytes:
    return hashlib.sha256(hashlib.sha256(data).digest()).digest()


@dataclass(frozen=True)
class BlockHeader:
    raw: bytes

    def __post_init__(self) -> None:
        if len(self.raw) != 80:
            raise ValueError("a Bitcoin block header is exactly 80 bytes")

    @property
    def merkle_root(self) -> bytes:
        """Internal byte order, which is the order an OpenTimestamps digest uses."""
        return self.raw[36:68]

    @property
    def time(self) -> int:
        return int.from_bytes(self.raw[68:72], "little")

    @property
    def bits(self) -> int:
        return int.from_bytes(self.raw[72:76], "little")

    def block_hash(self) -> bytes:
        """Display byte order, as explorers print it."""
        return double_sha256(self.raw)[::-1]

    def meets_own_target(self) -> bool:
        exponent, mantissa = self.bits >> 24, self.bits & 0x007FFFFF
        if self.bits & 0x00800000 or mantissa == 0 or exponent > 32:
            return False
        target = mantissa >> (8 * (3 - exponent)) if exponent <= 3 else mantissa << (8 * (exponent - 3))
        return int.from_bytes(self.block_hash(), "big") <= target


class HeaderSource(Protocol):
    kind: str

    def header_at(self, height: int) -> BlockHeader: ...


class LocalHeaderSource:
    """Headers the caller supplies from their own node (`bitcoin-cli getblockheader
    <hash> false`). What is checked: the attested digest equals the header's merkle
    root and the header meets its own nBits target. Not checked by this tool: that
    the header is on the best chain at that height."""

    kind = "local_header"

    def __init__(self, headers: dict[int, bytes]) -> None:
        self.headers = {height: BlockHeader(raw) for height, raw in headers.items()}

    def header_at(self, height: int) -> BlockHeader:
        if height not in self.headers:
            raise LookupError(f"no caller-supplied header for height {height}")
        return self.headers[height]


class ExplorerHeaderSource:
    """An Esplora-compatible explorer (GET /block-height/<h>, GET /block/<hash>/header)."""

    kind = "explorer"

    def __init__(self, base_url: str, timeout: float = 10.0, context=None) -> None:
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self.context = context

    def _get(self, path: str, max_bytes: int) -> str:
        response = http_fetch(
            self.base_url + path, timeout=self.timeout, max_bytes=max_bytes,
            headers={"User-Agent": USER_AGENT}, context=self.context,
        )
        if response.status != 200 or len(response.body) > max_bytes:
            raise LookupError(f"the explorer answered {response.status_line!r} for {path}")
        return response.body.decode("ascii", "replace").strip()

    def header_at(self, height: int) -> BlockHeader:
        block_hash = self._get(f"/block-height/{int(height)}", 64)
        header_hex = self._get(f"/block/{block_hash}/header", 160)
        if len(block_hash) != 64 or len(header_hex) != 160:
            raise LookupError("the explorer returned a malformed hash or header")
        header = BlockHeader(bytes.fromhex(header_hex))
        if header.block_hash().hex() != block_hash.lower():
            raise LookupError("the explorer's header does not hash to the block it named")
        return header


@dataclass(frozen=True)
class BitcoinCheck:
    height: int
    status: str  # verified | mismatch | unavailable
    block_time: int | None = None
    source: str = ""
    reason: str = ""


def check_bitcoin_attestations(stamp: Timestamp, source: HeaderSource) -> list[BitcoinCheck]:
    """Compare each Bitcoin attestation's digest to the merkle root of its block."""
    checks: list[BitcoinCheck] = []
    for msg, attestation in stamp.walk():
        if attestation.kind != "bitcoin":
            continue
        try:
            header = source.header_at(attestation.height)
        except (LookupError, OSError, ValueError) as error:
            checks.append(BitcoinCheck(attestation.height, "unavailable", None, source.kind, str(error)))
            continue
        if len(msg) != 32 or msg != header.merkle_root:
            checks.append(BitcoinCheck(attestation.height, "mismatch", None, source.kind,
                                       "the attested digest is not the block's merkle root"))
        elif not header.meets_own_target():
            checks.append(BitcoinCheck(attestation.height, "mismatch", None, source.kind,
                                       "the header does not meet its own proof-of-work target"))
        else:
            checks.append(BitcoinCheck(attestation.height, "verified", header.time, source.kind))
    return checks


# ------------------------------------ calendars ------------------------------------
def calendar_submit(url: str, digest: bytes, *, timeout: float = CALENDAR_TIMEOUT_SECONDS, context=None) -> Timestamp:
    response = http_fetch(
        url.rstrip("/") + "/digest", method="POST", data=digest, headers=CALENDAR_HEADERS,
        timeout=timeout, max_bytes=MAX_CALENDAR_RESPONSE_BYTES, context=context,
    )
    if response.status != 200:
        raise ValueError(f"the calendar answered {response.status_line!r}")
    if len(response.body) > MAX_CALENDAR_RESPONSE_BYTES:
        raise ValueError("calendar response exceeded the size bound")
    return parse_timestamp(response.body, digest)


def calendar_get(url: str, commitment: bytes, *, timeout: float = CALENDAR_TIMEOUT_SECONDS, context=None) -> Timestamp | None:
    """The calendar's timestamp for *commitment*, or None while it has none yet."""
    response = http_fetch(
        url.rstrip("/") + "/timestamp/" + commitment.hex(), headers=CALENDAR_HEADERS,
        timeout=timeout, max_bytes=MAX_CALENDAR_RESPONSE_BYTES, context=context,
    )
    if response.status == 404:
        return None
    if response.status != 200:
        raise ValueError(f"the calendar answered {response.status_line!r}")
    if len(response.body) > MAX_CALENDAR_RESPONSE_BYTES:
        raise ValueError("calendar response exceeded the size bound")
    return parse_timestamp(response.body, commitment)


def upgrade(proof: DetachedTimestampFile, allowed_calendars: set[str], *, timeout: float = CALENDAR_TIMEOUT_SECONDS,
            context=None) -> list[dict[str, object]]:
    """Merge each calendar's completed timestamp into a pending proof, in place.

    Only calendars in *allowed_calendars* are contacted: a proof is untrusted input
    and its pending URIs must not choose where this process connects. Returns one
    outcome per pending attestation: upgraded | not_ready | skipped | failed.
    """
    outcomes: list[dict[str, object]] = []
    nodes = [(msg, att) for msg, att in proof.timestamp.walk() if att.kind == "pending"]
    allowed = {url.rstrip("/") for url in allowed_calendars}
    for msg, attestation in nodes:
        uri = attestation.uri.rstrip("/")
        if uri not in allowed:
            outcomes.append({"uri": attestation.uri, "status": "skipped", "reason": "calendar not in the allowed set"})
            continue
        try:
            fresh = calendar_get(uri, msg, timeout=timeout, context=context)
        except (OSError, ValueError) as error:
            outcomes.append({"uri": attestation.uri, "status": "failed", "reason": str(error)})
            continue
        if fresh is None:
            outcomes.append({"uri": attestation.uri, "status": "not_ready", "reason": "the calendar has no attestation yet"})
            continue
        _node_for(proof.timestamp, msg).merge(fresh)
        outcomes.append({"uri": attestation.uri, "status": "upgraded"})
    return outcomes


def _node_for(root: Timestamp, msg: bytes) -> Timestamp:
    if root.msg == msg:
        return root
    for _key, (_op, child) in root.ops.items():
        try:
            return _node_for(child, msg)
        except KeyError:
            continue
    raise KeyError(msg)


# --------------------------------- anchoring service ---------------------------------
_PENDING_SCOPE = (
    "Each calendar server acknowledged the commitment and promised to include it in a "
    "Bitcoin transaction. No Bitcoin attestation exists yet, so this record asserts no "
    "external time. Upgrade the proof later with `python -m daemon.time_anchor.ots "
    "upgrade`; a completed proof shows only that the data existed no later than the "
    "attesting block's miner-set time."
)
_UNAVAILABLE_SCOPE = "No calendar accepted the commitment; no external time is asserted."


def unavailable_record(data_hash: str, reason: str, calendars: list[dict[str, object]]) -> dict[str, object]:
    return {
        "status": "unavailable",
        "data_hash": data_hash,
        "reason": reason,
        "calendars": calendars,
        "scope": _UNAVAILABLE_SCOPE,
        "apw:proof_level": "unknown_unobserved",
    }


class OtsAnchorService:
    """Submits a hash to OpenTimestamps calendars and renders the manifest record."""

    def __init__(self, calendars: tuple[str, ...] | list[str] | None = None, *, timeout: float = CALENDAR_TIMEOUT_SECONDS,
                 context=None, submit: Callable[..., Timestamp] | None = None) -> None:
        self.calendars = tuple(calendars) if calendars else DEFAULT_CALENDARS
        self.timeout = timeout
        self.context = context
        self._submit = submit or calendar_submit

    def anchor_record(self, data_hash: str) -> dict[str, object]:
        try:
            try:
                digest = bytes.fromhex(data_hash)
            except ValueError:
                digest = b""
            if len(digest) != 32:
                raise ValueError("data hash must be a 64-character SHA-256 hex digest")
            file_stamp = Timestamp(digest)
            # REQUIRED: the calendars see only sha256(digest || nonce), never the file
            # digest, and the nonce keeps the commitment unpredictable. Same design as
            # the reference client (otsclient/cmds.py).
            commitment_stamp = file_stamp.add_op((OP_APPEND, secrets.token_bytes(16))).add_op((OP_SHA256, None))
            commitment = commitment_stamp.msg
        except (ValueError, DeserializationError) as error:
            return unavailable_record(data_hash, str(error), [])

        def one(url: str) -> tuple[str, Timestamp | None, str]:
            try:
                return url, self._submit(url, commitment, timeout=self.timeout, context=self.context), ""
            except Exception as error:  # noqa: BLE001 - any failure degrades to a per-calendar reason
                return url, None, str(error)

        with concurrent.futures.ThreadPoolExecutor(max_workers=len(self.calendars)) as pool:
            results = list(pool.map(one, self.calendars))
        outcomes: list[dict[str, object]] = []
        for url, stamp, reason in results:
            if stamp is None:
                log.warning("OpenTimestamps calendar %s failed: %s", url, reason)
                outcomes.append({"url": url, "status": "failed", "reason": reason})
            else:
                commitment_stamp.merge(stamp)
                outcomes.append({"url": url, "status": "submitted"})
        if not commitment_stamp.attestations and not commitment_stamp.ops:
            return unavailable_record(data_hash, "no calendar returned a usable timestamp", outcomes)
        try:
            proof = DetachedTimestampFile(OP_SHA256, file_stamp)
            proof_bytes = proof.serialize()
        except ValueError as error:
            return unavailable_record(data_hash, str(error), outcomes)
        return {
            "status": "pending",
            "data_hash": data_hash,
            "file_hash_op": "sha256",
            "commitment_hex": commitment.hex(),
            "calendars": outcomes,
            "attestations": attestation_summary(file_stamp),
            "proof_hex": proof_bytes.hex(),
            "scope": _PENDING_SCOPE,
            # IMPORTANT: a calendar receipt asserts no time, so this is never above
            # unknown_unobserved, and even a completed proof is capped at inferred.
            "apw:proof_level": "unknown_unobserved",
        }


# ------------------------------- manifest record check -------------------------------
Finding = tuple  # (severity, code, message)


def _block_time_text(seconds: int) -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(seconds))


def evaluate_record(
    data: dict[str, object],
    header_source: HeaderSource | None = None,
    proof_override: bytes | None = None,
) -> list[Finding]:
    """Findings for ``data["time_anchor_opentimestamps"]``.

    *proof_override* is an upgraded ``.ots`` sidecar for the same record: it must be
    for the same export digest and contain the recorded commitment. The signed
    manifest is never rewritten by an upgrade.
    """
    record = data.get("time_anchor_opentimestamps")
    if record is None:
        return []
    if not isinstance(record, dict):
        return [("error", "time_anchor_ots_invalid", "time_anchor_opentimestamps must be an object")]
    status = record.get("status")
    if status == "unavailable":
        return [("info", "time_anchor_ots_unavailable",
                 "No OpenTimestamps calendar accepted the commitment; no external time is asserted")]
    if status != "pending":
        return [("error", "time_anchor_ots_invalid", "time_anchor_opentimestamps status must be pending or unavailable")]
    level = record.get("apw:proof_level")
    if not isinstance(level, str) or level not in PROOF_LEVELS:
        return [("error", "time_anchor_ots_invalid",
                 "time_anchor_opentimestamps claims stronger evidence than a calendar promise")]
    export = data.get("export")
    export_hash = export.get("sha256") if isinstance(export, dict) else None
    if record.get("data_hash") != export_hash:
        return [("error", "time_anchor_ots_invalid", "time_anchor_opentimestamps data_hash does not match the export hash")]
    proof_hex, commitment_hex = record.get("proof_hex"), record.get("commitment_hex")
    if not isinstance(proof_hex, str) or not isinstance(commitment_hex, str) or record.get("file_hash_op") != "sha256":
        return [("error", "time_anchor_ots_invalid", "time_anchor_opentimestamps is missing its proof fields")]
    try:
        proof_bytes, commitment = bytes.fromhex(proof_hex), bytes.fromhex(commitment_hex)
    except ValueError:
        return [("error", "time_anchor_ots_invalid", "time_anchor_opentimestamps proof_hex and commitment_hex must be hexadecimal")]
    try:
        recorded = parse_detached(proof_bytes)
    except DeserializationError as error:
        return [("error", "time_anchor_ots_invalid", f"time_anchor_opentimestamps proof does not parse: {error}")]
    if recorded.file_hash_op != OP_SHA256 or recorded.file_digest.hex() != export_hash:
        return [("error", "time_anchor_ots_invalid", "the proof commits to a different file digest than the export hash")]
    if commitment not in set(recorded.timestamp.messages()):
        return [("error", "time_anchor_ots_invalid", "the recorded commitment is not part of the proof")]
    if record.get("attestations") != attestation_summary(recorded.timestamp):
        return [("error", "time_anchor_ots_invalid", "the recorded attestations do not match the proof")]

    proof = recorded
    if proof_override is not None:
        try:
            proof = parse_detached(proof_override)
        except DeserializationError as error:
            return [("error", "time_anchor_ots_invalid", f"the supplied proof does not parse: {error}")]
        if proof.file_hash_op != OP_SHA256 or proof.file_digest.hex() != export_hash:
            return [("error", "time_anchor_ots_invalid", "the supplied proof is for a different file digest")]
        if commitment not in set(proof.timestamp.messages()):
            return [("error", "time_anchor_ots_invalid", "the supplied proof does not contain the recorded commitment")]

    leaves = list(proof.timestamp.walk())
    bitcoin = [(msg, att) for msg, att in leaves if att.kind == "bitcoin"]
    if not bitcoin:
        uris = sorted({att.uri for _msg, att in leaves if att.kind == "pending"})
        return [("info", "time_anchor_ots_pending",
                 f"OpenTimestamps proof is pending at {len(uris)} calendar(s); no Bitcoin attestation exists "
                 "yet, so no external time is asserted")]
    heights = sorted({att.height for _msg, att in bitcoin})
    if header_source is None:
        return [("info", "time_anchor_ots_bitcoin_unchecked",
                 f"The proof carries Bitcoin attestation(s) at height(s) {heights}; block headers were not "
                 "consulted in this verification, so no time is asserted")]
    findings: list[Finding] = []
    verified: list[BitcoinCheck] = []
    for check in check_bitcoin_attestations(proof.timestamp, header_source):
        if check.status == "verified":
            verified.append(check)
        elif check.status == "mismatch":
            findings.append(("error", "time_anchor_ots_invalid",
                             f"Bitcoin attestation at height {check.height} failed: {check.reason}"))
        else:
            findings.append(("warning", "time_anchor_ots_header_unavailable",
                             f"Bitcoin attestation at height {check.height} could not be checked: {check.reason}"))
    if verified:
        earliest = min(check.block_time for check in verified)
        if header_source.kind == "local_header":
            checked = ("the attested digest equals the merkle root of a header you supplied from your own node, and that "
                       "header meets its own proof-of-work target; that the header is on the best chain at that height "
                       "was not checked here")
        else:
            checked = ("the attested digest equals the merkle root of the header the configured explorer returned; the "
                       "explorer is trusted to serve the best chain, and this is not an independent check")
        findings.append(("info", "time_anchor_ots_block_verified",
                         f"The data existed no later than {_block_time_text(earliest)} (block timestamp, set by the "
                         f"miner and only roughly accurate): {checked}"))
    return findings


# ------------------------------------------ CLI ------------------------------------------
def _parse_headers(items: list[str]) -> dict[int, bytes]:
    headers: dict[int, bytes] = {}
    for item in items:
        height, _, raw = item.partition(":")
        headers[int(height)] = bytes.fromhex(raw)
    return headers


def main(argv: list[str] | None = None) -> int:
    import argparse
    import json
    from pathlib import Path

    parser = argparse.ArgumentParser(description="Upgrade or check an OpenTimestamps detached proof.")
    sub = parser.add_subparsers(dest="command", required=True)
    up = sub.add_parser("upgrade", help="Ask the proof's calendars for completed attestations.")
    up.add_argument("proof", type=Path)
    up.add_argument("--calendar", action="append", default=[], help="An extra calendar allowed to be contacted.")
    up.add_argument("--out", type=Path, default=None, help="Where to write the upgraded proof (default: PROOF.upgraded).")
    check = sub.add_parser("verify", help="Check a proof's Bitcoin attestations against block headers.")
    check.add_argument("proof", type=Path)
    what = check.add_mutually_exclusive_group(required=True)
    what.add_argument("--file", type=Path)
    what.add_argument("--digest", help="SHA-256 of the timestamped file, hex.")
    where = check.add_mutually_exclusive_group()
    where.add_argument("--explorer", metavar="URL", help="Esplora-compatible explorer (trusted to serve the best chain).")
    where.add_argument("--header", action="append", default=[], metavar="HEIGHT:HEX",
                       help="An 80-byte header from your own node (repeatable).")
    args = parser.parse_args(argv)
    try:
        proof = parse_detached(args.proof.read_bytes())
    except (OSError, DeserializationError) as error:
        print(f"cannot read the proof: {error}")
        return 2
    if args.command == "upgrade":
        outcomes = upgrade(proof, set(DEFAULT_CALENDARS) | set(args.calendar))
        out = args.out or args.proof.with_suffix(args.proof.suffix + ".upgraded")
        out.write_bytes(proof.serialize())
        print(json.dumps({"out": str(out), "outcomes": outcomes, "attestations": attestation_summary(proof.timestamp)}, indent=2))
        return 0
    digest = hashlib.sha256(args.file.read_bytes()).hexdigest() if args.file else args.digest
    if proof.file_hash_op != OP_SHA256 or proof.file_digest.hex() != digest:
        print("the proof is for a different file digest")
        return 1
    source: HeaderSource | None = None
    if args.header:
        source = LocalHeaderSource(_parse_headers(args.header))
    elif args.explorer:
        source = ExplorerHeaderSource(args.explorer)
    if source is None:
        print(json.dumps({"attestations": attestation_summary(proof.timestamp), "checked": False}, indent=2))
        return 0
    checks = check_bitcoin_attestations(proof.timestamp, source)
    print(json.dumps([check.__dict__ for check in checks], indent=2))
    return 0 if checks and all(check.status == "verified" for check in checks) else 1


if __name__ == "__main__":
    raise SystemExit(main())
