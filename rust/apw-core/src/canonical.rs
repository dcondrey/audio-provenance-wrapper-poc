use serde::Serialize;
use serde_json::{Map, Number, Value};

use crate::error::{CoreError, Result};
use crate::guard::{Guarded, NON_FINITE_MARKER};

/// Two canonicalizations exist and they are NOT interchangeable. Both sort keys
/// and use `(",", ":")` separators; they differ only in non-ASCII handling and
/// diverge the moment a track name, file name or `.als` string is non-ASCII.
/// `{"name":"Café"}` canonicalizes to raw UTF-8 under [`Canonicalization::Utf8`]
/// and to `Café` under [`Canonicalization::AsciiEscaped`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Canonicalization {
    /// `apw-json-sort-v1`: raw UTF-8. Ed25519 portable signature, evidence JSONL,
    /// UDP acknowledgements, and the evidence-bundle index id.
    Utf8,
    /// `apw-json-sort-ascii-v1`: `\uXXXX`-escaped. The local HMAC
    /// `manifest_signature` input and the verifier's tamper recomputation.
    AsciiEscaped,
}

pub const CANONICALIZATION_PORTABLE: &str = "apw-json-sort-v1";
pub const CANONICALIZATION_LOCAL: &str = "apw-json-sort-ascii-v1";

pub const PORTABLE_SIGNATURE_EXCLUDED_KEYS: [&str; 2] = ["portable_signature", "manifest_signature"];
pub const LOCAL_SIGNATURE_EXCLUDED_KEYS: [&str; 1] = ["manifest_signature"];

/// Matches `daemon/schema.py::_MAX_PROOF_VALUE_DEPTH`. Applied to the writers as
/// well: a crafted document must produce an error, never a stack overflow.
pub const MAX_PROOF_VALUE_DEPTH: usize = 64;

impl Canonicalization {
    pub fn label(self) -> &'static str {
        match self {
            Canonicalization::Utf8 => CANONICALIZATION_PORTABLE,
            Canonicalization::AsciiEscaped => CANONICALIZATION_LOCAL,
        }
    }
}

/// REQUIRED: rejects non-finite floats (Python `allow_nan=False`); serde_json's
/// default would silently emit `null`. Formats every f64 with [`python_repr_f64`].
pub fn canonical_json(value: &Value, mode: Canonicalization) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_canonical(&mut out, value, mode, "$", 0)?;
    Ok(out)
}

pub fn canonical_json_utf8(value: &Value) -> Result<Vec<u8>> {
    canonical_json(value, Canonicalization::Utf8)
}

pub fn canonical_json_ascii(value: &Value) -> Result<Vec<u8>> {
    canonical_json(value, Canonicalization::AsciiEscaped)
}

/// IMPORTANT: `serde_json` writes `null` for a non-finite f64 inside
/// `serialize_f64`, before any `Formatter` or `to_value` can observe it. The
/// [`crate::guard`] wrapper intercepts the value itself, so a NaN is rejected
/// here instead of being laundered into a signed manifest as `null`.
pub fn canonical_json_of<T: Serialize>(value: &T, mode: Canonicalization) -> Result<Vec<u8>> {
    let parsed = serde_json::to_value(Guarded(value)).map_err(|err| {
        if err.to_string().contains(NON_FINITE_MARKER) {
            CoreError::NonFiniteNumber {
                pointer: "$".to_owned(),
            }
        } else {
            CoreError::Json(err)
        }
    })?;
    canonical_json(&parsed, mode)
}

/// `json.dumps(..., indent=2, ensure_ascii=False)` plus a trailing newline.
/// Insertion order is preserved; this output is never a signing input.
pub fn pretty_json_bytes(value: &Value) -> Result<Vec<u8>> {
    pretty_json(value, false)
}

/// `json.dumps(..., indent=2, ensure_ascii=False, sort_keys=True)` plus a
/// trailing newline: `daemon/bundle.py`'s evidence-index rendering.
///
/// IMPORTANT: those exact bytes are hashed into the published bundle, so the
/// index is the one pretty rendering that sorts. Using
/// [`pretty_json_bytes`] for it would produce a different digest.
pub fn pretty_json_sorted_bytes(value: &Value) -> Result<Vec<u8>> {
    pretty_json(value, true)
}

fn pretty_json(value: &Value, sort_keys: bool) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_pretty(&mut out, value, 0, "$", sort_keys)?;
    out.push(b'\n');
    Ok(out)
}

/// Shallow top-level key removal, matching the Python dict comprehensions that
/// build each signing input. A non-object is returned unchanged.
pub fn without_top_level_keys(value: &Value, keys: &[&str]) -> Value {
    match value {
        Value::Object(map) => {
            let mut kept = Map::new();
            for (key, child) in map {
                if !keys.contains(&key.as_str()) {
                    kept.insert(key.clone(), child.clone());
                }
            }
            Value::Object(kept)
        }
        other => other.clone(),
    }
}

fn write_canonical(
    out: &mut Vec<u8>,
    value: &Value,
    mode: Canonicalization,
    pointer: &str,
    depth: usize,
) -> Result<()> {
    if depth >= MAX_PROOF_VALUE_DEPTH {
        return Err(CoreError::NestingTooDeep {
            pointer: pointer.to_owned(),
            max: MAX_PROOF_VALUE_DEPTH,
        });
    }
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(number) => write_number(out, number, pointer)?,
        Value::String(text) => write_json_string(out, text, mode),
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_canonical(out, item, mode, &format!("{pointer}[{index}]"), depth + 1)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            // Python `sort_keys=True` orders by code point; Rust's `str` Ord is
            // byte-wise UTF-8, which is the same order.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push(b'{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_json_string(out, key, mode);
                out.push(b':');
                let child = map.get(key.as_str()).unwrap_or(&Value::Null);
                write_canonical(out, child, mode, &format!("{pointer}.{key}"), depth + 1)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn write_pretty(
    out: &mut Vec<u8>,
    value: &Value,
    depth: usize,
    pointer: &str,
    sort_keys: bool,
) -> Result<()> {
    if depth >= MAX_PROOF_VALUE_DEPTH {
        return Err(CoreError::NestingTooDeep {
            pointer: pointer.to_owned(),
            max: MAX_PROOF_VALUE_DEPTH,
        });
    }
    let inner_pad = "  ".repeat(depth + 1);
    let close_pad = "  ".repeat(depth);
    match value {
        Value::Array(items) if !items.is_empty() => {
            out.extend_from_slice(b"[\n");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.extend_from_slice(b",\n");
                }
                out.extend_from_slice(inner_pad.as_bytes());
                write_pretty(out, item, depth + 1, &format!("{pointer}[{index}]"), sort_keys)?;
            }
            out.push(b'\n');
            out.extend_from_slice(close_pad.as_bytes());
            out.push(b']');
        }
        Value::Object(map) if !map.is_empty() => {
            out.extend_from_slice(b"{\n");
            let mut keys: Vec<&String> = map.keys().collect();
            if sort_keys {
                keys.sort_unstable();
            }
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.extend_from_slice(b",\n");
                }
                out.extend_from_slice(inner_pad.as_bytes());
                write_json_string(out, key, Canonicalization::Utf8);
                out.extend_from_slice(b": ");
                let child = map.get(key.as_str()).unwrap_or(&Value::Null);
                write_pretty(out, child, depth + 1, &format!("{pointer}.{key}"), sort_keys)?;
            }
            out.push(b'\n');
            out.extend_from_slice(close_pad.as_bytes());
            out.push(b'}');
        }
        other => write_canonical(out, other, Canonicalization::Utf8, pointer, depth)?,
    }
    Ok(())
}

fn write_number(out: &mut Vec<u8>, number: &Number, pointer: &str) -> Result<()> {
    if let Some(unsigned) = number.as_u64() {
        out.extend_from_slice(unsigned.to_string().as_bytes());
        return Ok(());
    }
    if let Some(signed) = number.as_i64() {
        out.extend_from_slice(signed.to_string().as_bytes());
        return Ok(());
    }
    // IMPORTANT: an integer literal wider than 64 bits must keep its exact digits. Widening it to
    // f64 would emit exponent notation where Python emits the integer, and divergent bytes out of
    // the signing-input canonicalizer mean a signature made by one implementation fails in the
    // other. arbitrary_precision preserves the source text, which is the only way to tell an
    // oversized integer from a float literal that happens to be integral, such as 1e21.
    let literal = number.as_str();
    if !literal.contains(['.', 'e', 'E']) {
        out.extend_from_slice(literal.as_bytes());
        return Ok(());
    }
    match number.as_f64() {
        Some(float) if float.is_finite() => {
            out.extend_from_slice(python_repr_f64(float)?.as_bytes());
            Ok(())
        }
        _ => Err(CoreError::NonFiniteNumber {
            pointer: pointer.to_owned(),
        }),
    }
}

fn write_json_string(out: &mut Vec<u8>, text: &str, mode: Canonicalization) {
    out.push(b'"');
    for character in text.chars() {
        match character {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            '\r' => out.extend_from_slice(b"\\r"),
            _ => {
                let code = character as u32;
                let escape_non_ascii = mode == Canonicalization::AsciiEscaped && code > 0x7e;
                if code < 0x20 || escape_non_ascii {
                    write_unicode_escape(out, code);
                } else {
                    let mut buffer = [0u8; 4];
                    out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                }
            }
        }
    }
    out.push(b'"');
}

fn write_unicode_escape(out: &mut Vec<u8>, code: u32) {
    if code > 0xffff {
        let adjusted = code - 0x1_0000;
        let high = 0xd800 + (adjusted >> 10);
        let low = 0xdc00 + (adjusted & 0x3ff);
        out.extend_from_slice(format!("\\u{high:04x}\\u{low:04x}").as_bytes());
    } else {
        out.extend_from_slice(format!("\\u{code:04x}").as_bytes());
    }
}

/// CPython `repr(float)` exactly: shortest round-tripping digits, fixed notation
/// while `-4 < decpt <= 16`, otherwise scientific with a signed, zero-padded,
/// at-least-two-digit exponent. Verified divergences from serde_json/ryu:
/// `1e-07` vs `1e-7`, `1e-05` vs `0.00001`, `3.0517578125e-05` vs
/// `0.000030517578125`.
pub fn python_repr_f64(value: f64) -> Result<String> {
    if !value.is_finite() {
        return Err(CoreError::NonFiniteNumber {
            pointer: "$".to_owned(),
        });
    }
    let magnitude = value.abs();
    // Rust's `{:e}` emits the same shortest-round-trip digit string CPython's
    // repr uses, so only the placement rules below differ.
    let scientific = format!("{magnitude:e}");
    let (mantissa, exponent_text) =
        scientific
            .split_once('e')
            .ok_or_else(|| CoreError::InvalidEnumValue {
                field: "float_repr",
                value: scientific.clone(),
            })?;
    let exponent: i64 = exponent_text
        .parse()
        .map_err(|_| CoreError::InvalidEnumValue {
            field: "float_repr",
            value: scientific.clone(),
        })?;
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let digit_count = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    let decimal_point = exponent + 1;

    let sign = if value.is_sign_negative() { "-" } else { "" };
    if decimal_point <= -4 || decimal_point > 16 {
        let head = digits.get(..1).unwrap_or("0");
        let tail = digits.get(1..).unwrap_or("");
        let fraction = if tail.is_empty() {
            String::new()
        } else {
            format!(".{tail}")
        };
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        let exponent_magnitude = exponent.unsigned_abs();
        return Ok(format!(
            "{sign}{head}{fraction}e{exponent_sign}{exponent_magnitude:02}"
        ));
    }
    if decimal_point <= 0 {
        let zeros = "0".repeat(usize::try_from(-decimal_point).unwrap_or(0));
        return Ok(format!("{sign}0.{zeros}{digits}"));
    }
    if decimal_point >= digit_count {
        let zeros = "0".repeat(usize::try_from(decimal_point - digit_count).unwrap_or(0));
        return Ok(format!("{sign}{digits}{zeros}.0"));
    }
    let split = usize::try_from(decimal_point).unwrap_or(0);
    let head = digits.get(..split).unwrap_or("0");
    let tail = digits.get(split..).unwrap_or("");
    Ok(format!("{sign}{head}.{tail}"))
}
