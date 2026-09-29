//! Host identification for hosts the plug-in wrapper does not recognise: a port of
//! `daemon/host_identity/__init__.py` reading the same `data/host_executables.json`.
//!
//! The table is embedded from a copy inside this crate
//! (`data/host_executables.json`); `tests/host_identity_parity.rs` proves the copy
//! is byte-identical to the repository's `data/` file.
//!
//! Rules (docs/HOST_IDENTITY.md): a wrapper-recognised host is never overridden; the
//! executable name is compared for exact, case-insensitive equality, one trailing
//! `.exe` ignored; a hit is `inferred`, never `directly_observed`.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use apw_core::{python_eq, python_repr};
use serde_json::Value;

pub const SCHEMA_VERSION: u64 = 1;
pub const MAX_TABLE_BYTES: usize = 256 * 1024;
const MAX_HOSTS: usize = 512;
const MAX_MATCHES_PER_HOST: usize = 32;
const MAX_STRING: usize = 512;
const PLATFORMS: [&str; 3] = ["windows", "macos", "linux"];

pub const IDENT_JUCE: &str = "juce_plugin_host_type";
pub const IDENT_INFERRED: &str = "inferred_from_executable_name";
pub const IDENT_NONE: &str = "unrecognised";

const EMBEDDED_TABLE: &str = include_str!("../data/host_executables.json");

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct HostTableError(pub String);

fn fail<T>(message: impl Into<String>) -> Result<T, HostTableError> {
    Err(HostTableError(message.into()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRecord {
    pub host_id: String,
    pub display_name: String,
    pub source_url: String,
}

/// `{(platform, normalised name): record}` in table order.
#[derive(Debug, Clone, Default)]
pub struct HostTable {
    entries: Vec<((String, String), HostRecord)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostIdentity {
    pub recognised: bool,
    pub host_name: Option<String>,
    pub identification: &'static str,
    pub proof_level: &'static str,
    pub host_id: Option<String>,
    pub display_name: Option<String>,
    pub source_url: Option<String>,
}

/// Python `str.isspace` for the characters `str.strip()` removes: Unicode
/// White_Space plus U+001C..U+001F, which Python also treats as whitespace.
fn is_python_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Full case folding restricted to what can produce ASCII, which is all that
/// matters when the folded name is compared to an ASCII table entry. Characters
/// whose Python `casefold()` differs from `lower()` and folds into ASCII: the
/// sharp s, the long s and the f-ligatures (Kelvin sign lowercases to `k`).
fn casefold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{df}' | '\u{1e9e}' => out.push_str("ss"),
            '\u{17f}' => out.push('s'),
            '\u{fb00}' => out.push_str("ff"),
            '\u{fb01}' => out.push_str("fi"),
            '\u{fb02}' => out.push_str("fl"),
            '\u{fb03}' => out.push_str("ffi"),
            '\u{fb04}' => out.push_str("ffl"),
            '\u{fb05}' | '\u{fb06}' => out.push_str("st"),
            other => out.extend(other.to_lowercase()),
        }
    }
    out
}

/// Casefold and drop one trailing `.exe`. Nothing else is altered.
pub fn normalise_executable_name(name: &str) -> String {
    let folded = casefold(name.trim_matches(is_python_space));
    match folded.strip_suffix(".exe") {
        Some(stem) => stem.to_owned(),
        None => folded,
    }
}

fn text<'a>(value: Option<&'a Value>, wher: &str) -> Result<&'a str, HostTableError> {
    match value {
        Some(Value::String(text)) if !text.is_empty() && text.chars().count() <= MAX_STRING => Ok(text),
        _ => fail(format!(
            "{wher}: expected a non-empty string of at most {MAX_STRING} characters"
        )),
    }
}

/// Validate table bytes; the same checks and messages as `parse_table`.
pub fn parse_table(raw: &[u8]) -> Result<HostTable, HostTableError> {
    if raw.len() > MAX_TABLE_BYTES {
        return fail(format!("table exceeds {MAX_TABLE_BYTES} bytes"));
    }
    let text_raw = core::str::from_utf8(raw)
        .map_err(|error| HostTableError(format!("table is not valid UTF-8 JSON: {error}")))?;
    let doc: Value = serde_json::from_str(text_raw)
        .map_err(|error| HostTableError(format!("table is not valid UTF-8 JSON: {error}")))?;
    let Value::Object(doc) = doc else {
        return fail("table root must be an object");
    };
    let version_ok = doc
        .get("schema_version")
        .is_some_and(|version| !version.is_boolean() && python_eq(version, &Value::from(SCHEMA_VERSION)));
    if !version_ok {
        return fail(format!("unsupported schema_version, expected {SCHEMA_VERSION}"));
    }
    let Some(Value::Array(hosts)) = doc.get("hosts").filter(|hosts| hosts.as_array().is_some_and(|list| list.len() <= MAX_HOSTS))
    else {
        return fail("hosts must be a list within bounds");
    };

    let mut table = HostTable::default();
    let mut host_ids: Vec<&str> = Vec::new();
    for (index, host) in hosts.iter().enumerate() {
        let wher = format!("hosts[{index}]");
        let Value::Object(host) = host else {
            return fail(format!("{wher}: must be an object"));
        };
        let host_id = text(host.get("host_id"), &format!("{wher}.host_id"))?;
        if host_ids.contains(&host_id) {
            return fail(format!(
                "{wher}: duplicate host_id {}",
                python_repr(&Value::String(host_id.to_owned()))
            ));
        }
        host_ids.push(host_id);
        let display = text(host.get("display_name"), &format!("{wher}.display_name"))?;
        let source = text(host.get("source_url"), &format!("{wher}.source_url"))?;
        if !source.starts_with("https://") {
            return fail(format!("{wher}.source_url must be https"));
        }
        let Some(Value::Array(matches)) = host
            .get("match")
            .filter(|value| value.as_array().is_some_and(|list| !list.is_empty() && list.len() <= MAX_MATCHES_PER_HOST))
        else {
            return fail(format!("{wher}.match must be a non-empty list within bounds"));
        };
        for (position, entry) in matches.iter().enumerate() {
            let mwhere = format!("{wher}.match[{position}]");
            let Value::Object(entry) = entry else {
                return fail(format!("{mwhere}: must be an object"));
            };
            let platform = match entry.get("platform") {
                Some(Value::String(platform)) if PLATFORMS.contains(&platform.as_str()) => platform.clone(),
                _ => {
                    return fail(format!(
                        "{mwhere}.platform must be one of ('windows', 'macos', 'linux')"
                    ));
                }
            };
            let exe = text(entry.get("executable_name"), &format!("{mwhere}.executable_name"))?;
            let folded = casefold(exe);
            if exe.contains(['/', '\\']) || folded.ends_with(".exe") || folded.ends_with(".app") {
                return fail(format!(
                    "{mwhere}.executable_name must be a bare name without path, .exe or .app"
                ));
            }
            let key = (platform, normalise_executable_name(exe));
            if let Some((_, existing)) = table.entries.iter().find(|(candidate, _)| *candidate == key) {
                return fail(format!(
                    "{mwhere}: ({}, {}) is already claimed by {}",
                    python_repr(&Value::String(key.0.clone())),
                    python_repr(&Value::String(key.1.clone())),
                    python_repr(&Value::String(existing.host_id.clone()))
                ));
            }
            table.entries.push((
                key,
                HostRecord {
                    host_id: host_id.to_owned(),
                    display_name: display.to_owned(),
                    source_url: source.to_owned(),
                },
            ));
        }
    }
    Ok(table)
}

impl HostTable {
    /// `(platform, normalised name, record)` in table order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str, &HostRecord)> {
        self.entries
            .iter()
            .map(|((platform, name), record)| (platform.as_str(), name.as_str(), record))
    }
}

fn shipped_table() -> Result<&'static HostTable, HostTableError> {
    static TABLE: OnceLock<Result<HostTable, HostTableError>> = OnceLock::new();
    TABLE
        .get_or_init(|| parse_table(EMBEDDED_TABLE.as_bytes()))
        .as_ref()
        .map_err(Clone::clone)
}

/// The daemon's own platform in the table's vocabulary.
pub fn current_platform() -> Option<&'static str> {
    if cfg!(target_os = "windows") {
        Some("windows")
    } else if cfg!(target_os = "macos") {
        Some("macos")
    } else if cfg!(target_os = "linux") {
        Some("linux")
    } else {
        None
    }
}

fn unmatched() -> HostIdentity {
    HostIdentity {
        recognised: false,
        host_name: None,
        identification: IDENT_NONE,
        proof_level: "unknown_unobserved",
        host_id: None,
        display_name: None,
        source_url: None,
    }
}

/// Name the host. A wrapper-recognised host is returned unchanged. `platform`
/// `None` matches any platform and is ambiguous (unmatched) if two hosts claim
/// the name.
pub fn identify_host_in(
    table: &HostTable,
    executable_name: Option<&str>,
    juce_recognised: bool,
    juce_name: Option<&str>,
    platform: Option<&str>,
) -> HostIdentity {
    if let (true, Some(name)) = (juce_recognised, juce_name.filter(|name| !name.is_empty())) {
        return HostIdentity {
            recognised: true,
            host_name: Some(name.to_owned()),
            identification: IDENT_JUCE,
            proof_level: "directly_observed",
            host_id: None,
            display_name: None,
            source_url: None,
        };
    }
    let Some(executable_name) = executable_name.filter(|name| !name.is_empty()) else {
        return unmatched();
    };
    if platform.is_some_and(|platform| !PLATFORMS.contains(&platform)) {
        return unmatched();
    }
    let name = normalise_executable_name(executable_name);
    let mut hits: BTreeMap<&str, &HostRecord> = BTreeMap::new();
    for ((entry_platform, exe), record) in &table.entries {
        if *exe == name && platform.is_none_or(|wanted| wanted == entry_platform) {
            hits.insert(record.host_id.as_str(), record);
        }
    }
    if hits.len() != 1 {
        return unmatched();
    }
    let Some(record) = hits.values().next() else {
        return unmatched();
    };
    HostIdentity {
        recognised: true,
        host_name: Some(record.display_name.clone()),
        identification: IDENT_INFERRED,
        proof_level: "inferred",
        host_id: Some(record.host_id.clone()),
        display_name: Some(record.display_name.clone()),
        source_url: Some(record.source_url.clone()),
    }
}

/// [`identify_host_in`] against the embedded shipped table.
pub fn identify_host(
    executable_name: Option<&str>,
    juce_recognised: bool,
    juce_name: Option<&str>,
    platform: Option<&str>,
) -> Result<HostIdentity, HostTableError> {
    Ok(identify_host_in(shipped_table()?, executable_name, juce_recognised, juce_name, platform))
}
