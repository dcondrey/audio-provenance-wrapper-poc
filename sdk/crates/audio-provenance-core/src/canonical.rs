use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde_json::{Number, Value};

use crate::error::CanonicalJsonError;

pub const CANONICALIZATION_ID: &str = "apw-json-sort-v1";

/// IMPORTANT: mirrors `_MAX_PROOF_VALUE_DEPTH` in the POC's `daemon/schema.py`. The root value sits
/// at depth 0 and any value reached at depth 64 is rejected, so hostile nesting cannot exhaust the
/// stack. The POC canonicaliser itself is unbounded; this bound is deliberate hardening, not parity.
pub const MAX_DEPTH: usize = 64;

pub fn canonical_json(value: &Value) -> Result<Vec<u8>, CanonicalJsonError> {
    let mut out = Vec::with_capacity(256);
    write_value(value, 0, "$", &mut out)?;
    Ok(out)
}

fn write_value(
    value: &Value,
    depth: usize,
    path: &str,
    out: &mut Vec<u8>,
) -> Result<(), CanonicalJsonError> {
    if depth >= MAX_DEPTH {
        return Err(CanonicalJsonError::DepthExceeded {
            limit: MAX_DEPTH,
            path: path.to_string(),
        });
    }
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => write_number(n, out)?,
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_value(item, depth + 1, &format!("{path}[{index}]"), out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            // IMPORTANT: Python sorts keys by Unicode code point; UTF-8 byte order is the same
            // total order. Sorting explicitly keeps that true even if serde_json's `preserve_order`
            // feature is switched on elsewhere in the workspace by feature unification.
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            out.push(b'{');
            for (index, (key, child)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_string(key, out);
                out.push(b':');
                write_value(child, depth + 1, &format!("{path}.{key}"), out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn write_number(number: &Number, out: &mut Vec<u8>) -> Result<(), CanonicalJsonError> {
    if let Some(u) = number.as_u64() {
        out.extend_from_slice(u.to_string().as_bytes());
    } else if let Some(i) = number.as_i64() {
        out.extend_from_slice(i.to_string().as_bytes());
    } else if let Some(f) = number.as_f64() {
        if !f.is_finite() {
            return Err(CanonicalJsonError::NonFiniteFloat);
        }
        out.extend_from_slice(python_repr_f64(f).as_bytes());
    } else {
        return Err(CanonicalJsonError::UnrepresentableNumber);
    }
    Ok(())
}

/// Parses JSON that is about to be canonicalised and signed, or whose signature is about to be
/// checked.
///
/// IMPORTANT: `serde_json` widens an integer literal outside i64/u64 into an f64. Python keeps the
/// int/float distinction, so `18446744073709551616` and `1.8446744073709552e19` are two documents
/// to the signer and one document here. Because the canonical bytes are the signed content, that
/// collapse would let a signature issued over one verify the other. Rejecting the literal at the
/// boundary is the only place the distinction still exists.
pub fn parse_signing_input(bytes: &[u8]) -> Result<Value, CanonicalJsonError> {
    reject_oversized_integers(bytes)?;
    serde_json::from_slice(bytes).map_err(|error| CanonicalJsonError::Malformed {
        reason: error.to_string(),
    })
}

fn reject_oversized_integers(bytes: &[u8]) -> Result<(), CanonicalJsonError> {
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => index = skip_string(bytes, index),
            b'-' | b'0'..=b'9' => {
                let start = index;
                index = skip_number(bytes, index);
                let literal = &bytes[start..index];
                if is_integer_literal(literal) && !fits_in_64_bits(literal) {
                    return Err(CanonicalJsonError::IntegerOutOfRange {
                        literal: String::from_utf8_lossy(literal).into_owned(),
                    });
                }
            }
            _ => index += 1,
        }
    }
    Ok(())
}

fn skip_string(bytes: &[u8], open_quote: usize) -> usize {
    let mut index = open_quote + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return index + 1,
            _ => index += 1,
        }
    }
    index
}

fn skip_number(bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    if index < bytes.len() && bytes[index] == b'-' {
        index += 1;
    }
    while index < bytes.len()
        && matches!(bytes[index], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
    {
        index += 1;
    }
    index
}

fn is_integer_literal(literal: &[u8]) -> bool {
    !literal
        .iter()
        .any(|byte| matches!(byte, b'.' | b'e' | b'E'))
}

fn fits_in_64_bits(literal: &[u8]) -> bool {
    let Ok(text) = core::str::from_utf8(literal) else {
        return false;
    };
    text.parse::<i64>().is_ok() || text.parse::<u64>().is_ok()
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn write_string(value: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    let mut utf8 = [0u8; 4];
    for ch in value.chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{08}' => out.extend_from_slice(b"\\b"),
            '\u{09}' => out.extend_from_slice(b"\\t"),
            '\u{0a}' => out.extend_from_slice(b"\\n"),
            '\u{0c}' => out.extend_from_slice(b"\\f"),
            '\u{0d}' => out.extend_from_slice(b"\\r"),
            c if (c as u32) < 0x20 => {
                let v = c as u32;
                out.extend_from_slice(b"\\u00");
                out.push(HEX[((v >> 4) & 0xf) as usize]);
                out.push(HEX[(v & 0xf) as usize]);
            }
            c => out.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes()),
        }
    }
    out.push(b'"');
}

/// Splits a Rust `{:e}` rendering into its digit string and the power of ten applying to the
/// leading digit, with trailing zeros removed.
fn split_scientific(rendered: &str) -> (String, i32) {
    let mut digits = String::with_capacity(24);
    let mut exponent: i32 = 0;
    let mut exponent_negative = false;
    let mut past_e = false;
    for ch in rendered.chars() {
        match ch {
            'e' => past_e = true,
            '-' if past_e => exponent_negative = true,
            '0'..='9' if past_e => {
                exponent = exponent
                    .saturating_mul(10)
                    .saturating_add(i32::from(ch as u8 - b'0'));
            }
            '0'..='9' => digits.push(ch),
            _ => {}
        }
    }
    if exponent_negative {
        exponent = -exponent;
    }
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    (digits, exponent)
}

fn round_trips(digits: &str, exponent: i32, magnitude: f64) -> bool {
    let scale = exponent.saturating_sub(digits.len() as i32 - 1);
    format!("{digits}e{scale}")
        .parse::<f64>()
        .is_ok_and(|parsed| parsed == magnitude)
}

/// IMPORTANT: reproduces CPython `format_float_short` in repr mode, which `json.dumps` uses
/// verbatim. Two things stop this being a delegation to `serde_json` or to Rust's own `{}`:
/// `serde_json` writes `0.00001` where CPython writes `1e-05` and does not zero-pad the exponent to
/// two digits; and Rust's shortest-form printer breaks an exact decimal tie away from zero where
/// CPython's dtoa breaks it to even, which is why the shortest length is taken from `{:e}` but the
/// digits themselves are re-derived with `{:.*e}`, whose rounding is exact and half-even.
fn python_repr_f64(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_string()
        } else {
            "0.0".to_string()
        };
    }
    let negative = value.is_sign_negative();
    let magnitude = if negative { -value } else { value };

    let (shortest, shortest_exponent) = split_scientific(&format!("{magnitude:e}"));
    let precision = shortest.len().saturating_sub(1);
    let (even, even_exponent) = split_scientific(&format!("{magnitude:.precision$e}"));
    let (digits, exponent) = if round_trips(&even, even_exponent, magnitude) {
        (even, even_exponent)
    } else {
        (shortest, shortest_exponent)
    };

    let digit_count = digits.len() as i32;
    let decimal_point = exponent.saturating_add(1);

    let mut out = String::with_capacity(digits.len() + 8);
    if negative {
        out.push('-');
    }
    if decimal_point <= -4 || decimal_point > 16 {
        let mut chars = digits.chars();
        if let Some(first) = chars.next() {
            out.push(first);
        }
        let rest = chars.as_str();
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
        }
        let scaled = decimal_point.saturating_sub(1);
        out.push('e');
        out.push(if scaled < 0 { '-' } else { '+' });
        let scaled_magnitude = scaled.unsigned_abs();
        if scaled_magnitude < 10 {
            out.push('0');
        }
        out.push_str(&scaled_magnitude.to_string());
    } else if decimal_point <= 0 {
        out.push_str("0.");
        for _ in 0..(-decimal_point) {
            out.push('0');
        }
        out.push_str(&digits);
    } else if decimal_point >= digit_count {
        out.push_str(&digits);
        for _ in 0..(decimal_point - digit_count) {
            out.push('0');
        }
        out.push_str(".0");
    } else {
        let split = decimal_point as usize;
        out.push_str(&digits[..split]);
        out.push('.');
        out.push_str(&digits[split..]);
    }
    out
}
