//! Bounded Zstandard and ustar/pax readers for `.vcv` archives: a port of
//! `daemon/project_formats/_tarzst.py`, rule for rule. The shared corpus
//! `tests/fixtures/parity/project_modules_corpus.json` is what keeps them equal.
//!
//! A `.vcv` is exactly one Zstandard frame that consumes the whole input. The
//! frame is walked here first so that truncation, trailing bytes, dictionaries,
//! reserved bits and oversized windows are refused identically whichever decoder
//! follows; `ruzstd` (decode only, pure Rust) then inflates it under a hard cap.

use std::collections::HashSet;
use std::io::Read;

use super::safe::{check_member_name, py_repr, Limits, Result};

pub const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const ZSTD_MAX_WINDOW: u64 = 1 << 27;
const ZSTD_MAX_BLOCK: u64 = 128 * 1024;

const BLOCK: usize = 512;
const MAX_PAX_BYTES: usize = 64 * 1024;
const MAX_PAX_DIGITS: usize = 10;
const MAX_PAX_SIZE_DIGITS: usize = 15;

fn oversize_message(cap: usize) -> String {
    format!("Zstandard data is invalid or decompresses past {cap} bytes; refusing to parse")
}

fn truncated_frame() -> String {
    "truncated Zstandard frame".to_owned()
}

fn le(data: &[u8], start: usize, len: usize) -> u64 {
    data.get(start..start + len)
        .map_or(0, |bytes| bytes.iter().rev().fold(0_u64, |acc, byte| (acc << 8) | u64::from(*byte)))
}

/// Validate one whole Zstandard frame; return its declared content size.
fn frame_walk(data: &[u8]) -> Result<(Option<u64>, bool)> {
    if data.get(..4) != Some(ZSTD_MAGIC.as_slice()) {
        return Err("not a Zstandard frame".to_owned());
    }
    if data.len() < 6 {
        return Err(truncated_frame());
    }
    let descriptor = data.get(4).copied().unwrap_or(0);
    let fcs_flag = descriptor >> 6;
    let single_segment = descriptor & 0x20 != 0;
    if descriptor & 0x08 != 0 || descriptor & 0x03 != 0 {
        return Err("unsupported Zstandard frame: reserved bit or dictionary".to_owned());
    }
    let has_checksum = descriptor & 0x04 != 0;
    let mut pos = 5_usize;
    let mut window_descriptor = None;
    if !single_segment {
        window_descriptor = data.get(pos).copied();
        pos += 1;
    }
    let fcs_size: usize = match (fcs_flag, single_segment) {
        (0, true) => 1,
        (0, false) => 0,
        (1, _) => 2,
        (2, _) => 4,
        _ => 8,
    };
    if pos + fcs_size > data.len() {
        return Err(truncated_frame());
    }
    let mut content_size = None;
    if fcs_size > 0 {
        let mut size = le(data, pos, fcs_size);
        if fcs_flag == 1 {
            size += 256;
        }
        content_size = Some(size);
    }
    pos += fcs_size;
    let window = match window_descriptor {
        Some(descriptor) => {
            let base = 1_u64 << (10 + u32::from(descriptor >> 3));
            base + (base >> 3) * u64::from(descriptor & 7)
        }
        None => content_size.unwrap_or(0),
    };
    if window > ZSTD_MAX_WINDOW {
        return Err("unsupported Zstandard frame: window past 128 MiB".to_owned());
    }
    let block_max = if window > 0 { window.min(ZSTD_MAX_BLOCK) } else { ZSTD_MAX_BLOCK };
    loop {
        if pos + 3 > data.len() {
            return Err(truncated_frame());
        }
        let header = le(data, pos, 3);
        pos += 3;
        let (last, kind, size) = (header & 1, (header >> 1) & 3, header >> 3);
        if kind == 3 {
            return Err("unsupported Zstandard frame: reserved block type".to_owned());
        }
        if size > block_max {
            return Err("unsupported Zstandard frame: block past the maximum block size".to_owned());
        }
        pos += if kind == 1 { 1 } else { size as usize };
        if pos > data.len() {
            return Err(truncated_frame());
        }
        if last == 1 {
            break;
        }
    }
    if has_checksum {
        pos += 4;
        if pos > data.len() {
            return Err(truncated_frame());
        }
    }
    if pos != data.len() {
        return Err("trailing data after the Zstandard frame".to_owned());
    }
    Ok((content_size, has_checksum))
}

const P1: u64 = 11_400_714_785_074_694_791;
const P2: u64 = 14_029_467_366_897_019_727;
const P3: u64 = 1_609_587_929_392_839_161;
const P4: u64 = 9_650_029_242_287_828_579;
const P5: u64 = 2_870_177_450_012_600_261;

fn xx_round(acc: u64, input: u64) -> u64 {
    acc.wrapping_add(input.wrapping_mul(P2)).rotate_left(31).wrapping_mul(P1)
}

fn xx_merge(acc: u64, value: u64) -> u64 {
    (acc ^ xx_round(0, value)).wrapping_mul(P1).wrapping_add(P4)
}

/// XXH64 with seed 0: the Zstandard content checksum keeps its low 32 bits. `ruzstd`'s
/// streaming decoder does not verify it, libzstd does, so it is verified here.
fn xxh64(data: &[u8]) -> u64 {
    let mut hash;
    let mut rest = data;
    if data.len() >= 32 {
        let mut lanes = [P1.wrapping_add(P2), P2, 0, 0_u64.wrapping_sub(P1)];
        let mut stripes = data.chunks_exact(32);
        for stripe in &mut stripes {
            for (index, lane) in lanes.iter_mut().enumerate() {
                *lane = xx_round(*lane, le(stripe, index * 8, 8));
            }
        }
        rest = stripes.remainder();
        hash = lanes[0].rotate_left(1).wrapping_add(lanes[1].rotate_left(7)).wrapping_add(lanes[2].rotate_left(12)).wrapping_add(lanes[3].rotate_left(18));
        for lane in lanes {
            hash = xx_merge(hash, lane);
        }
    } else {
        hash = P5;
    }
    hash = hash.wrapping_add(data.len() as u64);
    while rest.len() >= 8 {
        hash = (hash ^ xx_round(0, le(rest, 0, 8))).rotate_left(27).wrapping_mul(P1).wrapping_add(P4);
        rest = rest.get(8..).unwrap_or_default();
    }
    if rest.len() >= 4 {
        hash = (hash ^ le(rest, 0, 4).wrapping_mul(P1)).rotate_left(23).wrapping_mul(P2).wrapping_add(P3);
        rest = rest.get(4..).unwrap_or_default();
    }
    for byte in rest {
        hash = (hash ^ u64::from(*byte).wrapping_mul(P5)).rotate_left(11).wrapping_mul(P1);
    }
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(P2);
    hash ^= hash >> 29;
    hash = hash.wrapping_mul(P3);
    hash ^ (hash >> 32)
}

/// Decompress one Zstandard frame, refusing output past `cap`.
pub fn inflate_zstd(data: &[u8], cap: usize) -> Result<Vec<u8>> {
    let (content_size, has_checksum) = frame_walk(data)?;
    if content_size.is_some_and(|size| size > cap as u64) {
        return Err(oversize_message(cap));
    }
    let mut source = data;
    let decoder = ruzstd::decoding::StreamingDecoder::new(&mut source).map_err(|_| oversize_message(cap))?;
    let mut out = Vec::new();
    decoder
        .take(cap as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|_| oversize_message(cap))?;
    if out.len() > cap {
        return Err(oversize_message(cap));
    }
    if has_checksum && le(data, data.len() - 4, 4) != xxh64(&out) & 0xFFFF_FFFF {
        return Err(oversize_message(cap));
    }
    if content_size.is_some_and(|size| size != out.len() as u64) {
        return Err(oversize_message(cap));
    }
    Ok(out)
}

fn strip_nul_space(field: &[u8]) -> &[u8] {
    let is_pad = |byte: &u8| *byte == 0 || *byte == b' ';
    let start = field.iter().position(|byte| !is_pad(byte)).unwrap_or(field.len());
    let end = field.iter().rposition(|byte| !is_pad(byte)).map_or(start, |index| index + 1);
    field.get(start..end).unwrap_or_default()
}

fn octal(field: &[u8]) -> Result<u64> {
    if field.first().is_some_and(|byte| byte & 0x80 != 0) {
        return Err("unsupported tar numeric encoding".to_owned());
    }
    let digits = strip_nul_space(field);
    if digits.is_empty() || digits.iter().any(|byte| !(b'0'..=b'7').contains(byte)) {
        return Err("malformed tar number".to_owned());
    }
    Ok(digits.iter().fold(0_u64, |acc, byte| (acc << 3) | u64::from(byte - b'0')))
}

fn cstring(field: &[u8]) -> &[u8] {
    field.split(|byte| *byte == 0).next().unwrap_or_default()
}

fn decode_name(raw: &[u8]) -> Result<String> {
    String::from_utf8(raw.to_vec()).map_err(|_| "tar entry name is not valid UTF-8".to_owned())
}

type Pax = Vec<(Vec<u8>, Vec<u8>)>;

fn pax_get<'a>(pax: &'a Pax, key: &[u8]) -> Option<&'a [u8]> {
    pax.iter().rev().find(|(name, _)| name == key).map(|(_, value)| value.as_slice())
}

fn malformed_pax() -> String {
    "malformed pax record".to_owned()
}

fn parse_pax(buf: &[u8]) -> Result<Pax> {
    let mut records: Pax = Vec::new();
    let mut index = 0_usize;
    while index < buf.len() {
        let rest = buf.get(index..).unwrap_or_default();
        let space = rest.iter().position(|byte| *byte == b' ').ok_or_else(malformed_pax)?;
        let digits = rest.get(..space).unwrap_or_default();
        if space == 0 || digits.len() > MAX_PAX_DIGITS || digits.iter().any(|byte| !byte.is_ascii_digit()) {
            return Err(malformed_pax());
        }
        let length = digits.iter().fold(0_usize, |acc, byte| acc * 10 + usize::from(byte - b'0'));
        if length < digits.len() + 3 || index + length > buf.len() {
            return Err(malformed_pax());
        }
        let record = rest.get(space + 1..length).unwrap_or_default();
        let record = record.strip_suffix(b"\n").ok_or_else(malformed_pax)?;
        let equals = record.iter().position(|byte| *byte == b'=').ok_or_else(malformed_pax)?;
        if equals < 1 {
            return Err(malformed_pax());
        }
        records.push((
            record.get(..equals).unwrap_or_default().to_vec(),
            record.get(equals + 1..).unwrap_or_default().to_vec(),
        ));
        index += length;
    }
    Ok(records)
}

fn normalise(name: &str) -> Result<String> {
    check_member_name(name)?;
    let joined = name
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/");
    Ok(joined)
}

fn padded(size: usize) -> usize {
    size.div_ceil(BLOCK) * BLOCK
}

/// A regular file of a validated tar stream: name, data offset, data size.
pub type TarMember = (String, usize, usize);

/// Validate a whole tar stream; return every regular file.
pub fn read_tar(data: &[u8], limits: &Limits) -> Result<Vec<TarMember>> {
    let mut members: Vec<TarMember> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut pax: Option<Pax> = None;
    let mut entries = 0_usize;
    let mut pos = 0_usize;
    while pos != data.len() {
        let Some(block) = data.get(pos..pos + BLOCK) else {
            return Err("truncated tar header".to_owned());
        };
        if block.iter().all(|byte| *byte == 0) {
            break;
        }
        let checksum: u64 = block
            .iter()
            .enumerate()
            .map(|(index, byte)| if (148..156).contains(&index) { 32 } else { u64::from(*byte) })
            .sum();
        if octal(block.get(148..156).unwrap_or_default())? != checksum {
            return Err("tar header checksum mismatch".to_owned());
        }
        if block.get(257..262) != Some(b"ustar".as_slice()) {
            return Err("unsupported tar format: missing ustar magic".to_owned());
        }
        let mut size = octal(block.get(124..136).unwrap_or_default())?;
        let kind = block.get(156).copied().unwrap_or(0);
        let start = pos + BLOCK;
        if kind == 0x78 {
            if pax.is_some() {
                return Err("tar has consecutive pax headers".to_owned());
            }
            if size > MAX_PAX_BYTES as u64 || start as u64 + size > data.len() as u64 {
                return Err("truncated or oversized pax header".to_owned());
            }
            let size = size as usize;
            pax = Some(parse_pax(data.get(start..start + size).unwrap_or_default())?);
            pos = start + padded(size);
            continue;
        }
        if !matches!(kind, 0x30 | 0x00 | 0x35) {
            return Err(format!("unsupported tar entry type {kind}"));
        }
        let mut name_bytes = cstring(block.get(0..100).unwrap_or_default()).to_vec();
        let prefix = cstring(block.get(345..500).unwrap_or_default());
        if !prefix.is_empty() {
            let mut joined = prefix.to_vec();
            joined.push(b'/');
            joined.extend(&name_bytes);
            name_bytes = joined;
        }
        if let Some(records) = pax.take() {
            if let Some(path) = pax_get(&records, b"path") {
                name_bytes = path.to_vec();
            }
            if let Some(digits) = pax_get(&records, b"size") {
                if digits.is_empty() || digits.len() > MAX_PAX_SIZE_DIGITS || digits.iter().any(|byte| !byte.is_ascii_digit()) {
                    return Err("malformed pax size".to_owned());
                }
                size = digits.iter().fold(0_u64, |acc, byte| acc * 10 + u64::from(byte - b'0'));
            }
        }
        let name = normalise(&decode_name(&name_bytes)?)?;
        entries += 1;
        if entries > limits.max_tar_members {
            return Err(format!("tar has more than {} members; refusing to parse", limits.max_tar_members));
        }
        if kind == 0x35 {
            if size != 0 {
                return Err("tar directory entry has data".to_owned());
            }
        } else {
            if name.is_empty() {
                return Err("tar file entry has no name".to_owned());
            }
            if seen.contains(&name) {
                return Err(format!("duplicate archive member: {}", py_repr(&name)));
            }
            if start as u64 + size > data.len() as u64 {
                return Err("truncated tar entry".to_owned());
            }
            seen.insert(name.clone());
            members.push((name, start, size as usize));
        }
        pos = start + padded(size as usize);
    }
    if pax.is_some() {
        return Err("tar ends after a pax header".to_owned());
    }
    Ok(members)
}
