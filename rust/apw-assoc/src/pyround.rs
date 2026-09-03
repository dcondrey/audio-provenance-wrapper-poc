use core::fmt::Write as _;

/// Enough fractional digits for the exact decimal expansion of any finite f64
/// (the smallest subnormal terminates at 1074 places).
const EXACT_FRACTION_DIGITS: usize = 1080;

/// CPython's `round(value, ndigits)`: correctly rounded on the exact decimal
/// expansion of the double, ties to even, then converted back to the nearest
/// double.
///
/// IMPORTANT: the naive `(x * 10f64.powi(n)).round() / 10f64.powi(n)` disagrees
/// with CPython on values whose scaled form lands on the wrong side of a tie,
/// and `round()` rounds half away from zero where CPython rounds half to even.
/// Every rendered field of an association record passes through here, so a
/// divergence would show up as a changed manifest byte.
pub fn python_round(value: f64, ndigits: usize) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let mut text = String::new();
    if write!(text, "{value:.EXACT_FRACTION_DIGITS$}").is_err() {
        return value;
    }
    let negative = text.starts_with('-');
    let body = text.get(usize::from(negative)..).unwrap_or_default();
    let Some((integer, fraction)) = body.split_once('.') else {
        return value;
    };
    let kept = fraction.get(..ndigits).unwrap_or(fraction);
    let deciding = fraction.as_bytes().get(ndigits).copied();
    let rest_nonzero = fraction
        .get(ndigits.saturating_add(1)..)
        .is_some_and(|rest| rest.bytes().any(|byte| byte != b'0'));

    let mut digits: Vec<u8> = integer.bytes().chain(kept.bytes()).collect();
    let last_is_odd = digits.last().is_some_and(|byte| (byte - b'0') % 2 == 1);
    let round_up = match deciding {
        Some(byte) if byte > b'5' => true,
        Some(b'5') => rest_nonzero || last_is_odd,
        _ => false,
    };

    let mut integer_len = integer.len();
    if round_up {
        let mut index = digits.len();
        loop {
            if index == 0 {
                digits.insert(0, b'1');
                integer_len = integer_len.saturating_add(1);
                break;
            }
            index -= 1;
            match digits.get_mut(index) {
                Some(digit) if *digit == b'9' => *digit = b'0',
                Some(digit) => {
                    *digit += 1;
                    break;
                }
                None => break,
            }
        }
    }

    let mut rounded = String::with_capacity(digits.len() + 2);
    if negative {
        rounded.push('-');
    }
    let integer_digits = digits.get(..integer_len).unwrap_or_default();
    let fraction_digits = digits.get(integer_len..).unwrap_or_default();
    rounded.push_str(&String::from_utf8_lossy(integer_digits));
    if !fraction_digits.is_empty() {
        rounded.push('.');
        rounded.push_str(&String::from_utf8_lossy(fraction_digits));
    }
    rounded.parse::<f64>().unwrap_or(value)
}

/// CPython's single-argument `round()`: half to even, to an integer.
pub fn python_round_to_i64(value: f64) -> Option<i64> {
    let rounded = python_round(value, 0);
    if !rounded.is_finite() || rounded < -(2f64.powi(63)) || rounded >= 2f64.powi(63) {
        return None;
    }
    Some(rounded as i64)
}
