use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::{file_name, io_error, C2paError, Result};

pub const WAV_MIME: &str = "audio/wav";
pub const AIFF_MIME: &str = "audio/aiff";

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Upper bound on how much of any one chunk the format probe will hold in memory.
/// REQUIRED: the declared chunk length is attacker-controlled, so it never sizes an allocation.
const MAX_CHUNK_PAYLOAD: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContainerKind {
    Wav,
    Aiff,
}

impl ContainerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ContainerKind::Wav => "wav",
            ContainerKind::Aiff => "aiff",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            ContainerKind::Wav => WAV_MIME,
            ContainerKind::Aiff => AIFF_MIME,
        }
    }

    /// c2pa-rs ships a RIFF handler and no AIFF handler, so only WAV can carry an
    /// embedded manifest. AIFF is sidecar-only.
    pub fn embeddable(self) -> bool {
        matches!(self, ContainerKind::Wav)
    }
}

impl core::fmt::Display for ContainerKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SampleFormat {
    Pcm,
}

impl SampleFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            SampleFormat::Pcm => "pcm",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssetFormat {
    pub container: ContainerKind,
    pub mime: &'static str,
    pub embeddable: bool,
    pub sample_format: SampleFormat,
    pub bit_depth: i32,
}

fn float_export_error(container: &str) -> String {
    format!(
        "32-bit float {container} is not supported for provenance signing because the \
         association between the observed stems and the export does not survive the float \
         conversion. Re-export as 16-bit PCM WAV and sign that file."
    )
}

fn unsupported(path: &Path, reason: &str) -> C2paError {
    C2paError::UnsupportedAsset(format!("{}: {reason}", file_name(path)))
}

fn chunk_repr(id: &[u8]) -> String {
    let mut out = String::from("b'");
    for byte in id {
        match *byte {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            0x20..=0x7e => out.push(*byte as char),
            other => out.push_str(&format!("\\x{other:02x}")),
        }
    }
    out.push('\'');
    out
}

fn read_head(path: &Path, size: usize) -> Result<Vec<u8>> {
    let mut file = File::open(path).map_err(|e| io_error(path, e))?;
    let mut buf = vec![0u8; size];
    let mut filled = 0usize;
    while filled < size {
        let read = file
            .read(buf.get_mut(filled..).unwrap_or_default())
            .map_err(|e| io_error(path, e))?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    buf.truncate(filled);
    Ok(buf)
}

/// Walk the top-level IFF/RIFF chunk table without loading the audio data.
fn find_chunk(path: &Path, wanted: &[u8; 4], big_endian: bool) -> Result<Option<Vec<u8>>> {
    let mut file = File::open(path).map_err(|e| io_error(path, e))?;
    let size = file.metadata().map_err(|e| io_error(path, e))?.len();
    let mut offset: u64 = 12;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| io_error(path, e))?;

    while offset.checked_add(8).is_some_and(|end| end <= size) {
        let mut header = [0u8; 8];
        let mut filled = 0usize;
        while filled < header.len() {
            let read = file
                .read(header.get_mut(filled..).unwrap_or_default())
                .map_err(|e| io_error(path, e))?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled < 8 {
            return Ok(None);
        }

        let mut id = [0u8; 4];
        let mut raw_len = [0u8; 4];
        id.copy_from_slice(header.get(0..4).unwrap_or(&[0; 4]));
        raw_len.copy_from_slice(header.get(4..8).unwrap_or(&[0; 4]));
        let length = u64::from(if big_endian {
            u32::from_be_bytes(raw_len)
        } else {
            u32::from_le_bytes(raw_len)
        });

        let remaining = size - offset - 8;
        if length > remaining {
            return Err(unsupported(
                path,
                &format!(
                    "chunk {} declares {length} bytes but the file ends first",
                    chunk_repr(&id)
                ),
            ));
        }

        if &id == wanted {
            let take = length.min(MAX_CHUNK_PAYLOAD as u64);
            let mut payload = Vec::new();
            file.take(take)
                .read_to_end(&mut payload)
                .map_err(|e| io_error(path, e))?;
            return Ok(Some(payload));
        }

        let padded = length
            .checked_add(length & 1)
            .and_then(|len| len.checked_add(8))
            .and_then(|step| offset.checked_add(step));
        let Some(next) = padded else {
            return Ok(None);
        };
        offset = next;
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| io_error(path, e))?;
    }
    Ok(None)
}

fn le_u16(payload: &[u8], at: usize) -> Option<u16> {
    let mut buf = [0u8; 2];
    buf.copy_from_slice(payload.get(at..at.checked_add(2)?)?);
    Some(u16::from_le_bytes(buf))
}

fn be_i16(payload: &[u8], at: usize) -> Option<i16> {
    let mut buf = [0u8; 2];
    buf.copy_from_slice(payload.get(at..at.checked_add(2)?)?);
    Some(i16::from_be_bytes(buf))
}

fn wave_format(path: &Path) -> Result<AssetFormat> {
    let Some(payload) = find_chunk(path, b"fmt ", false)? else {
        return Err(unsupported(path, "no fmt chunk found in RIFF/WAVE file"));
    };
    if payload.len() < 16 {
        return Err(unsupported(path, "truncated WAV fmt chunk"));
    }
    let Some(mut tag) = le_u16(&payload, 0) else {
        return Err(unsupported(path, "truncated WAV fmt chunk"));
    };
    let Some(bits) = le_u16(&payload, 14) else {
        return Err(unsupported(path, "truncated WAV fmt chunk"));
    };
    if tag == WAVE_FORMAT_EXTENSIBLE {
        if payload.len() < 26 {
            return Err(unsupported(
                path,
                "truncated WAVE_FORMAT_EXTENSIBLE fmt chunk",
            ));
        }
        let Some(sub) = le_u16(&payload, 24) else {
            return Err(unsupported(
                path,
                "truncated WAVE_FORMAT_EXTENSIBLE fmt chunk",
            ));
        };
        tag = sub;
    }
    if tag == WAVE_FORMAT_IEEE_FLOAT {
        return Err(C2paError::UnsupportedAsset(float_export_error("WAV")));
    }
    if tag != WAVE_FORMAT_PCM {
        return Err(unsupported(
            path,
            &format!("WAV format tag 0x{tag:04x} is not supported; export 16-bit PCM WAV"),
        ));
    }
    Ok(AssetFormat {
        container: ContainerKind::Wav,
        mime: WAV_MIME,
        embeddable: true,
        sample_format: SampleFormat::Pcm,
        bit_depth: i32::from(bits),
    })
}

fn aiff_format(path: &Path) -> Result<AssetFormat> {
    let head = read_head(path, 16)?;
    let is_aifc = head.get(8..12) == Some(b"AIFC".as_slice());
    let Some(payload) = find_chunk(path, b"COMM", true)? else {
        return Err(unsupported(path, "no COMM chunk found in AIFF file"));
    };
    if payload.len() < 18 {
        return Err(unsupported(path, "truncated AIFF COMM chunk"));
    }
    let Some(bits) = be_i16(&payload, 6) else {
        return Err(unsupported(path, "truncated AIFF COMM chunk"));
    };
    if is_aifc {
        if let Some(compression) = payload.get(18..22) {
            let lowered = compression.to_ascii_lowercase();
            if lowered == b"fl32" || lowered == b"fl64" {
                return Err(C2paError::UnsupportedAsset(float_export_error("AIFF")));
            }
        }
    }
    Ok(AssetFormat {
        container: ContainerKind::Aiff,
        mime: AIFF_MIME,
        embeddable: false,
        sample_format: SampleFormat::Pcm,
        bit_depth: i32::from(bits),
    })
}

/// Inspect the actual bytes of `path` and decide how it may carry provenance.
///
/// IMPORTANT: this never trusts the file extension. A `.wav` holding 32-bit float
/// samples is refused here, before any signing work, because the routed/export
/// association the manifest asserts does not survive the float conversion.
pub fn detect_format(path: &Path) -> Result<AssetFormat> {
    let head = read_head(path, 16)?;
    if head.get(0..4) == Some(b"RIFF".as_slice()) && head.get(8..12) == Some(b"WAVE".as_slice()) {
        return wave_format(path);
    }
    if head.get(0..4) == Some(b"FORM".as_slice())
        && matches!(
            head.get(8..12),
            Some(b"AIFF") | Some(b"AIFC")
        )
    {
        return aiff_format(path);
    }
    Err(unsupported(
        path,
        "unrecognised audio container; the engine signs 16-bit PCM WAV (embedded) \
         and AIFF (sidecar)",
    ))
}
