use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::error::{AssocError, Result};
use crate::feature::ByteOrder;

/// A bounded stream of interleaved PCM frames.
///
/// `apw-audio` owns the general container walk; this trait is the seam an
/// AIFF or other reader plugs into without this crate growing a parser for it.
pub trait PcmSource {
    fn sample_rate_hz(&self) -> u32;
    fn channel_count(&self) -> u16;
    fn sample_width_bytes(&self) -> u16;
    fn byte_order(&self) -> ByteOrder;
    /// Up to `frames` frames of raw interleaved PCM; a short return means the
    /// stream is exhausted.
    fn read_frames(&mut self, frames: u64) -> Result<Vec<u8>>;
}

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const KSDATAFORMAT_SUBTYPE_PCM: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

/// A top-level RIFF chunk walk reproducing `wave.Wave_read`, including which
/// malformation raises which message, because that message is copied into the
/// manifest's `reason` field.
pub struct WavSource {
    reader: BufReader<File>,
    path: PathBuf,
    sample_rate_hz: u32,
    channel_count: u16,
    sample_width_bytes: u16,
    frame_size: u64,
    remaining: u64,
}

impl WavSource {
    pub fn open(path: &Path) -> Result<WavSource> {
        let file = File::open(path).map_err(|source| io_error(path, source))?;
        let file_length = file
            .metadata()
            .map_err(|source| io_error(path, source))?
            .len();
        let mut reader = BufReader::new(file);

        let header = read_bounded(&mut reader, path, 12)?;
        if header.len() < 8 {
            return Err(AssocError::Eof);
        }
        if header.get(..4) != Some(b"RIFF") {
            return Err(AssocError::UnsupportedWav("file does not start with RIFF id"));
        }
        if header.get(8..12) != Some(b"WAVE") {
            return Err(AssocError::UnsupportedWav("not a WAVE file"));
        }
        let riff_size = u64::from(read_u32_le(header.get(4..8)));
        let riff_end = file_length.min(riff_size.saturating_add(8));

        let mut format: Option<WaveFormat> = None;
        let mut data: Option<(u64, u64)> = None;
        let mut position = 12u64;
        while position.saturating_add(8) <= riff_end {
            seek_to(&mut reader, path, position)?;
            let header = read_bounded(&mut reader, path, 8)?;
            if header.len() < 8 {
                break;
            }
            let name = header.get(..4).unwrap_or_default().to_vec();
            let declared = u64::from(read_u32_le(header.get(4..8)));
            let body_start = position.saturating_add(8);
            let available = riff_end.saturating_sub(body_start);
            let readable = declared.min(available);
            if name == b"fmt " {
                let body = read_bounded(&mut reader, path, readable.min(40) as usize)?;
                format = Some(parse_format(&body)?);
            } else if name == b"data" {
                if format.is_none() {
                    return Err(AssocError::UnsupportedWav("data chunk before fmt chunk"));
                }
                data = Some((body_start, readable));
                break;
            }
            position = body_start
                .saturating_add(declared)
                .saturating_add(declared % 2);
        }

        let (Some(format), Some((data_start, data_length))) = (format, data) else {
            return Err(AssocError::UnsupportedWav(
                "fmt chunk and/or data chunk missing",
            ));
        };
        seek_to(&mut reader, path, data_start)?;
        Ok(WavSource {
            reader,
            path: path.to_path_buf(),
            sample_rate_hz: format.sample_rate_hz,
            channel_count: format.channel_count,
            sample_width_bytes: format.sample_width_bytes,
            frame_size: u64::from(format.channel_count) * u64::from(format.sample_width_bytes),
            remaining: data_length,
        })
    }
}

impl PcmSource for WavSource {
    fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    fn channel_count(&self) -> u16 {
        self.channel_count
    }

    fn sample_width_bytes(&self) -> u16 {
        self.sample_width_bytes
    }

    fn byte_order(&self) -> ByteOrder {
        ByteOrder::Little
    }

    fn read_frames(&mut self, frames: u64) -> Result<Vec<u8>> {
        let wanted = frames.saturating_mul(self.frame_size).min(self.remaining);
        let mut buffer = Vec::new();
        self.reader
            .by_ref()
            .take(wanted)
            .read_to_end(&mut buffer)
            .map_err(|source| io_error(&self.path, source))?;
        self.remaining = self.remaining.saturating_sub(buffer.len() as u64);
        Ok(buffer)
    }
}

struct WaveFormat {
    sample_rate_hz: u32,
    channel_count: u16,
    sample_width_bytes: u16,
}

fn parse_format(body: &[u8]) -> Result<WaveFormat> {
    if body.len() < 14 {
        return Err(AssocError::Eof);
    }
    let format_tag = read_u16_le(body.get(..2));
    let channel_count = read_u16_le(body.get(2..4));
    let sample_rate_hz = read_u32_le(body.get(4..8));
    if format_tag != WAVE_FORMAT_PCM && format_tag != WAVE_FORMAT_EXTENSIBLE {
        return Err(AssocError::UnknownWaveFormatTag(format_tag));
    }
    if body.len() < 16 {
        return Err(AssocError::Eof);
    }
    let bits_per_sample = read_u16_le(body.get(14..16));
    if format_tag == WAVE_FORMAT_EXTENSIBLE {
        let Some(subformat) = body.get(24..40) else {
            return Err(AssocError::Eof);
        };
        if subformat != KSDATAFORMAT_SUBTYPE_PCM {
            return Err(AssocError::UnknownExtendedWaveFormat(format_uuid_le(
                subformat,
            )));
        }
    }
    let sample_width_bytes = (bits_per_sample.saturating_add(7)) / 8;
    if sample_width_bytes == 0 {
        return Err(AssocError::UnsupportedWav("bad sample width"));
    }
    if channel_count == 0 {
        return Err(AssocError::UnsupportedWav("bad # of channels"));
    }
    Ok(WaveFormat {
        sample_rate_hz,
        channel_count,
        sample_width_bytes,
    })
}

/// `uuid.UUID(bytes_le=...)`: the first three fields are little-endian, the
/// rest is taken as written. Reproduced because a 32-bit-float export written
/// as WAVE_FORMAT_EXTENSIBLE is refused with this UUID in the message.
fn format_uuid_le(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(36);
    let groups: [&[usize]; 5] = [
        &[3, 2, 1, 0],
        &[5, 4],
        &[7, 6],
        &[8, 9],
        &[10, 11, 12, 13, 14, 15],
    ];
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            text.push('-');
        }
        for position in group.iter() {
            let byte = bytes.get(*position).copied().unwrap_or(0);
            text.push_str(&format!("{byte:02x}"));
        }
    }
    text
}

fn read_bounded(reader: &mut BufReader<File>, path: &Path, length: usize) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    reader
        .by_ref()
        .take(length as u64)
        .read_to_end(&mut buffer)
        .map_err(|source| io_error(path, source))?;
    Ok(buffer)
}

fn seek_to(reader: &mut BufReader<File>, path: &Path, position: u64) -> Result<()> {
    reader
        .seek(SeekFrom::Start(position))
        .map_err(|source| io_error(path, source))?;
    Ok(())
}

fn read_u16_le(bytes: Option<&[u8]>) -> u16 {
    match bytes {
        Some([low, high]) => u16::from_le_bytes([*low, *high]),
        _ => 0,
    }
}

fn read_u32_le(bytes: Option<&[u8]>) -> u32 {
    match bytes {
        Some([a, b, c, d]) => u32::from_le_bytes([*a, *b, *c, *d]),
        _ => 0,
    }
}

fn io_error(path: &Path, source: std::io::Error) -> AssocError {
    if source.kind() == std::io::ErrorKind::NotFound {
        AssocError::NotFound(path.to_path_buf())
    } else {
        AssocError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}
