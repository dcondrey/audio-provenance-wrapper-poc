//! Serialising a Rust result into the object JavaScript expects, and an error JavaScript can catch.

use audio_provenance_core::CodedError;
use serde::Serialize;
use wasm_bindgen::JsValue;

/// Every error crossing this boundary is a `AudioProvenanceError` carrying a stable `code`.
///
/// The code is the Rust type's own [`CodedError::code`], never a string invented here, so a JS
/// caller branching on `error.code` is branching on the same vocabulary the CLI exits with.
pub fn throw(code: &str, message: &str) -> JsValue {
    let error = js_sys::Error::new(message);
    error.set_name("AudioProvenanceError");
    // A failed property write would mean the JS engine refused a plain object; the Error still
    // carries the message, so degrade rather than lose the error entirely.
    let _ = js_sys::Reflect::set(&error, &JsValue::from_str("code"), &JsValue::from_str(code));
    error.into()
}

pub fn coded<E: CodedError + core::fmt::Display>(error: E) -> JsValue {
    throw(error.code(), &error.to_string())
}

pub fn invalid_option(option: &str, reason: &str) -> JsValue {
    throw(
        "option_invalid",
        &format!("option {option} is invalid: {reason}"),
    )
}

/// Serialises to JSON with every key rewritten from `snake_case` to `camelCase`.
///
/// The rewrite is a blind walk over the tree rather than a second hand-built shape, so a field
/// added to `VerifyResult` reaches JavaScript on the day it is added instead of on the day someone
/// remembers this file. No result type in the graph carries a map whose KEYS are data, which is the
/// one thing that would make a blind walk wrong.
pub fn to_camel_json<T: Serialize>(value: &T) -> Result<String, JsValue> {
    let mut tree = serde_json::to_value(value)
        .map_err(|error| throw("serialization_failed", &error.to_string()))?;
    camelize(&mut tree);
    serde_json::to_string(&tree).map_err(|error| throw("serialization_failed", &error.to_string()))
}

pub fn camelize(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            let mut renamed = serde_json::Map::with_capacity(map.len());
            for (key, mut entry) in core::mem::take(map) {
                camelize(&mut entry);
                renamed.insert(camel_case(&key), entry);
            }
            *map = renamed;
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(camelize),
        _ => {}
    }
}

fn camel_case(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut upper_next = false;
    for ch in key.chars() {
        if ch == '_' {
            upper_next = true;
            continue;
        }
        if upper_next {
            out.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::camel_case;

    #[test]
    fn snake_keys_become_camel_keys() {
        assert_eq!(camel_case("signed_at"), "signedAt");
        assert_eq!(
            camel_case("false_positive_rate_at_match"),
            "falsePositiveRateAtMatch"
        );
        assert_eq!(camel_case("match"), "match");
        assert_eq!(camel_case("content_sha256"), "contentSha256");
    }
}
