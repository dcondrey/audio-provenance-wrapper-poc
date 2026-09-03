//! The hostile-input boundary. Every byte of a trust document is read through here.
//!
//! Nothing in this crate touches a `serde_json::Value` directly: a reader is constructed against an
//! exact key set, so an unknown key, a missing key, a wrong type, an over-long string or a
//! control character is refused before any field is used.

use serde_json::{Map, Value};

use crate::error::TrustError;
use crate::time::Instant;

pub(crate) struct Reader<'a> {
    what: &'static str,
    object: &'a Map<String, Value>,
}

impl<'a> Reader<'a> {
    /// `allowed` is the complete key set. Extra keys and missing keys are both refusals: a document
    /// whose reader accepted an unknown key would have that key outside the signed payload.
    pub(crate) fn new(
        value: &'a Value,
        what: &'static str,
        allowed: &[&str],
    ) -> Result<Self, TrustError> {
        let Value::Object(object) = value else {
            return Err(TrustError::Malformed {
                what,
                reason: "expected a JSON object".to_string(),
            });
        };
        for key in object.keys() {
            if !allowed.contains(&key.as_str()) {
                return Err(TrustError::Malformed {
                    what,
                    reason: format!("unknown field {}", quoted(key)),
                });
            }
        }
        for key in allowed {
            if !object.contains_key(*key) {
                return Err(TrustError::Malformed {
                    what,
                    reason: format!("missing field {key:?}"),
                });
            }
        }
        Ok(Self { what, object })
    }

    fn raw(&self, field: &'static str) -> Result<&'a Value, TrustError> {
        self.object.get(field).ok_or(TrustError::Malformed {
            what: self.what,
            reason: format!("missing field {field:?}"),
        })
    }

    fn raw_str(&self, field: &'static str) -> Result<&'a str, TrustError> {
        self.raw(field)?.as_str().ok_or(TrustError::Malformed {
            what: self.what,
            reason: format!("field {field:?} is not a string"),
        })
    }

    /// A bounded, non-empty, control-character-free string.
    pub(crate) fn text(&self, field: &'static str, limit: usize) -> Result<String, TrustError> {
        let value = self.raw_str(field)?;
        if value.is_empty() {
            return Err(TrustError::FieldEmpty { field });
        }
        if value.len() > limit {
            return Err(TrustError::FieldTooLong {
                field,
                limit,
                found: value.len(),
            });
        }
        if value.chars().any(is_deceptive) || value.starts_with(' ') || value.ends_with(' ') {
            return Err(TrustError::UnprintableText { field });
        }
        Ok(value.to_string())
    }

    /// An identifier that is safe to interpolate into a path, a log line or a terminal.
    pub(crate) fn identifier(
        &self,
        field: &'static str,
        limit: usize,
    ) -> Result<String, TrustError> {
        let value = self.text(field, limit)?;
        if !value.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
        }) || value.starts_with('.')
        {
            return Err(TrustError::UnprintableText { field });
        }
        Ok(value)
    }

    pub(crate) fn hex<const N: usize>(&self, field: &'static str) -> Result<[u8; N], TrustError> {
        let value = self.raw_str(field)?;
        let invalid = TrustError::Hex {
            field,
            expected_bytes: N,
        };
        if value.len() != N * 2 || value.bytes().any(|b| b.is_ascii_uppercase()) {
            return Err(invalid);
        }
        let decoded = hex::decode(value).map_err(|_| TrustError::Hex {
            field,
            expected_bytes: N,
        })?;
        decoded.try_into().map_err(|_| TrustError::Hex {
            field,
            expected_bytes: N,
        })
    }

    pub(crate) fn instant(&self, field: &'static str) -> Result<Instant, TrustError> {
        Instant::parse(self.raw_str(field)?)
    }

    pub(crate) fn integer(
        &self,
        field: &'static str,
        range: core::ops::RangeInclusive<u64>,
    ) -> Result<u64, TrustError> {
        let value = self.raw(field)?.as_u64().ok_or(TrustError::Malformed {
            what: self.what,
            reason: format!("field {field:?} is not a non-negative integer"),
        })?;
        if !range.contains(&value) {
            return Err(TrustError::Malformed {
                what: self.what,
                reason: format!(
                    "field {field:?} is {value}, outside {}..={}",
                    range.start(),
                    range.end()
                ),
            });
        }
        Ok(value)
    }

    pub(crate) fn array(
        &self,
        field: &'static str,
        limit: usize,
    ) -> Result<&'a Vec<Value>, TrustError> {
        let items = self.raw(field)?.as_array().ok_or(TrustError::Malformed {
            what: self.what,
            reason: format!("field {field:?} is not an array"),
        })?;
        if items.len() > limit {
            return Err(TrustError::TooMany {
                what: field,
                limit,
                found: items.len(),
            });
        }
        Ok(items)
    }

    /// Refuses any `type` other than the one expected, which is what stops a signed anchor from
    /// being replayed where a signer record is read.
    pub(crate) fn expect_type(&self, expected: &'static str) -> Result<(), TrustError> {
        let found = self.raw_str("type")?;
        if found != expected {
            return Err(TrustError::UnexpectedType {
                expected,
                found: found.chars().take(64).collect(),
            });
        }
        Ok(())
    }
}

/// A character that makes displayed text differ from the text that was signed.
///
/// IMPORTANT: `char::is_control` covers C0/C1 and DEL only. The bidirectional overrides and the
/// zero-width joiners are format characters, not control characters, and U+202E alone lets a
/// display name render as a different studio's than the one the anchor vouched for. A vouched name
/// is the entire product of this crate, so both classes are refused at the parse boundary rather
/// than left to whatever renders the result.
fn is_deceptive(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}')
}

fn quoted(key: &str) -> String {
    format!("{:?}", key.chars().take(48).collect::<String>())
}
