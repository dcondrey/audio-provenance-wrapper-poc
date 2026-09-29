//! Python-compatible number parsing for the project parsers. Only ASCII digits are
//! understood; Python also accepts other Unicode decimal digits, which real project
//! files do not use.

use super::safe::is_python_space;

fn strip(text: &str) -> &str {
    text.trim_matches(is_python_space)
}

/// Underscores are legal only between digits (`1_000`).
fn without_underscores(text: &str) -> Option<String> {
    if !text.contains('_') {
        return Some(text.to_owned());
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, c) in chars.iter().enumerate() {
        if *c == '_' {
            let before = index.checked_sub(1).and_then(|i| chars.get(i)).is_some_and(char::is_ascii_digit);
            let after = chars.get(index + 1).is_some_and(char::is_ascii_digit);
            if !(before && after) {
                return None;
            }
        } else {
            out.push(*c);
        }
    }
    Some(out)
}

/// Python `float(text)`.
pub fn py_float(text: &str) -> Option<f64> {
    let cleaned = without_underscores(strip(text))?;
    if cleaned.is_empty() || cleaned.contains(|c: char| c.is_whitespace()) {
        return None;
    }
    // Rust accepts nothing Python rejects here except a leading '+' on inf/nan,
    // which Python accepts too.
    cleaned.parse::<f64>().ok()
}

/// Python `int(text)` (base 10); values beyond `i128` saturate.
pub fn py_int(text: &str) -> Option<i128> {
    let cleaned = without_underscores(strip(text))?;
    let (negative, digits) = match cleaned.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, cleaned.strip_prefix('+').unwrap_or(&cleaned)),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let magnitude = digits.parse::<i128>().unwrap_or(i128::MAX);
    Some(if negative { -magnitude } else { magnitude })
}

/// Python `int(text, 16)`: optional sign, optional `0x`, hex digits.
pub fn py_int_hex(text: &str) -> Option<i128> {
    let cleaned = without_underscores(strip(text))?;
    let (negative, rest) = match cleaned.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, cleaned.strip_prefix('+').unwrap_or(&cleaned)),
    };
    let digits = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")).unwrap_or(rest);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let magnitude = i128::from_str_radix(digits, 16).unwrap_or(i128::MAX);
    Some(if negative { -magnitude } else { magnitude })
}

/// Python `int(float(text))`: `Ok(None)` for what `except (TypeError, ValueError)`
/// swallows (unparseable, NaN); `Err` for what propagates (an infinity raises
/// `OverflowError`, which aborts the parse).
pub fn py_int_of_float(text: &str) -> Result<Option<i64>, String> {
    let Some(value) = py_float(text) else {
        return Ok(None);
    };
    if value.is_nan() {
        return Ok(None);
    }
    if value.is_infinite() {
        return Err("cannot convert float infinity to integer".to_owned());
    }
    Ok(Some(value.trunc() as i64))
}
