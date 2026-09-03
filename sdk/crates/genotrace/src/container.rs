//! Locating a manifest inside a container, by bytes.
//!
//! # What is implemented, and what is not
//!
//! RIFF (`aprv`), AIFF (`APRV`), FLAC APPLICATION (`aprv`) and ID3v2 `GEOB` are scanned. MP4 `uuid`
//! and Ogg's non-FLAC metadata packets are not: nothing in this workspace writes them, and a scan
//! that silently returned nothing would be indistinguishable from a scan that found nothing. The
//! rung reports which containers it did not search, so `not_found` stays readable as "not
//! recovered" rather than "no manifest exists".

use crate::ingest::Container;

/// The four-character provenance slot Audio Provenance writes into an IFF container.
pub const RIFF_CHUNK_ID: [u8; 4] = *b"aprv";
pub const AIFF_CHUNK_ID: [u8; 4] = *b"APRV";
/// The ID3v2 `GEOB` description that marks a Audio Provenance manifest object.
pub const GEOB_DESCRIPTION: &str = "audio-provenance-manifest";
/// The FLAC APPLICATION block id.
pub const FLAC_APPLICATION_ID: [u8; 4] = *b"aprv";

const MAX_EMBEDDED_BYTES: usize = audio_provenance_manifest::MAX_MANIFEST_BYTES;

/// Why a container carried no readable manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddedOutcome {
    Found(Vec<u8>),
    Absent,
    /// A provenance slot exists and is corrupt or truncated. Recorded and stepped over, never
    /// forced into a candidate.
    Unparseable(&'static str),
    /// This build does not scan this container's provenance slot.
    Unsearched(&'static str),
}

pub fn find_embedded(bytes: &[u8], container: Container) -> EmbeddedOutcome {
    match container {
        Container::Wav => riff_chunk(bytes, RIFF_CHUNK_ID),
        Container::Aiff => iff_chunk_be(bytes, AIFF_CHUNK_ID),
        Container::Flac => flac_application(bytes),
        Container::Mp3 => id3_geob(bytes),
        Container::Ogg => EmbeddedOutcome::Unsearched("ogg metadata packets are not scanned"),
        Container::Mp4 => EmbeddedOutcome::Unsearched("mp4 uuid boxes are not scanned"),
    }
}

fn payload(bytes: &[u8], start: usize, size: usize) -> EmbeddedOutcome {
    if size == 0 || size > MAX_EMBEDDED_BYTES {
        return EmbeddedOutcome::Unparseable("provenance slot length is out of range");
    }
    match start
        .checked_add(size)
        .and_then(|end| bytes.get(start..end))
    {
        Some(slice) => EmbeddedOutcome::Found(slice.to_vec()),
        None => EmbeddedOutcome::Unparseable("provenance slot runs past the end of the file"),
    }
}

fn riff_chunk(bytes: &[u8], id: [u8; 4]) -> EmbeddedOutcome {
    let mut offset = 12usize;
    while offset.saturating_add(8) <= bytes.len() {
        let Some(header) = bytes.get(offset..offset + 8) else {
            break;
        };
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        if header.get(..4) == Some(&id[..]) {
            return payload(bytes, offset + 8, size);
        }
        // Chunks are word-aligned, so the pad byte is part of the advance. The advance is at least
        // 8, which is what guarantees the walk terminates.
        let advance = 8usize
            .checked_add(size)
            .and_then(|n| n.checked_add(size & 1));
        offset = match advance.and_then(|advance| offset.checked_add(advance)) {
            Some(next) => next,
            None => return EmbeddedOutcome::Unparseable("RIFF chunk length overflows"),
        };
    }
    EmbeddedOutcome::Absent
}

fn iff_chunk_be(bytes: &[u8], id: [u8; 4]) -> EmbeddedOutcome {
    let mut offset = 12usize;
    while offset.saturating_add(8) <= bytes.len() {
        let Some(header) = bytes.get(offset..offset + 8) else {
            break;
        };
        let size = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
        if header.get(..4) == Some(&id[..]) {
            return payload(bytes, offset + 8, size);
        }
        let advance = 8usize
            .checked_add(size)
            .and_then(|n| n.checked_add(size & 1));
        offset = match advance.and_then(|advance| offset.checked_add(advance)) {
            Some(next) => next,
            None => return EmbeddedOutcome::Unparseable("AIFF chunk length overflows"),
        };
    }
    EmbeddedOutcome::Absent
}

fn flac_application(bytes: &[u8]) -> EmbeddedOutcome {
    let mut offset = 4usize;
    loop {
        let Some(header) = bytes.get(offset..offset + 4) else {
            return EmbeddedOutcome::Absent;
        };
        let last = header[0] & 0x80 != 0;
        let kind = header[0] & 0x7F;
        let size = ((u32::from(header[1]) << 16)
            | (u32::from(header[2]) << 8)
            | u32::from(header[3])) as usize;
        let body = offset + 4;
        // 2 is APPLICATION; the first four payload bytes are the registered application id, so a
        // block declaring fewer than four carries no id to read.
        if kind == 2 && size >= 4 {
            match bytes.get(body..body + 4) {
                Some(application) if application == FLAC_APPLICATION_ID => {
                    return payload(bytes, body + 4, size.saturating_sub(4));
                }
                Some(_) => {}
                None => return EmbeddedOutcome::Unparseable("FLAC APPLICATION block is truncated"),
            }
        }
        if last {
            return EmbeddedOutcome::Absent;
        }
        offset = match body.checked_add(size) {
            Some(next) if next > offset => next,
            _ => return EmbeddedOutcome::Unparseable("FLAC metadata block length overflows"),
        };
    }
}

/// ID3v2 `GEOB`: encoding byte, MIME (NUL-terminated), filename (NUL-terminated), description
/// (NUL-terminated), then the object.
fn id3_geob(bytes: &[u8]) -> EmbeddedOutcome {
    if !bytes.starts_with(b"ID3") {
        return EmbeddedOutcome::Absent;
    }
    let Some(header) = bytes.get(..10) else {
        return EmbeddedOutcome::Absent;
    };
    let major = header[3];
    if major < 3 {
        return EmbeddedOutcome::Unsearched("ID3v2.2 frame headers are not scanned");
    }
    let Some(size_bytes) = header.get(6..10) else {
        return EmbeddedOutcome::Absent;
    };
    if size_bytes.iter().any(|byte| byte & 0x80 != 0) {
        return EmbeddedOutcome::Unparseable("ID3 tag size is not synchsafe");
    }
    let tag_size = size_bytes
        .iter()
        .fold(0usize, |acc, byte| (acc << 7) | usize::from(*byte));
    let tag_end = match 10usize.checked_add(tag_size) {
        Some(end) if end <= bytes.len() => end,
        _ => return EmbeddedOutcome::Unparseable("ID3 tag runs past the end of the file"),
    };
    // An extended header, if present, is skipped by its own declared size.
    let mut offset = 10usize;
    if header[5] & 0x40 != 0 {
        let Some(extended) = bytes.get(offset..offset + 4) else {
            return EmbeddedOutcome::Unparseable("ID3 extended header is truncated");
        };
        let extended_size =
            u32::from_be_bytes([extended[0], extended[1], extended[2], extended[3]]) as usize;
        offset = match offset.checked_add(extended_size.max(4)) {
            Some(next) if next <= tag_end => next,
            _ => return EmbeddedOutcome::Unparseable("ID3 extended header length overflows"),
        };
    }

    while offset + 10 <= tag_end {
        let Some(frame) = bytes.get(offset..offset + 10) else {
            break;
        };
        if frame[..4] == [0, 0, 0, 0] {
            return EmbeddedOutcome::Absent;
        }
        let raw = u32::from_be_bytes([frame[4], frame[5], frame[6], frame[7]]);
        // v4 frame sizes are synchsafe; v3 sizes are plain big-endian.
        let size = if major >= 4 {
            let parts = [frame[4], frame[5], frame[6], frame[7]];
            if parts.iter().any(|byte| byte & 0x80 != 0) {
                return EmbeddedOutcome::Unparseable("ID3v2.4 frame size is not synchsafe");
            }
            parts
                .iter()
                .fold(0usize, |acc, byte| (acc << 7) | usize::from(*byte))
        } else {
            raw as usize
        };
        let body = offset + 10;
        let Some(end) = body.checked_add(size).filter(|end| *end <= tag_end) else {
            return EmbeddedOutcome::Unparseable("ID3 frame runs past the tag");
        };
        if &frame[..4] == b"GEOB"
            && let Some(found) = geob_object(bytes.get(body..end).unwrap_or(&[]))
        {
            return payload(&found, 0, found.len());
        }
        offset = end;
    }
    EmbeddedOutcome::Absent
}

fn geob_object(frame: &[u8]) -> Option<Vec<u8>> {
    let encoding = *frame.first()?;
    // Only ISO-8859-1 and UTF-8 descriptors are read; the UTF-16 forms use two-byte terminators and
    // nothing in this workspace writes them.
    if encoding != 0 && encoding != 3 {
        return None;
    }
    let mut cursor = 1usize;
    let mut fields = Vec::with_capacity(3);
    for _ in 0..3 {
        let start = cursor;
        let relative = frame.get(cursor..)?.iter().position(|byte| *byte == 0)?;
        cursor = start + relative + 1;
        fields.push(frame.get(start..start + relative)?);
    }
    let description = fields.get(2).copied().unwrap_or(&[]);
    if description != GEOB_DESCRIPTION.as_bytes() {
        return None;
    }
    Some(frame.get(cursor..)?.to_vec())
}

/// Why a provenance chunk could not be written.
///
/// Carries no owned data so that a caller can map it into its own error type without allocating,
/// and so that `reason` is the one place the wording lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkWriteError {
    NotRiffWave,
    PayloadOutOfRange,
    TruncatedChunkTable,
    ChunkPastEnd,
    ExceedsRiffLimit,
}

impl ChunkWriteError {
    pub const fn reason(self) -> &'static str {
        match self {
            Self::NotRiffWave => "not a RIFF/WAVE container",
            Self::PayloadOutOfRange => "manifest length is out of range",
            Self::TruncatedChunkTable => {
                "RIFF chunk table ends mid-header; refusing to rewrite a malformed file"
            }
            Self::ChunkPastEnd => "a RIFF chunk runs past the end of the file",
            Self::ExceedsRiffLimit => "the rewritten file would exceed the 4 GiB RIFF limit",
        }
    }
}

impl core::fmt::Display for ChunkWriteError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for ChunkWriteError {}

/// Returns a copy of `wav` carrying `payload` in a single trailing `aprv` chunk.
///
/// IMPORTANT: this REPLACES any existing `aprv` chunk. `riff_chunk` above returns the FIRST match,
/// so a writer that merely appended would publish a file whose recovered manifest is the one it was
/// meant to supersede. Writer and reader are two halves of one format and live together for that
/// reason; two independent implementations of this had already drifted to opposite behaviours.
///
/// Every other chunk is re-emitted with its word-alignment pad byte, so appending cannot misalign a
/// reader on a source whose final chunk omitted its pad. The `data` chunk is untouched, so the
/// decoded samples, and therefore `decoded_audio_sha256`, are bit-identical to the input's; the
/// file bytes do move, so `content_sha256` cannot recompute over the result.
pub fn write_riff_chunk(wav: &[u8], payload: &[u8]) -> Result<Vec<u8>, ChunkWriteError> {
    if payload.is_empty() || payload.len() > MAX_EMBEDDED_BYTES {
        return Err(ChunkWriteError::PayloadOutOfRange);
    }
    let payload_len =
        u32::try_from(payload.len()).map_err(|_| ChunkWriteError::PayloadOutOfRange)?;
    if wav.len() < 12 || wav.get(..4) != Some(b"RIFF") || wav.get(8..12) != Some(b"WAVE") {
        return Err(ChunkWriteError::NotRiffWave);
    }

    let mut out = Vec::with_capacity(wav.len() + payload.len() + 9);
    out.extend_from_slice(&wav[..12]);

    let mut offset = 12usize;
    while offset < wav.len() {
        let Some(header) = wav.get(offset..offset + 8) else {
            return Err(ChunkWriteError::TruncatedChunkTable);
        };
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let body_start = offset + 8;
        let Some(body) = size
            .checked_add(body_start)
            .and_then(|end| wav.get(body_start..end))
        else {
            return Err(ChunkWriteError::ChunkPastEnd);
        };
        if header.get(..4) != Some(&RIFF_CHUNK_ID[..]) {
            out.extend_from_slice(header);
            out.extend_from_slice(body);
            if size % 2 == 1 {
                out.push(0);
            }
        }
        // A source file may omit the final pad byte, so the advance is clamped to the real end.
        offset = (body_start + size + (size % 2)).min(wav.len());
    }

    out.extend_from_slice(&RIFF_CHUNK_ID);
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }

    let riff_size = u32::try_from(out.len() - 8).map_err(|_| ChunkWriteError::ExceedsRiffLimit)?;
    out.splice(4..8, riff_size.to_le_bytes());
    Ok(out)
}
