//! RFC 3161 time-anchor protocol: request encoder, response parser, retained
//! token verification, manifest records and the verifier's `time_anchor` check.
//!
//! IMPORTANT: this is a byte-for-byte port of `daemon/time_anchor/anchor.py` and
//! `daemon/verify.py::_check_time_anchor`. The parse is structural only. The CMS
//! signature over the token is never verified, so no record may claim a proof
//! level above `inferred`.

use serde_json::{json, Map, Value};

use crate::finding::VerificationReport;
use crate::proof::{ProofLevel, PROOF_LEVEL_KEY};
use crate::pyvalue::{python_eq, python_str};

pub const TSA_TIMEOUT_SECONDS: u64 = 10;
pub const MAX_TSA_RESPONSE_BYTES: usize = 64 * 1024;
pub const DEFAULT_TSA_URL: &str = "http://timestamp.digicert.com";

/// 2.16.840.1.101.3.4.2.1 as a complete OID TLV.
const SHA256_ALGORITHM_OID: [u8; 11] = [
    0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01,
];
const TAG_SEQUENCE: u8 = 0x30;
const TAG_SET: u8 = 0x31;
const TAG_INTEGER: u8 = 0x02;
const TAG_OCTET_STRING: u8 = 0x04;
const TAG_NULL: u8 = 0x05;
const TAG_OID: u8 = 0x06;
const TAG_BOOLEAN: u8 = 0x01;
const TAG_GENERALIZED_TIME: u8 = 0x18;
const TAG_CONTEXT_0: u8 = 0xA0;

const UNAVAILABLE_SCOPE: &str = "No timestamp token was obtained; no external time is asserted.";
const ANCHORED_SCOPE: &str = "The retained DER TimeStampResp is externally verifiable against the \
TSA certificate chain; this daemon checked only that the token's \
message imprint and nonce match the request. It did NOT verify the \
token's CMS signature, so the time is a relayed third-party \
assertion, not a firsthand or cryptographically authenticated \
observation. Over plaintext HTTP an on-path attacker can substitute \
a forged token; verify the CMS signature against the TSA chain \
downstream before relying on the timestamp.";

/// A malformed or refused timestamp exchange. The message is the text the Python
/// `ValueError` carries, because it lands in the manifest's `reason`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TimeAnchorError(pub String);

fn fail<T>(message: &str) -> Result<T, TimeAnchorError> {
    Err(TimeAnchorError(message.to_owned()))
}

type Tlv<'a> = (u8, &'a [u8]);

fn der_length(length: usize) -> Vec<u8> {
    if length < 0x80 {
        return vec![length as u8];
    }
    let bytes = length.to_be_bytes();
    let body: Vec<u8> = bytes.iter().copied().skip_while(|byte| *byte == 0).collect();
    let mut out = vec![0x80 | body.len() as u8];
    out.extend(body);
    out
}

fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend(der_length(content.len()));
    out.extend_from_slice(content);
    out
}

/// Minimal big-endian magnitude: no leading zero bytes, empty for zero.
fn magnitude(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().copied().skip_while(|byte| *byte == 0).collect()
}

/// Python `int.to_bytes((bit_length + 8) // 8 or 1, "big")` of a non-negative
/// integer: the minimal two's-complement body with a sign-clearing zero byte.
fn der_integer_unsigned(value_bytes: &[u8]) -> Vec<u8> {
    let mut body = magnitude(value_bytes);
    if body.first().is_none_or(|byte| byte & 0x80 != 0) {
        body.insert(0, 0);
    }
    der(TAG_INTEGER, &body)
}

fn der_read(data: &[u8], offset: usize) -> Result<(u8, &[u8], usize), TimeAnchorError> {
    if offset.saturating_add(2) > data.len() {
        return fail("DER value overruns the buffer");
    }
    let (Some(tag), Some(first)) = (data.get(offset), data.get(offset + 1)) else {
        return fail("DER value overruns the buffer");
    };
    let (length, header) = if *first < 0x80 {
        (usize::from(*first), 2_usize)
    } else {
        let count = usize::from(first & 0x7F);
        if count == 0 || count > 4 {
            return fail("unsupported DER length encoding");
        }
        let Some(length_bytes) = data.get(offset + 2..offset + 2 + count) else {
            return fail("DER length overruns the buffer");
        };
        let length = length_bytes
            .iter()
            .fold(0_usize, |acc, byte| (acc << 8) | usize::from(*byte));
        (length, 2 + count)
    };
    let start = offset + header;
    let Some(end) = start.checked_add(length) else {
        return fail("DER value overruns the buffer");
    };
    let Some(content) = data.get(start..end) else {
        return fail("DER value overruns the buffer");
    };
    Ok((*tag, content, end))
}

fn der_children(content: &[u8]) -> Result<Vec<Tlv<'_>>, TimeAnchorError> {
    let mut children = Vec::new();
    let mut offset = 0;
    while offset < content.len() {
        let (tag, inner, next) = der_read(content, offset)?;
        children.push((tag, inner));
        offset = next;
    }
    Ok(children)
}

fn expect<'a>(
    children: &[Tlv<'a>],
    index: usize,
    tag: u8,
    what: &str,
) -> Result<&'a [u8], TimeAnchorError> {
    match children.get(index) {
        Some((found, content)) if *found == tag => Ok(content),
        _ => Err(TimeAnchorError(format!("malformed TimeStampResp: expected {what}"))),
    }
}

/// RFC 3161 TimeStampReq: v1, SHA-256 imprint, nonce, certReq TRUE.
pub fn encode_timestamp_request(
    data_hash_hex: &str,
    nonce: &[u8],
) -> Result<Vec<u8>, TimeAnchorError> {
    let hashed_message = python_from_hex(data_hash_hex)
        .ok_or_else(|| TimeAnchorError("non-hexadecimal number found in fromhex() arg".to_owned()))?;
    if hashed_message.len() != 32 {
        return fail("data hash must be a 64-character SHA-256 hex digest");
    }
    let mut algorithm = SHA256_ALGORITHM_OID.to_vec();
    algorithm.extend(der(TAG_NULL, &[]));
    let mut imprint = der(TAG_SEQUENCE, &algorithm);
    imprint.extend(der(TAG_OCTET_STRING, &hashed_message));
    let mut body = der_integer_unsigned(&[1]);
    body.extend(der(TAG_SEQUENCE, &imprint));
    body.extend(der_integer_unsigned(nonce));
    body.extend(der(TAG_BOOLEAN, &[0xFF]));
    Ok(der(TAG_SEQUENCE, &body))
}

/// The hash and nonce a [`encode_timestamp_request`] output carries.
pub fn parse_timestamp_request(data: &[u8]) -> Result<(String, Vec<u8>), TimeAnchorError> {
    let (tag, content, _) = der_read(data, 0)?;
    if tag != TAG_SEQUENCE {
        return fail("malformed TimeStampReq: not a SEQUENCE");
    }
    let children = der_children(content)?;
    let imprint = der_children(expect(&children, 1, TAG_SEQUENCE, "messageImprint")?)?;
    let hash = expect(&imprint, 1, TAG_OCTET_STRING, "hashedMessage")?;
    let nonce = expect(&children, 2, TAG_INTEGER, "nonce")?;
    Ok((hex(hash), magnitude(nonce)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTimestampResponse {
    pub granted: bool,
    pub status: u128,
    pub gentime_ms: Option<i64>,
    pub imprint_hash_hex: Option<String>,
    /// Unsigned big-endian magnitude, the way Python `int.from_bytes` reads it.
    pub nonce: Option<Vec<u8>>,
}

/// Extract PKIStatus, genTime, message imprint and nonce from a TimeStampResp.
///
/// Structural parse only: the CMS signature over the token is NOT verified.
pub fn parse_timestamp_response(data: &[u8]) -> Result<ParsedTimestampResponse, TimeAnchorError> {
    let (tag, response_content, _) = der_read(data, 0)?;
    if tag != TAG_SEQUENCE {
        return fail("malformed TimeStampResp: not a SEQUENCE");
    }
    let response_children = der_children(response_content)?;
    let status_children = der_children(expect(&response_children, 0, TAG_SEQUENCE, "PKIStatusInfo")?)?;
    let status_bytes = magnitude(expect(&status_children, 0, TAG_INTEGER, "PKIStatus")?);
    let status = status_bytes.iter().try_fold(0_u128, |acc, byte| {
        (status_bytes.len() <= 16).then(|| (acc << 8) | u128::from(*byte))
    });
    let Some(status) = status else {
        return fail("malformed TimeStampResp: PKIStatus is out of range");
    };
    let granted = status == 0 || status == 1;
    if !granted || response_children.len() < 2 {
        return Ok(ParsedTimestampResponse {
            granted: false,
            status,
            gentime_ms: None,
            imprint_hash_hex: None,
            nonce: None,
        });
    }

    let content_info = der_children(expect(&response_children, 1, TAG_SEQUENCE, "TimeStampToken")?)?;
    expect(&content_info, 0, TAG_OID, "signedData OID")?;
    let signed_data_wrap = der_children(expect(&content_info, 1, TAG_CONTEXT_0, "SignedData wrapper")?)?;
    let signed_data = der_children(expect(&signed_data_wrap, 0, TAG_SEQUENCE, "SignedData")?)?;
    expect(&signed_data, 0, TAG_INTEGER, "SignedData version")?;
    expect(&signed_data, 1, TAG_SET, "digestAlgorithms")?;
    let encap = der_children(expect(&signed_data, 2, TAG_SEQUENCE, "encapContentInfo")?)?;
    expect(&encap, 0, TAG_OID, "TSTInfo OID")?;
    let econtent = der_children(expect(&encap, 1, TAG_CONTEXT_0, "eContent wrapper")?)?;
    let tstinfo_der = expect(&econtent, 0, TAG_OCTET_STRING, "TSTInfo octets")?;
    let (tst_tag, tstinfo_content, _) = der_read(tstinfo_der, 0)?;
    if tst_tag != TAG_SEQUENCE {
        return fail("malformed TimeStampResp: TSTInfo is not a SEQUENCE");
    }
    let tstinfo = der_children(tstinfo_content)?;

    // TSTInfo: version, policy, messageImprint, serialNumber, genTime,
    //          accuracy?, ordering?, nonce?, tsa?, extensions?
    let imprint = der_children(expect(&tstinfo, 2, TAG_SEQUENCE, "messageImprint")?)?;
    let algorithm = expect(&imprint, 0, TAG_SEQUENCE, "imprint AlgorithmIdentifier")?;
    if !algorithm.starts_with(&SHA256_ALGORITHM_OID) {
        return fail("TSTInfo message imprint does not use SHA-256");
    }
    let imprint_hash_hex = hex(expect(&imprint, 1, TAG_OCTET_STRING, "hashedMessage")?);
    let Some(gentime_index) = tstinfo.iter().position(|(tag, _)| *tag == TAG_GENERALIZED_TIME) else {
        return fail("malformed TimeStampResp: TSTInfo has no genTime");
    };
    let gentime_text = tstinfo.get(gentime_index).map_or(&[][..], |(_, content)| *content);
    let gentime_ms = parse_gentime_ms(gentime_text)?;
    // serialNumber sits before genTime; the only INTEGER after it is the nonce.
    let nonce = tstinfo
        .iter()
        .skip(gentime_index + 1)
        .find(|(tag, _)| *tag == TAG_INTEGER)
        .map(|(_, content)| magnitude(content));
    Ok(ParsedTimestampResponse {
        granted: true,
        status,
        gentime_ms: Some(gentime_ms),
        imprint_hash_hex: Some(imprint_hash_hex),
        nonce,
    })
}

// ---------------------------------------------------------------------------
// genTime: `datetime.strptime(base, "%Y%m%d%H%M%S")`
// ---------------------------------------------------------------------------

type CharClass = (u8, u8);

const DIGIT: CharClass = (b'0', b'9');
const YEAR: &[&[CharClass]] = &[&[DIGIT, DIGIT, DIGIT, DIGIT]];
const MONTH: &[&[CharClass]] = &[
    &[(b'1', b'1'), (b'0', b'2')],
    &[(b'0', b'0'), (b'1', b'9')],
    &[(b'1', b'9')],
];
const DAY: &[&[CharClass]] = &[
    &[(b'3', b'3'), (b'0', b'1')],
    &[(b'1', b'2'), DIGIT],
    &[(b'0', b'0'), (b'1', b'9')],
    &[(b'1', b'9')],
    &[(b' ', b' '), (b'1', b'9')],
];
const HOUR: &[&[CharClass]] = &[
    &[(b'2', b'2'), (b'0', b'3')],
    &[(b'0', b'1'), DIGIT],
    &[DIGIT],
];
const MINUTE: &[&[CharClass]] = &[&[(b'0', b'5'), DIGIT], &[DIGIT]];
const SECOND: &[&[CharClass]] = &[
    &[(b'6', b'6'), (b'0', b'1')],
    &[(b'0', b'5'), DIGIT],
    &[DIGIT],
];
const GENTIME_FIELDS: [&[&[CharClass]]; 6] = [YEAR, MONTH, DAY, HOUR, MINUTE, SECOND];

/// Backtracking match of CPython's `_strptime` regex for `%Y%m%d%H%M%S`, which
/// accepts one- and two-digit fields and leaves unanchored trailing input for a
/// later "unconverted data" check. Returns the field lengths and the end offset.
fn match_fields(text: &[u8], position: usize, field: usize, lengths: &mut Vec<usize>) -> Option<usize> {
    let Some(alternatives) = GENTIME_FIELDS.get(field) else {
        return Some(position);
    };
    for alternative in *alternatives {
        let matches = alternative.iter().enumerate().all(|(index, (low, high))| {
            text.get(position + index)
                .is_some_and(|byte| byte >= low && byte <= high)
        });
        if !matches {
            continue;
        }
        lengths.push(alternative.len());
        if let Some(end) = match_fields(text, position + alternative.len(), field + 1, lengths) {
            return Some(end);
        }
        lengths.pop();
    }
    None
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn parse_gentime_ms(content: &[u8]) -> Result<i64, TimeAnchorError> {
    let Ok(text) = core::str::from_utf8(content).map_err(|_| ()).and_then(|text| {
        text.is_ascii().then_some(text).ok_or(())
    }) else {
        return fail("TSTInfo genTime is not ASCII");
    };
    let base = text.trim_end_matches('Z').split('.').next().unwrap_or_default();
    let bytes = base.as_bytes();
    let mut lengths = Vec::new();
    let Some(end) = match_fields(bytes, 0, 0, &mut lengths) else {
        return Err(TimeAnchorError(format!(
            "time data '{base}' does not match format '%Y%m%d%H%M%S'"
        )));
    };
    if end != bytes.len() {
        return Err(TimeAnchorError(format!(
            "unconverted data remains when parsing with format '%Y%m%d%H%M%S': '{}'",
            base.get(end..).unwrap_or_default()
        )));
    }
    let mut values = [0_i64; 6];
    let mut position = 0;
    for (slot, length) in values.iter_mut().zip(&lengths) {
        let field = base.get(position..position + length).unwrap_or_default();
        *slot = field.trim_start().parse::<i64>().unwrap_or(0);
        position += length;
    }
    let [year, month, day, hour, minute, second] = values;
    if year < 1 {
        return fail("year 0 is out of range");
    }
    if day > days_in_month(year, month) {
        return fail("day is out of range for month");
    }
    if second > 59 {
        return fail("second must be in 0..59");
    }
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    Ok(seconds * 1000)
}

/// Python `datetime.fromtimestamp(ms / 1000, utc).isoformat()` with `+00:00`
/// replaced by `Z`.
pub fn iso_from_ms(timestamp_ms: i64) -> String {
    let seconds = timestamp_ms.div_euclid(1000);
    let millis = timestamp_ms.rem_euclid(1000);
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let in_day = seconds.rem_euclid(86_400);
    let mut text = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        in_day / 3600,
        in_day % 3600 / 60,
        in_day % 60
    );
    if millis != 0 {
        text.push_str(&format!(".{:06}", millis * 1000));
    }
    text.push('Z');
    text
}

// ---------------------------------------------------------------------------
// hex helpers
// ---------------------------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Python `bytes.fromhex`: ASCII whitespace is skipped between bytes, never
/// inside one; either case is accepted; an odd digit count is an error.
pub fn python_from_hex(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut index = 0;
    while index < bytes.len() {
        let byte = *bytes.get(index)?;
        if matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C) {
            index += 1;
            continue;
        }
        let high = char::from(byte).to_digit(16)?;
        let low = char::from(*bytes.get(index + 1)?).to_digit(16)?;
        out.push((high * 16 + low) as u8);
        index += 2;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// proof, verification and records
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeProof {
    pub source: String,
    pub timestamp_ms: i128,
    pub nonce_hex: String,
    pub response_hex: String,
}

impl TimeProof {
    /// The checks `RFC3161Provider.anchor` runs on a TSA reply, in order. Never
    /// falls back to the local clock: the time is the token's `genTime`.
    pub fn from_response(
        tsa_url: &str,
        data_hash: &str,
        nonce: &[u8],
        body: &[u8],
    ) -> Result<TimeProof, TimeAnchorError> {
        if body.len() > MAX_TSA_RESPONSE_BYTES {
            return fail("TSA response exceeds the size bound");
        }
        let parsed = parse_timestamp_response(body)?;
        if !parsed.granted {
            return Err(TimeAnchorError(format!(
                "TSA refused the timestamp request (PKIStatus={})",
                parsed.status
            )));
        }
        if parsed.imprint_hash_hex.as_deref() != Some(data_hash) {
            return fail("TSA token message imprint does not match the anchored hash");
        }
        // RFC 3161 section 2.4.2: the nonce echo is the replay protection.
        if parsed.nonce.as_deref() != Some(magnitude(nonce).as_slice()) {
            return fail("TSA token nonce does not match the request nonce");
        }
        let Some(gentime_ms) = parsed.gentime_ms else {
            return fail("TSA token has no genTime");
        };
        Ok(TimeProof {
            source: format!("rfc3161:{tsa_url}"),
            timestamp_ms: i128::from(gentime_ms),
            nonce_hex: hex(nonce),
            response_hex: hex(body),
        })
    }
}

/// `RFC3161Provider.verify`: the retained token must grant, carry this hash and
/// nonce, and report the same time as the record.
pub fn verify_time_proof(proof: &TimeProof, data_hash: &str) -> bool {
    let Some(token) = python_from_hex(&proof.response_hex) else {
        return false;
    };
    let Some(nonce) = python_from_hex(&proof.nonce_hex) else {
        return false;
    };
    let Ok(parsed) = parse_timestamp_response(&token) else {
        return false;
    };
    parsed.granted
        && parsed.imprint_hash_hex.as_deref() == Some(data_hash)
        && parsed.nonce.as_deref() == Some(magnitude(&nonce).as_slice())
        && parsed.gentime_ms.map(i128::from) == Some(proof.timestamp_ms)
}

/// IMPORTANT: an unavailable anchor asserts no external time and stays at
/// `unknown_unobserved`.
pub fn unavailable_anchor_record(data_hash: &str, reason: &str) -> Value {
    json!({
        "status": "unavailable",
        "data_hash": data_hash,
        "reason": reason,
        "scope": UNAVAILABLE_SCOPE,
        PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
    })
}

/// IMPORTANT: never `directly_observed`. The daemon relays a third-party time
/// assertion whose CMS signature it did not verify; `inferred` is the ceiling.
pub fn anchored_record(data_hash: &str, proof: &TimeProof) -> Value {
    let mut record = Map::new();
    record.insert("status".to_owned(), json!("anchored"));
    record.insert("data_hash".to_owned(), json!(data_hash));
    record.insert("source".to_owned(), json!(proof.source));
    record.insert("timestamp_ms".to_owned(), timestamp_value(proof.timestamp_ms));
    record.insert(
        "timestamp".to_owned(),
        json!(i64::try_from(proof.timestamp_ms).map(iso_from_ms).unwrap_or_default()),
    );
    record.insert("nonce_hex".to_owned(), json!(proof.nonce_hex));
    record.insert("response_der_hex".to_owned(), json!(proof.response_hex));
    record.insert("cms_signature_verified".to_owned(), json!(false));
    record.insert("scope".to_owned(), json!(ANCHORED_SCOPE));
    record.insert(PROOF_LEVEL_KEY.to_owned(), json!(ProofLevel::Inferred.as_str()));
    Value::Object(record)
}

fn timestamp_value(timestamp_ms: i128) -> Value {
    i64::try_from(timestamp_ms).map_or(Value::Null, Value::from)
}

const TIME_ANCHOR_LABELS: [&str; 2] = ["inferred", "unknown_unobserved"];

/// Whether a JSON number is a Python `int` (no fraction or exponent). Returns it
/// when it fits `i128`.
fn python_int(value: &Value) -> Option<Option<i128>> {
    let Value::Number(number) = value else {
        return None;
    };
    let text = number.to_string();
    if text.contains(['.', 'e', 'E']) {
        return None;
    }
    Some(text.parse::<i128>().ok())
}

/// `daemon/verify.py::_check_time_anchor`.
pub fn check_time_anchor(data: &Value, report: &mut VerificationReport) {
    let Some(anchor) = data.get("time_anchor") else {
        return;
    };
    if anchor.is_null() {
        return;
    }
    let Value::Object(anchor) = anchor else {
        report.error("time_anchor_invalid", "time_anchor must be an object");
        return;
    };
    let field = |key: &str| anchor.get(key).unwrap_or(&Value::Null);
    match field("status").as_str() {
        Some("unavailable") => {
            report.info(
                "time_anchor_unavailable",
                "No external timestamp was obtained; no external time is asserted",
            );
            return;
        }
        Some("anchored") => {}
        _ => {
            report.error(
                "time_anchor_invalid",
                "time_anchor status must be anchored or unavailable",
            );
            return;
        }
    }
    let label_ok = field(PROOF_LEVEL_KEY)
        .as_str()
        .is_some_and(|label| TIME_ANCHOR_LABELS.contains(&label));
    if !label_ok || field("cms_signature_verified") != &Value::Bool(false) {
        report.error(
            "time_anchor_invalid",
            "time_anchor claims stronger evidence than a relayed, unauthenticated TSA token",
        );
        return;
    }
    let export_hash = data
        .get("export")
        .and_then(Value::as_object)
        .and_then(|export| export.get("sha256"))
        .unwrap_or(&Value::Null);
    if !python_eq(field("data_hash"), export_hash) {
        report.error(
            "time_anchor_invalid",
            "time_anchor data_hash does not match the export hash",
        );
        return;
    }
    let timestamp_ms = python_int(field("timestamp_ms"));
    let (Some(timestamp_ms), Some(source), Some(nonce_hex), Some(token_hex)) = (
        timestamp_ms,
        field("source").as_str(),
        field("nonce_hex").as_str(),
        field("response_der_hex").as_str(),
    ) else {
        report.error("time_anchor_invalid", "time_anchor is missing its token fields");
        return;
    };
    let proof = TimeProof {
        source: source.to_owned(),
        // An integer wider than i128 cannot equal any genTime.
        timestamp_ms: timestamp_ms.unwrap_or(i128::MAX),
        nonce_hex: nonce_hex.to_owned(),
        response_hex: token_hex.to_owned(),
    };
    if !verify_time_proof(&proof, &python_str(export_hash)) {
        report.error(
            "time_anchor_invalid",
            "time_anchor token does not match its imprint, nonce, or reported time",
        );
        return;
    }
    report.info(
        "time_anchor_consistent",
        "TSA token imprint, nonce, and time are consistent; its CMS signature was not verified",
    );
}
