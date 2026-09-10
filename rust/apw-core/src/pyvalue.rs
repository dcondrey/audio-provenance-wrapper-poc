use serde_json::{Map, Value};

use crate::canonical::python_repr_f64;

pub(crate) const NULL: Value = Value::Null;

pub(crate) fn get<'a>(map: &'a Map<String, Value>, key: &str) -> &'a Value {
    map.get(key).unwrap_or(&NULL)
}

/// Python truthiness: `None`, `False`, `0`, `""`, `[]` and `{}` are falsy.
pub fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// Python `==` over decoded JSON. IMPORTANT: `1 == 1.0` and `False == 0` are
/// true in Python but false under `serde_json::Value`'s derived `PartialEq`,
/// which compares the `PosInt`/`NegInt`/`Float` discriminant. Every counter
/// comparison in the schema validator goes through here so a manifest Python
/// accepts is not rejected in Rust.
pub fn python_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Bool(a), Value::Number(_)) => numeric_eq(&Value::from(u8::from(*a)), right),
        (Value::Number(_), Value::Bool(b)) => numeric_eq(left, &Value::from(u8::from(*b))),
        (Value::Number(_), Value::Number(_)) => numeric_eq(left, right),
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| python_eq(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, x)| b.get(key).is_some_and(|y| python_eq(x, y)))
        }
        _ => false,
    }
}

fn numeric_eq(left: &Value, right: &Value) -> bool {
    let (Value::Number(left), Value::Number(right)) = (left, right) else {
        return false;
    };
    match (left.as_i128(), right.as_i128()) {
        (Some(a), Some(b)) => a == b,
        _ => match (left.as_f64(), right.as_f64()) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        },
    }
}

pub fn is_string_equal(value: &Value, expected: &str) -> bool {
    matches!(value, Value::String(text) if text == expected)
}

/// Python `str()` of a decoded JSON value. Used by the claim-summary f-strings
/// and by the `len(str(export["sha256"]))` schema check, both of which are
/// byte-visible in the manifest.
pub fn python_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => python_repr(other),
    }
}

pub fn python_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(number) => {
            if let Some(unsigned) = number.as_u64() {
                unsigned.to_string()
            } else if let Some(signed) = number.as_i64() {
                signed.to_string()
            } else {
                number
                    .as_f64()
                    .and_then(|float| python_repr_f64(float).ok())
                    .unwrap_or_else(|| number.to_string())
            }
        }
        Value::String(text) => python_quote(text),
        Value::Array(items) => {
            let rendered: Vec<String> = items.iter().map(python_repr).collect();
            format!("[{}]", rendered.join(", "))
        }
        Value::Object(map) => {
            let rendered: Vec<String> = map
                .iter()
                .map(|(key, child)| format!("{}: {}", python_quote(key), python_repr(child)))
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
    }
}

fn python_quote(text: &str) -> String {
    if text.contains('\'') && !text.contains('"') {
        format!("\"{text}\"")
    } else {
        format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}
