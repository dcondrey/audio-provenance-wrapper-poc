//! Untrusted-input hardening shared by the project parsers: a port of
//! `daemon/project_formats/_safe.py`.
//!
//! Every limit lives in [`Limits`] so tests can lower one and exercise
//! limit-1 / limit / limit+1 without building 64 MiB inputs, as the Python tests do.

use std::io::{Cursor, Read};
use std::path::Path;

use serde_json::Value;

use super::xml::{parse_xml as parse_xml_raw, Element, XmlLimits};

#[derive(Debug, Clone)]
pub struct Limits {
    pub max_project_file_bytes: u64,
    pub max_zip_members: usize,
    pub max_zip_total_bytes: u64,
    pub max_xml_bytes: usize,
    pub max_xml_depth: usize,
    pub max_xml_elements: usize,
    pub max_json_depth: usize,
    pub max_json_nodes: usize,
    pub max_pd_statements: usize,
    pub max_pd_canvas_depth: usize,
    pub max_pd_canvases: usize,
    pub max_maxpat_patchers: usize,
    pub max_dawproject_track_depth: usize,
    pub max_rpp_depth: usize,
    pub max_rpp_nodes: usize,
    pub max_rpp_lines: usize,
    pub max_als_decompressed_bytes: u64,
    /// The `.als` reader has no depth cap in Python; this bounds native recursion.
    pub max_als_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_project_file_bytes: 64 * 1024 * 1024,
            max_zip_members: 10_000,
            max_zip_total_bytes: 256 * 1024 * 1024,
            max_xml_bytes: 256 * 1024 * 1024,
            max_xml_depth: 128,
            max_xml_elements: 2_000_000,
            max_json_depth: 128,
            max_json_nodes: 2_000_000,
            max_pd_statements: 1_000_000,
            max_pd_canvas_depth: 64,
            max_pd_canvases: 100_000,
            max_maxpat_patchers: 100_000,
            max_dawproject_track_depth: 32,
            max_rpp_depth: 64,
            max_rpp_nodes: 500_000,
            max_rpp_lines: 5_000_000,
            max_als_decompressed_bytes: 256 * 1024 * 1024,
            max_als_depth: 1024,
        }
    }
}

/// Python `str.isspace` for the characters `str.strip()` removes.
pub fn is_python_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

pub type Result<T> = core::result::Result<T, String>;

/// Read a whole project file, refusing anything over the size cap.
pub fn read_project_bytes(path: &Path, limits: &Limits) -> Result<Vec<u8>> {
    let limit = limits.max_project_file_bytes;
    let size = std::fs::metadata(path).map_err(|error| format!("cannot stat {}: {error}", path.display()))?.len();
    if size > limit {
        return Err(format!("{} is {size} bytes, past {limit}; refusing to parse", path.display()));
    }
    let file = std::fs::File::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut data = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if data.len() as u64 > limit {
        return Err(format!("{} grew past {limit} bytes; refusing to parse", path.display()));
    }
    Ok(data)
}

/// Reject absolute, drive-lettered, NUL-bearing and parent-traversing archive names.
pub fn check_member_name(name: &str) -> Result<()> {
    let normal = name.replace('\\', "/");
    let bytes = normal.as_bytes();
    let drive = bytes.len() >= 2 && bytes.first().is_some_and(u8::is_ascii_alphabetic) && bytes.get(1) == Some(&b':');
    if normal.is_empty() || normal.contains('\0') || normal.starts_with('/') || drive {
        return Err(format!("unsafe archive member name: {}", py_repr(name)));
    }
    if normal.split('/').any(|part| part == "..") {
        return Err(format!("unsafe archive member name: {}", py_repr(name)));
    }
    Ok(())
}

fn py_repr(text: &str) -> String {
    apw_core::python_repr(&Value::String(text.to_owned()))
}

pub struct Archive<'a> {
    inner: zip::ZipArchive<Cursor<&'a [u8]>>,
    pub names: Vec<String>,
}

/// Member names as written in the central directory, duplicates included. The zip
/// reader keeps one entry per name, so repeated names are only visible here.
/// `None` when the directory cannot be walked (zip64, damage); the reader's own
/// validation then stands alone.
fn central_directory_names(data: &[u8]) -> Option<Vec<Vec<u8>>> {
    let u16_at = |offset: usize| data.get(offset..offset + 2).and_then(|b| <[u8; 2]>::try_from(b).ok()).map(|b| usize::from(u16::from_le_bytes(b)));
    let u32_at = |offset: usize| data.get(offset..offset + 4).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(|b| u32::from_le_bytes(b) as usize);
    let start = data.len().saturating_sub(22 + 65_535);
    let eocd = (start..=data.len().checked_sub(22)?).rev().find(|&i| data.get(i..i + 4) == Some(&[0x50, 0x4b, 0x05, 0x06]))?;
    let count = u16_at(eocd + 10)?;
    let mut offset = u32_at(eocd + 16)?;
    if count == 0xFFFF || offset == 0xFFFF_FFFF {
        return None;
    }
    let mut names = Vec::new();
    for _ in 0..count {
        if data.get(offset..offset + 4) != Some(&[0x50, 0x4b, 0x01, 0x02]) {
            return None;
        }
        let (name_len, extra_len, comment_len) = (u16_at(offset + 28)?, u16_at(offset + 30)?, u16_at(offset + 32)?);
        names.push(data.get(offset + 46..offset + 46 + name_len)?.to_vec());
        offset += 46 + name_len + extra_len + comment_len;
    }
    Some(names)
}

/// Open an in-memory zip after validating member count, names and declared sizes.
pub fn open_zip<'a>(data: &'a [u8], limits: &Limits) -> Result<Archive<'a>> {
    let mut inner = zip::ZipArchive::new(Cursor::new(data)).map_err(|error| format!("not a valid zip archive: {error}"))?;
    if inner.len() > limits.max_zip_members {
        return Err(format!(
            "zip has {} members, past {}; refusing to parse",
            inner.len(),
            limits.max_zip_members
        ));
    }
    if let Some(raw_names) = central_directory_names(data) {
        if raw_names.len() > limits.max_zip_members {
            return Err(format!("zip has {} members, past {}; refusing to parse", raw_names.len(), limits.max_zip_members));
        }
        let mut seen: Vec<&Vec<u8>> = Vec::new();
        for raw in &raw_names {
            let name = String::from_utf8_lossy(raw).into_owned();
            check_member_name(&name)?;
            if seen.contains(&raw) {
                return Err(format!("duplicate archive member: {}", py_repr(&name)));
            }
            seen.push(raw);
        }
    }
    let mut names: Vec<String> = Vec::new();
    let mut total: u64 = 0;
    for index in 0..inner.len() {
        let entry = inner
            .by_index_raw(index)
            .map_err(|error| format!("not a valid zip archive: {error}"))?;
        let name = entry.name().to_owned();
        check_member_name(&name)?;
        if names.contains(&name) {
            return Err(format!("duplicate archive member: {}", py_repr(&name)));
        }
        total = total.saturating_add(entry.size());
        names.push(name);
    }
    if total > limits.max_zip_total_bytes {
        return Err(format!(
            "zip declares {total} decompressed bytes, past {}; refusing to parse",
            limits.max_zip_total_bytes
        ));
    }
    Ok(Archive { inner, names })
}

impl Archive<'_> {
    /// Read one member through a capped stream; the declared size is never trusted.
    pub fn read_member(&mut self, name: &str, cap: u64) -> Result<Vec<u8>> {
        let index = self
            .names
            .iter()
            .position(|candidate| candidate == name)
            .ok_or_else(|| format!("archive has no member {}", py_repr(name)))?;
        let entry = self
            .inner
            .by_index(index)
            .map_err(|error| format!("cannot read archive member {}: {error}", py_repr(name)))?;
        let mut data = Vec::new();
        entry
            .take(cap + 1)
            .read_to_end(&mut data)
            .map_err(|error| format!("cannot read archive member {}: {error}", py_repr(name)))?;
        if data.len() as u64 > cap {
            return Err(format!(
                "archive member {} decompresses past {cap} bytes; refusing to parse",
                py_repr(name)
            ));
        }
        Ok(data)
    }
}

/// Decompress a zlib stream, stopping as soon as output would pass the limit.
pub fn inflate_zlib(data: &[u8], cap: u64) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .take(cap + 1)
        .read_to_end(&mut out)
        .map_err(|error| format!("invalid zlib data: {error}"))?;
    if out.len() as u64 > cap {
        return Err(format!("data decompresses past {cap} bytes; refusing to parse"));
    }
    Ok(out)
}

/// XML with no DTD/entity support and depth and element caps. `allowed_doctype`
/// names one bodyless root doctype (LMMS writes `<!DOCTYPE lmms-project>`).
pub fn parse_xml(data: &[u8], allowed_doctype: Option<&str>, limits: &Limits) -> Result<Element> {
    parse_xml_raw(
        data,
        allowed_doctype,
        XmlLimits {
            max_bytes: limits.max_xml_bytes,
            max_depth: limits.max_xml_depth,
            max_elements: limits.max_xml_elements,
            et_mode: false,
        },
    )
}

/// JSON with bounded depth and node count.
pub fn parse_json(data: &[u8], limits: &Limits) -> Result<Value> {
    if data.len() as u64 > limits.max_project_file_bytes {
        return Err("JSON exceeds the project size cap; refusing to parse".to_owned());
    }
    let data = data.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(data);
    let text = core::str::from_utf8(data).map_err(|error| format!("malformed JSON: {error}"))?;
    // serde_json's own recursion limit is 127 levels, one short of Python's 128, so
    // nesting is bounded here first and the parser's limit is then lifted.
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in text.bytes() {
        if in_string {
            match (escaped, byte) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                if depth > limits.max_json_depth {
                    return Err(format!("JSON nesting deeper than {}; refusing to parse", limits.max_json_depth));
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(text);
    deserializer.disable_recursion_limit();
    let value = <Value as serde::Deserialize>::deserialize(&mut deserializer)
        .and_then(|value| deserializer.end().map(|()| value))
        .map_err(|error| format!("malformed JSON: {error}"))?;
    check_json_bounds(&value, limits)?;
    Ok(value)
}

pub fn check_json_bounds(value: &Value, limits: &Limits) -> Result<()> {
    let mut nodes = 0_usize;
    let mut stack: Vec<(&Value, usize)> = vec![(value, 1)];
    while let Some((item, depth)) = stack.pop() {
        nodes += 1;
        if nodes > limits.max_json_nodes {
            return Err(format!("JSON has more than {} nodes; refusing to parse", limits.max_json_nodes));
        }
        if matches!(item, Value::Object(_) | Value::Array(_)) && depth > limits.max_json_depth {
            return Err(format!("JSON nesting deeper than {}; refusing to parse", limits.max_json_depth));
        }
        match item {
            Value::Object(map) => stack.extend(map.values().map(|child| (child, depth + 1))),
            Value::Array(items) => stack.extend(items.iter().map(|child| (child, depth + 1))),
            _ => {}
        }
    }
    Ok(())
}
