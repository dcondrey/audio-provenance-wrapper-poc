//! OpenTimestamps: the detached-proof codec, Bitcoin header verification and the
//! verifier's check of the `time_anchor_opentimestamps` record.
//!
//! IMPORTANT: a port of `daemon/time_anchor/ots.py`, which follows the published
//! python-opentimestamps source. Error strings are the Python ones because they
//! land in findings and in the record's `reason`.
//!
//! Deliberate differences from upstream, shared with the Python port: KECCAK-256
//! is rejected as unsupported, and LEB128 integers are bounded to 63 bits.
//!
//! HONESTY: a calendar receipt promises a later Bitcoin attestation and asserts no
//! time. A completed proof's time is the miner-set timestamp of the attesting
//! block: an upper bound ("existed no later than"), never an earlier time.

use core::cmp::Ordering;

use ripemd::Ripemd160;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::proof::PROOF_LEVEL_KEY;
use crate::pyvalue::python_eq;
use crate::time_anchor::{iso_from_ms, python_from_hex};

pub const HEADER_MAGIC: [u8; 31] = [
    0x00, b'O', b'p', b'e', b'n', b'T', b'i', b'm', b'e', b's', b't', b'a', b'm', b'p', b's', 0x00,
    0x00, b'P', b'r', b'o', b'o', b'f', 0x00, 0xbf, 0x89, 0xe2, 0xe8, 0x84, 0xe8, 0x92, 0x94,
];
const MAJOR_VERSION: u8 = 1;
const MAX_MSG_LENGTH: usize = 4096;
const MAX_RESULT_LENGTH: usize = 4096;
const MAX_HEXLIFY_MSG_LENGTH: usize = MAX_RESULT_LENGTH / 2;
const MAX_ATTESTATION_PAYLOAD: usize = 8192;
const MAX_URI_LENGTH: usize = 1000;
const RECURSION_LIMIT: usize = 256;

pub const TAG_PENDING: [u8; 8] = [0x83, 0xdf, 0xe3, 0x0d, 0x2e, 0xf9, 0x0c, 0x8e];
pub const TAG_BITCOIN: [u8; 8] = [0x05, 0x88, 0x96, 0x0d, 0x73, 0xd7, 0x19, 0x01];
pub const TAG_LITECOIN: [u8; 8] = [0x06, 0x86, 0x9a, 0x0d, 0x73, 0xd7, 0x1b, 0x45];
pub const TAG_ETHEREUM: [u8; 8] = [0x30, 0xfe, 0x80, 0x87, 0xb5, 0xc7, 0xea, 0xd7];

pub const OP_SHA1: u8 = 0x02;
pub const OP_RIPEMD160: u8 = 0x03;
pub const OP_SHA256: u8 = 0x08;
pub const OP_KECCAK256: u8 = 0x67;
pub const OP_APPEND: u8 = 0xF0;
pub const OP_PREPEND: u8 = 0xF1;
pub const OP_REVERSE: u8 = 0xF2;
pub const OP_HEXLIFY: u8 = 0xF3;

pub const DEFAULT_CALENDARS: [&str; 3] = [
    "https://alice.btc.calendar.opentimestamps.org",
    "https://bob.btc.calendar.opentimestamps.org",
    "https://finney.calendar.eternitywall.com",
];
pub const CALENDAR_TIMEOUT_SECONDS: u64 = 10;
pub const MAX_CALENDAR_RESPONSE_BYTES: usize = 10_000;

const PROOF_LEVELS: [&str; 2] = ["inferred", "unknown_unobserved"];

const PENDING_SCOPE: &str = "Each calendar server acknowledged the commitment and promised to include it in a \
Bitcoin transaction. No Bitcoin attestation exists yet, so this record asserts no \
external time. Upgrade the proof later with `python -m daemon.time_anchor.ots \
upgrade`; a completed proof shows only that the data existed no later than the \
attesting block's miner-set time.";
const UNAVAILABLE_SCOPE: &str = "No calendar accepted the commitment; no external time is asserted.";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct OtsError(pub String);

fn fail<T>(message: impl Into<String>) -> Result<T, OtsError> {
    Err(OtsError(message.into()))
}

fn op_name(tag: u8) -> &'static str {
    match tag {
        OP_SHA1 => "sha1",
        OP_RIPEMD160 => "ripemd160",
        OP_SHA256 => "sha256",
        OP_KECCAK256 => "keccak256",
        OP_APPEND => "append",
        OP_PREPEND => "prepend",
        OP_REVERSE => "reverse",
        _ => "hexlify",
    }
}

fn hash_length(tag: u8) -> Option<usize> {
    match tag {
        OP_SHA1 | OP_RIPEMD160 => Some(20),
        OP_SHA256 => Some(32),
        _ => None,
    }
}

// --------------------------------- reader ----------------------------------------

struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, offset: 0 }
    }

    fn bytes(&mut self, count: usize) -> Result<&'a [u8], OtsError> {
        let available = self.data.len().saturating_sub(self.offset);
        let end = self.offset.checked_add(count).filter(|end| *end <= self.data.len());
        let Some(end) = end else {
            return fail(format!("Tried to read {count} bytes but got only {available} bytes"));
        };
        let chunk = self.data.get(self.offset..end).unwrap_or_default();
        self.offset = end;
        Ok(chunk)
    }

    fn uint8(&mut self) -> Result<u8, OtsError> {
        Ok(self.bytes(1)?.first().copied().unwrap_or(0))
    }

    /// LEB128, bounded to 63 bits.
    fn varuint(&mut self) -> Result<u64, OtsError> {
        let mut value: u64 = 0;
        let mut shift: u32 = 0;
        loop {
            let byte = self.uint8()?;
            let payload = u128::from(byte & 0x7F);
            if payload != 0 {
                if shift >= 63 || (payload << shift) >> 63 != 0 {
                    return fail("varuint exceeds 63 bits");
                }
                value |= (payload << shift) as u64;
            }
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift = shift.saturating_add(7);
        }
    }

    fn varbytes(&mut self, max_length: usize, min_length: usize) -> Result<&'a [u8], OtsError> {
        let length = self.varuint()?;
        let length = usize::try_from(length).unwrap_or(usize::MAX);
        if length > max_length {
            return fail(format!("varbytes max length exceeded; {length} > {max_length}"));
        }
        if length < min_length {
            return fail(format!("varbytes min length not met; {length} < {min_length}"));
        }
        self.bytes(length)
    }

    fn assert_eof(&self) -> Result<(), OtsError> {
        if self.offset != self.data.len() {
            return fail("Trailing garbage found after end of deserialized data");
        }
        Ok(())
    }
}

fn write_varuint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            out.push(byte | 0x80);
        } else {
            out.push(byte);
            return;
        }
    }
}

fn write_varbytes(out: &mut Vec<u8>, data: &[u8]) {
    write_varuint(out, data.len() as u64);
    out.extend_from_slice(data);
}

// -------------------------------- operations -------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    pub tag: u8,
    pub arg: Option<Vec<u8>>,
}

impl Op {
    pub fn append(arg: Vec<u8>) -> Op {
        Op { tag: OP_APPEND, arg: Some(arg) }
    }

    pub fn prepend(arg: Vec<u8>) -> Op {
        Op { tag: OP_PREPEND, arg: Some(arg) }
    }

    pub fn sha256() -> Op {
        Op { tag: OP_SHA256, arg: None }
    }

    fn key(&self) -> (u8, &[u8]) {
        (self.tag, self.arg.as_deref().unwrap_or_default())
    }

    fn apply(&self, msg: &[u8]) -> Result<Vec<u8>, OtsError> {
        let limit = if self.tag == OP_HEXLIFY { MAX_HEXLIFY_MSG_LENGTH } else { MAX_MSG_LENGTH };
        if msg.len() > limit {
            return fail(format!("Message too long for {}; {}", op_name(self.tag), msg.len()));
        }
        let arg = self.arg.as_deref().unwrap_or_default();
        let result: Vec<u8> = match self.tag {
            OP_APPEND => [msg, arg].concat(),
            OP_PREPEND => [arg, msg].concat(),
            OP_REVERSE => {
                if msg.is_empty() {
                    return fail("Can't reverse an empty message");
                }
                msg.iter().rev().copied().collect()
            }
            OP_HEXLIFY => {
                if msg.is_empty() {
                    return fail("Can't hexlify an empty message");
                }
                msg.iter().map(|byte| format!("{byte:02x}")).collect::<String>().into_bytes()
            }
            OP_SHA1 => {
                use sha1::Digest as _;
                sha1::Sha1::digest(msg).to_vec()
            }
            OP_RIPEMD160 => {
                use ripemd::Digest as _;
                Ripemd160::digest(msg).to_vec()
            }
            OP_SHA256 => Sha256::digest(msg).to_vec(),
            other => return fail(format!("Unknown operation tag 0x{other:02x}")),
        };
        if result.len() > MAX_RESULT_LENGTH {
            return fail(format!("Result too long; {} > {MAX_RESULT_LENGTH}", result.len()));
        }
        Ok(result)
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.push(self.tag);
        if let Some(arg) = &self.arg {
            write_varbytes(out, arg);
        }
    }
}

fn read_op(reader: &mut Reader, tag: u8) -> Result<Op, OtsError> {
    match tag {
        OP_APPEND | OP_PREPEND => {
            Ok(Op { tag, arg: Some(reader.varbytes(MAX_RESULT_LENGTH, 1)?.to_vec()) })
        }
        OP_REVERSE | OP_HEXLIFY | OP_SHA1 | OP_RIPEMD160 | OP_SHA256 => Ok(Op { tag, arg: None }),
        OP_KECCAK256 => fail("keccak256 is not supported"),
        other => fail(format!("Unknown operation tag 0x{other:02x}")),
    }
}

// ------------------------------- attestations ------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttestationKind {
    Pending,
    Bitcoin,
    Litecoin,
    Ethereum,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    pub kind: AttestationKind,
    pub tag: [u8; 8],
    pub uri: String,
    pub height: u64,
    pub payload: Vec<u8>,
}

impl Attestation {
    pub fn pending(uri: &str) -> Result<Attestation, OtsError> {
        if uri.len() > MAX_URI_LENGTH || !uri.bytes().all(uri_char_allowed) {
            return fail("calendar URI is too long or uses characters an OpenTimestamps proof cannot carry");
        }
        Ok(Attestation {
            kind: AttestationKind::Pending,
            tag: TAG_PENDING,
            uri: uri.to_owned(),
            height: 0,
            payload: Vec::new(),
        })
    }

    pub fn bitcoin(height: u64) -> Attestation {
        Attestation {
            kind: AttestationKind::Bitcoin,
            tag: TAG_BITCOIN,
            uri: String::new(),
            height,
            payload: Vec::new(),
        }
    }

    fn compare(&self, other: &Attestation) -> Ordering {
        self.tag.cmp(&other.tag).then_with(|| match self.kind {
            AttestationKind::Pending => self.uri.as_bytes().cmp(other.uri.as_bytes()),
            AttestationKind::Unknown => self.payload.cmp(&other.payload),
            _ => self.height.cmp(&other.height),
        })
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.tag);
        let mut payload = Vec::new();
        match self.kind {
            AttestationKind::Pending => write_varbytes(&mut payload, self.uri.as_bytes()),
            AttestationKind::Unknown => payload.extend_from_slice(&self.payload),
            _ => write_varuint(&mut payload, self.height),
        }
        write_varbytes(out, &payload);
    }
}

fn uri_char_allowed(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'/' | b':')
}

fn read_attestation(reader: &mut Reader) -> Result<Attestation, OtsError> {
    let tag: [u8; 8] = reader.bytes(8)?.try_into().map_err(|_| OtsError("bad tag".to_owned()))?;
    let payload = reader.varbytes(MAX_ATTESTATION_PAYLOAD, 0)?;
    let mut inner = Reader::new(payload);
    if tag == TAG_PENDING {
        let uri = inner.varbytes(MAX_URI_LENGTH, 0)?;
        if !uri.iter().copied().all(uri_char_allowed) {
            return fail("Invalid URI: contains a disallowed character");
        }
        inner.assert_eof()?;
        return Ok(Attestation {
            kind: AttestationKind::Pending,
            tag,
            uri: String::from_utf8_lossy(uri).into_owned(),
            height: 0,
            payload: Vec::new(),
        });
    }
    for (kind, known) in [
        (AttestationKind::Bitcoin, TAG_BITCOIN),
        (AttestationKind::Litecoin, TAG_LITECOIN),
        (AttestationKind::Ethereum, TAG_ETHEREUM),
    ] {
        if tag == known {
            let height = inner.varuint()?;
            inner.assert_eof()?;
            return Ok(Attestation { kind, tag, uri: String::new(), height, payload: Vec::new() });
        }
    }
    Ok(Attestation {
        kind: AttestationKind::Unknown,
        tag,
        uri: String::new(),
        height: 0,
        payload: payload.to_vec(),
    })
}

// -------------------------------- timestamp --------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timestamp {
    pub msg: Vec<u8>,
    attestations: Vec<Attestation>,
    /// Insertion order, like the Python dict; a repeated op replaces in place.
    ops: Vec<(Op, Timestamp)>,
}

impl Timestamp {
    pub fn new(msg: Vec<u8>) -> Timestamp {
        Timestamp { msg, attestations: Vec::new(), ops: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.attestations.is_empty() && self.ops.is_empty()
    }

    pub fn add_attestation(&mut self, attestation: Attestation) {
        if !self.attestations.contains(&attestation) {
            self.attestations.push(attestation);
        }
    }

    /// The child for `op`, created (and computed) on first use.
    pub fn add_op(&mut self, op: Op) -> Result<&mut Timestamp, OtsError> {
        let position = self.ops.iter().position(|(existing, _)| existing.key() == op.key());
        let index = match position {
            Some(index) => index,
            None => {
                let result = op.apply(&self.msg)?;
                self.ops.push((op, Timestamp::new(result)));
                self.ops.len() - 1
            }
        };
        self.ops
            .get_mut(index)
            .map(|(_, child)| child)
            .ok_or_else(|| OtsError("internal error: missing timestamp child".to_owned()))
    }

    pub fn merge(&mut self, other: &Timestamp) -> Result<(), OtsError> {
        if self.msg != other.msg {
            return fail("Can't merge timestamps for different messages together");
        }
        for attestation in &other.attestations {
            self.add_attestation(attestation.clone());
        }
        for (op, child) in &other.ops {
            self.add_op(op.clone())?.merge(child)?;
        }
        Ok(())
    }

    fn sorted_attestations(&self) -> Vec<&Attestation> {
        let mut sorted: Vec<&Attestation> = self.attestations.iter().collect();
        sorted.sort_by(|a, b| a.compare(b));
        sorted
    }

    fn sorted_ops(&self) -> Vec<&(Op, Timestamp)> {
        let mut sorted: Vec<&(Op, Timestamp)> = self.ops.iter().collect();
        sorted.sort_by(|a, b| a.0.key().cmp(&b.0.key()));
        sorted
    }

    pub fn serialize(&self, out: &mut Vec<u8>) -> Result<(), OtsError> {
        if self.is_empty() {
            return fail("An empty timestamp can't be serialized");
        }
        let attestations = self.sorted_attestations();
        let ops = self.sorted_ops();
        let split_last = attestations.len().saturating_sub(1);
        for attestation in attestations.iter().take(split_last) {
            out.extend_from_slice(&[0xFF, 0x00]);
            attestation.write(out);
        }
        if ops.is_empty() {
            if let Some(last) = attestations.last() {
                out.push(0x00);
                last.write(out);
            }
        } else {
            if let Some(last) = attestations.last() {
                out.extend_from_slice(&[0xFF, 0x00]);
                last.write(out);
            }
            let split = ops.len() - 1;
            for (op, child) in ops.iter().take(split) {
                out.push(0xFF);
                op.write(out);
                child.serialize(out)?;
            }
            if let Some((op, child)) = ops.last().copied() {
                op.write(out);
                child.serialize(out)?;
            }
        }
        Ok(())
    }

    /// Every `(msg, attestation)` leaf, depth first, in serialization order.
    pub fn walk(&self) -> Vec<(&[u8], &Attestation)> {
        let mut out = Vec::new();
        self.walk_into(&mut out);
        out
    }

    fn walk_into<'a>(&'a self, out: &mut Vec<(&'a [u8], &'a Attestation)>) {
        for attestation in self.sorted_attestations() {
            out.push((self.msg.as_slice(), attestation));
        }
        for (_, child) in self.sorted_ops() {
            child.walk_into(out);
        }
    }

    pub fn messages(&self) -> Vec<&[u8]> {
        let mut out = vec![self.msg.as_slice()];
        for (_, child) in &self.ops {
            out.extend(child.messages());
        }
        out
    }

    /// The first node (insertion order, depth first) whose message is `msg`.
    pub fn node_for(&mut self, msg: &[u8]) -> Option<&mut Timestamp> {
        if self.msg == msg {
            return Some(self);
        }
        for (_, child) in &mut self.ops {
            if let Some(found) = child.node_for(msg) {
                return Some(found);
            }
        }
        None
    }
}

fn read_timestamp(reader: &mut Reader, msg: &[u8], limit: usize) -> Result<Timestamp, OtsError> {
    if limit == 0 {
        return fail("Reached timestamp recursion depth limit while deserializing");
    }
    if msg.len() > MAX_MSG_LENGTH {
        return fail(format!("Message exceeds Op length limit; {} > {MAX_MSG_LENGTH}", msg.len()));
    }
    let mut stamp = Timestamp::new(msg.to_vec());
    let mut tag = reader.uint8()?;
    loop {
        let current = if tag == 0xFF { reader.uint8()? } else { tag };
        if current == 0x00 {
            stamp.add_attestation(read_attestation(reader)?);
        } else {
            let op = read_op(reader, current)?;
            let result = op.apply(msg)?;
            let child = read_timestamp(reader, &result, limit - 1)?;
            match stamp.ops.iter_mut().find(|(existing, _)| existing.key() == op.key()) {
                Some(slot) => slot.1 = child,
                None => stamp.ops.push((op, child)),
            }
        }
        if tag != 0xFF {
            return Ok(stamp);
        }
        tag = reader.uint8()?;
    }
}

/// A bare timestamp (a calendar reply) for the known message `msg`.
pub fn parse_timestamp(data: &[u8], msg: &[u8]) -> Result<Timestamp, OtsError> {
    let mut reader = Reader::new(data);
    let stamp = read_timestamp(&mut reader, msg, RECURSION_LIMIT)?;
    reader.assert_eof()?;
    Ok(stamp)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachedProof {
    pub file_hash_op: u8,
    pub timestamp: Timestamp,
}

impl DetachedProof {
    pub fn file_digest(&self) -> &[u8] {
        &self.timestamp.msg
    }

    pub fn serialize(&self) -> Result<Vec<u8>, OtsError> {
        if hash_length(self.file_hash_op) != Some(self.timestamp.msg.len()) {
            return fail("Timestamp message length and file_hash_op digest length differ");
        }
        let mut out = HEADER_MAGIC.to_vec();
        out.extend_from_slice(&[MAJOR_VERSION, self.file_hash_op]);
        out.extend_from_slice(&self.timestamp.msg);
        self.timestamp.serialize(&mut out)?;
        Ok(out)
    }
}

pub fn parse_detached(data: &[u8]) -> Result<DetachedProof, OtsError> {
    if data.get(..HEADER_MAGIC.len()) != Some(&HEADER_MAGIC[..]) {
        return fail("Expected magic bytes for an OpenTimestamps detached proof");
    }
    let mut reader = Reader::new(data);
    reader.bytes(HEADER_MAGIC.len())?;
    let major = reader.uint8()?;
    if major != MAJOR_VERSION {
        return fail(format!("Version {major} detached timestamp files are not supported"));
    }
    let op_tag = reader.uint8()?;
    let Some(length) = hash_length(op_tag) else {
        return fail(format!("Unknown file hash operation tag 0x{op_tag:02x}"));
    };
    let digest = reader.bytes(length)?;
    let timestamp = read_timestamp(&mut reader, digest, RECURSION_LIMIT)?;
    reader.assert_eof()?;
    Ok(DetachedProof { file_hash_op: op_tag, timestamp })
}

pub fn file_hash_op_name(tag: u8) -> &'static str {
    op_name(tag)
}

/// The distinct attestations in serialization order, as the manifest records them.
pub fn attestation_summary(stamp: &Timestamp) -> Vec<Value> {
    let mut seen: Vec<&Attestation> = Vec::new();
    let mut out = Vec::new();
    for (_, attestation) in stamp.walk() {
        if seen.iter().any(|earlier| earlier.compare(attestation) == Ordering::Equal) {
            continue;
        }
        seen.push(attestation);
        out.push(match attestation.kind {
            AttestationKind::Pending => json!({"type": "pending", "uri": attestation.uri}),
            AttestationKind::Bitcoin => json!({"type": "bitcoin", "height": attestation.height}),
            AttestationKind::Litecoin => json!({"type": "litecoin", "height": attestation.height}),
            AttestationKind::Ethereum => json!({"type": "ethereum", "height": attestation.height}),
            AttestationKind::Unknown => json!({"type": "unknown", "tag": hex(&attestation.tag)}),
        });
    }
    out
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// ------------------------------ Bitcoin headers ----------------------------------

pub fn double_sha256(data: &[u8]) -> [u8; 32] {
    let once = Sha256::digest(data);
    let twice = Sha256::digest(once);
    let mut out = [0_u8; 32];
    out.copy_from_slice(&twice);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockHeader {
    raw: [u8; 80],
}

impl BlockHeader {
    pub fn new(raw: &[u8]) -> Result<BlockHeader, OtsError> {
        let raw: [u8; 80] = raw
            .try_into()
            .map_err(|_| OtsError("a Bitcoin block header is exactly 80 bytes".to_owned()))?;
        Ok(BlockHeader { raw })
    }

    /// Internal byte order, which is the order an OpenTimestamps digest uses.
    pub fn merkle_root(&self) -> &[u8] {
        self.raw.get(36..68).unwrap_or_default()
    }

    pub fn time(&self) -> u32 {
        let bytes: [u8; 4] = self.raw.get(68..72).and_then(|b| b.try_into().ok()).unwrap_or_default();
        u32::from_le_bytes(bytes)
    }

    fn bits(&self) -> u32 {
        let bytes: [u8; 4] = self.raw.get(72..76).and_then(|b| b.try_into().ok()).unwrap_or_default();
        u32::from_le_bytes(bytes)
    }

    /// Display byte order, as explorers print it.
    pub fn block_hash(&self) -> [u8; 32] {
        let mut hash = double_sha256(&self.raw);
        hash.reverse();
        hash
    }

    pub fn meets_own_target(&self) -> bool {
        let bits = self.bits();
        let exponent = (bits >> 24) as usize;
        let mantissa = bits & 0x007F_FFFF;
        if bits & 0x0080_0000 != 0 || mantissa == 0 || exponent > 32 {
            return false;
        }
        let mut target = [0_u8; 32];
        if exponent <= 3 {
            let value = mantissa >> (8 * (3 - exponent));
            if let Some(tail) = target.get_mut(28..32) {
                tail.copy_from_slice(&value.to_be_bytes());
            }
        } else {
            let start = 32 - exponent;
            if let Some(window) = target.get_mut(start..start + 3) {
                window.copy_from_slice(mantissa.to_be_bytes().get(1..4).unwrap_or_default());
            }
        }
        self.block_hash() <= target
    }
}

pub trait HeaderSource {
    /// `explorer` or `local_header`; it decides how a verification is described.
    fn kind(&self) -> &str;
    fn header_at(&self, height: u64) -> Result<BlockHeader, String>;
}

/// Headers the caller supplies from their own node. Checked: the attested digest
/// equals the header's merkle root and the header meets its own nBits target. Not
/// checked here: that the header is on the best chain at that height.
pub struct LocalHeaderSource {
    headers: Vec<(u64, BlockHeader)>,
}

impl LocalHeaderSource {
    pub fn new(headers: Vec<(u64, BlockHeader)>) -> Self {
        LocalHeaderSource { headers }
    }
}

impl HeaderSource for LocalHeaderSource {
    fn kind(&self) -> &str {
        "local_header"
    }

    fn header_at(&self, height: u64) -> Result<BlockHeader, String> {
        self.headers
            .iter()
            .find(|(candidate, _)| *candidate == height)
            .map(|(_, header)| header.clone())
            .ok_or_else(|| format!("no caller-supplied header for height {height}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Verified,
    Mismatch,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitcoinCheck {
    pub height: u64,
    pub status: CheckStatus,
    pub block_time: Option<u32>,
    pub source: String,
    pub reason: String,
}

/// Compare each Bitcoin attestation's digest to the merkle root of its block.
pub fn check_bitcoin_attestations(stamp: &Timestamp, source: &dyn HeaderSource) -> Vec<BitcoinCheck> {
    let mut checks = Vec::new();
    for (msg, attestation) in stamp.walk() {
        if attestation.kind != AttestationKind::Bitcoin {
            continue;
        }
        let make = |status, block_time, reason: &str| BitcoinCheck {
            height: attestation.height,
            status,
            block_time,
            source: source.kind().to_owned(),
            reason: reason.to_owned(),
        };
        match source.header_at(attestation.height) {
            Err(reason) => checks.push(make(CheckStatus::Unavailable, None, &reason)),
            Ok(header) => {
                if msg.len() != 32 || msg != header.merkle_root() {
                    checks.push(make(
                        CheckStatus::Mismatch,
                        None,
                        "the attested digest is not the block's merkle root",
                    ));
                } else if !header.meets_own_target() {
                    checks.push(make(
                        CheckStatus::Mismatch,
                        None,
                        "the header does not meet its own proof-of-work target",
                    ));
                } else {
                    checks.push(make(CheckStatus::Verified, Some(header.time()), ""));
                }
            }
        }
    }
    checks
}

// ------------------------------ records and findings -----------------------------

pub fn unavailable_record(data_hash: &str, reason: &str, calendars: Vec<Value>) -> Value {
    json!({
        "status": "unavailable",
        "data_hash": data_hash,
        "reason": reason,
        "calendars": calendars,
        "scope": UNAVAILABLE_SCOPE,
        PROOF_LEVEL_KEY: "unknown_unobserved",
    })
}

pub fn pending_record(
    data_hash: &str,
    commitment: &[u8],
    calendars: Vec<Value>,
    file_stamp: &Timestamp,
    proof: &[u8],
) -> Value {
    json!({
        "status": "pending",
        "data_hash": data_hash,
        "file_hash_op": "sha256",
        "commitment_hex": hex(commitment),
        "calendars": calendars,
        "attestations": attestation_summary(file_stamp),
        "proof_hex": hex(proof),
        "scope": PENDING_SCOPE,
        // IMPORTANT: a calendar receipt asserts no time, so this is never above
        // unknown_unobserved, and even a completed proof is capped at inferred.
        PROOF_LEVEL_KEY: "unknown_unobserved",
    })
}

pub type OtsFinding = (String, String, String);

fn finding(severity: &str, code: &str, message: impl Into<String>) -> OtsFinding {
    (severity.to_owned(), code.to_owned(), message.into())
}

fn invalid(message: impl Into<String>) -> Vec<OtsFinding> {
    vec![finding("error", "time_anchor_ots_invalid", message)]
}

/// `evaluate_record` in `daemon/time_anchor/ots.py`: findings for
/// `data["time_anchor_opentimestamps"]`. `proof_override` is an upgraded `.ots`
/// sidecar for the same record; the signed manifest is never rewritten.
pub fn evaluate_record(
    data: &Value,
    header_source: Option<&dyn HeaderSource>,
    proof_override: Option<&[u8]>,
) -> Vec<OtsFinding> {
    let Some(record) = data.get("time_anchor_opentimestamps") else {
        return Vec::new();
    };
    if record.is_null() {
        return Vec::new();
    }
    let Value::Object(record) = record else {
        return invalid("time_anchor_opentimestamps must be an object");
    };
    let field = |key: &str| record.get(key).unwrap_or(&Value::Null);
    match field("status").as_str() {
        Some("unavailable") => {
            return vec![finding(
                "info",
                "time_anchor_ots_unavailable",
                "No OpenTimestamps calendar accepted the commitment; no external time is asserted",
            )];
        }
        Some("pending") => {}
        _ => {
            return invalid("time_anchor_opentimestamps status must be pending or unavailable");
        }
    }
    if !field(PROOF_LEVEL_KEY).as_str().is_some_and(|level| PROOF_LEVELS.contains(&level)) {
        return invalid("time_anchor_opentimestamps claims stronger evidence than a calendar promise");
    }
    let export_hash = data
        .get("export")
        .and_then(Value::as_object)
        .and_then(|export| export.get("sha256"))
        .unwrap_or(&Value::Null);
    if !python_eq(field("data_hash"), export_hash) {
        return invalid("time_anchor_opentimestamps data_hash does not match the export hash");
    }
    let (Some(proof_hex), Some(commitment_hex), Some("sha256")) = (
        field("proof_hex").as_str(),
        field("commitment_hex").as_str(),
        field("file_hash_op").as_str(),
    ) else {
        return invalid("time_anchor_opentimestamps is missing its proof fields");
    };
    let (Some(proof_bytes), Some(commitment)) = (python_from_hex(proof_hex), python_from_hex(commitment_hex)) else {
        return invalid("time_anchor_opentimestamps proof_hex and commitment_hex must be hexadecimal");
    };
    let recorded = match parse_detached(&proof_bytes) {
        Ok(proof) => proof,
        Err(error) => return invalid(format!("time_anchor_opentimestamps proof does not parse: {error}")),
    };
    let export_text = export_hash.as_str();
    if recorded.file_hash_op != OP_SHA256 || Some(hex(recorded.file_digest()).as_str()) != export_text {
        return invalid("the proof commits to a different file digest than the export hash");
    }
    if !recorded.timestamp.messages().contains(&commitment.as_slice()) {
        return invalid("the recorded commitment is not part of the proof");
    }
    let summary = Value::Array(attestation_summary(&recorded.timestamp));
    if !python_eq(field("attestations"), &summary) {
        return invalid("the recorded attestations do not match the proof");
    }

    let mut proof = recorded;
    if let Some(override_bytes) = proof_override {
        proof = match parse_detached(override_bytes) {
            Ok(parsed) => parsed,
            Err(error) => return invalid(format!("the supplied proof does not parse: {error}")),
        };
        if proof.file_hash_op != OP_SHA256 || Some(hex(proof.file_digest()).as_str()) != export_text {
            return invalid("the supplied proof is for a different file digest");
        }
        if !proof.timestamp.messages().contains(&commitment.as_slice()) {
            return invalid("the supplied proof does not contain the recorded commitment");
        }
    }

    let leaves = proof.timestamp.walk();
    let has_bitcoin = leaves.iter().any(|(_, attestation)| attestation.kind == AttestationKind::Bitcoin);
    if !has_bitcoin {
        let mut uris: Vec<&str> = leaves
            .iter()
            .filter(|(_, attestation)| attestation.kind == AttestationKind::Pending)
            .map(|(_, attestation)| attestation.uri.as_str())
            .collect();
        uris.sort_unstable();
        uris.dedup();
        return vec![finding(
            "info",
            "time_anchor_ots_pending",
            format!(
                "OpenTimestamps proof is pending at {} calendar(s); no Bitcoin attestation exists yet, so no external time is asserted",
                uris.len()
            ),
        )];
    }
    let mut heights: Vec<u64> = leaves
        .iter()
        .filter(|(_, attestation)| attestation.kind == AttestationKind::Bitcoin)
        .map(|(_, attestation)| attestation.height)
        .collect();
    heights.sort_unstable();
    heights.dedup();
    let heights_text = format!(
        "[{}]",
        heights.iter().map(u64::to_string).collect::<Vec<_>>().join(", ")
    );
    let Some(source) = header_source else {
        return vec![finding(
            "info",
            "time_anchor_ots_bitcoin_unchecked",
            format!(
                "The proof carries Bitcoin attestation(s) at height(s) {heights_text}; block headers were not consulted in this verification, so no time is asserted"
            ),
        )];
    };
    let mut findings = Vec::new();
    let mut earliest: Option<u32> = None;
    for check in check_bitcoin_attestations(&proof.timestamp, source) {
        match check.status {
            CheckStatus::Verified => {
                earliest = match (earliest, check.block_time) {
                    (Some(current), Some(time)) => Some(current.min(time)),
                    (None, time) => time,
                    (current, None) => current,
                };
            }
            CheckStatus::Mismatch => findings.push(finding(
                "error",
                "time_anchor_ots_invalid",
                format!("Bitcoin attestation at height {} failed: {}", check.height, check.reason),
            )),
            CheckStatus::Unavailable => findings.push(finding(
                "warning",
                "time_anchor_ots_header_unavailable",
                format!(
                    "Bitcoin attestation at height {} could not be checked: {}",
                    check.height, check.reason
                ),
            )),
        }
    }
    if let Some(time) = earliest {
        let checked = if source.kind() == "local_header" {
            "the attested digest equals the merkle root of a header you supplied from your own node, and that header meets its own proof-of-work target; that the header is on the best chain at that height was not checked here"
        } else {
            "the attested digest equals the merkle root of the header the configured explorer returned; the explorer is trusted to serve the best chain, and this is not an independent check"
        };
        findings.push(finding(
            "info",
            "time_anchor_ots_block_verified",
            format!(
                "The data existed no later than {} (block timestamp, set by the miner and only roughly accurate): {checked}",
                iso_from_ms(i64::from(time) * 1000)
            ),
        ));
    }
    findings
}
