use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Why an asset could not be described.
///
/// Every variant is recoverable: `recover_mark` reports "no mark" rather than
/// failing, matching the Python contract that an undescribable asset degrades to
/// an honest absence.
#[derive(Debug, thiserror::Error)]
pub enum DescriptorError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Malformed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    Little,
    Big,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_width: u8,
    pub byte_order: ByteOrder,
}

/// Bounds on what the chunk walk will accept from an untrusted file. Every one of
/// them turns a hostile header into a rejection rather than an allocation.
const MAX_CHUNKS: usize = 64;
const MAX_SAMPLE_RATE: u32 = 384_000;
const MAX_CHANNELS: u16 = 32;
const MAX_WINDOW_BYTES: u64 = 8 << 20;
const HEADER_BYTES: usize = 12;
const CHUNK_HEADER_BYTES: u64 = 8;

pub struct PcmReader {
    file: File,
    path: PathBuf,
    format: PcmFormat,
    remaining_bytes: u64,
}

impl PcmReader {
    pub fn open(path: &Path) -> Result<Self, DescriptorError> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default();
        match extension.as_str() {
            "wav" => Self::open_riff(path),
            "aif" | "aiff" | "aifc" => Self::open_aiff(path),
            _ => Err(DescriptorError::Unsupported(
                "only PCM WAV and AIFF exports are supported".to_string(),
            )),
        }
    }

    pub fn format(&self) -> PcmFormat {
        self.format
    }

    /// Frames still unread in the data chunk. On a freshly opened reader this is
    /// the file's whole frame count, which is what a duration needs.
    pub fn remaining_frames(&self) -> u64 {
        let block = u64::from(self.format.channels) * u64::from(self.format.sample_width);
        if block == 0 {
            return 0;
        }
        self.remaining_bytes / block
    }

    /// Read up to `frames` frames and return them mono-mixed.
    ///
    /// A short read is reported as a shorter vector; the caller decides whether a
    /// partial window counts.
    pub fn read_mono_frames(&mut self, frames: usize) -> Result<Vec<f64>, DescriptorError> {
        let block = u64::from(self.format.channels) * u64::from(self.format.sample_width);
        let wanted = (frames as u64).saturating_mul(block).min(self.remaining_bytes);
        if wanted > MAX_WINDOW_BYTES {
            return Err(DescriptorError::Unsupported(format!(
                "window of {wanted} bytes exceeds the {MAX_WINDOW_BYTES}-byte bound"
            )));
        }
        let mut raw = vec![0u8; wanted as usize];
        let read = read_at_most(&mut self.file, &mut raw).map_err(|source| DescriptorError::Io {
            path: self.path.clone(),
            source,
        })?;
        raw.truncate(read);
        self.remaining_bytes -= read as u64;
        decode_mono(&raw, self.format)
    }
}

fn read_at_most(file: &mut File, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0usize;
    while filled < buffer.len() {
        let Some(slice) = buffer.get_mut(filled..) else {
            break;
        };
        match file.read(slice)? {
            0 => break,
            count => filled += count,
        }
    }
    Ok(filled)
}

fn decode_mono(raw: &[u8], format: PcmFormat) -> Result<Vec<f64>, DescriptorError> {
    let width = usize::from(format.sample_width);
    let channels = usize::from(format.channels);
    let frame_bytes = width * channels;
    if frame_bytes == 0 {
        return Err(DescriptorError::Malformed(
            "invalid audio format metadata".to_string(),
        ));
    }
    let mut mono = Vec::with_capacity(raw.len() / frame_bytes);
    for frame in raw.chunks_exact(frame_bytes) {
        let mut total = 0.0f64;
        for sample in frame.chunks_exact(width) {
            total += decode_sample(sample, format.byte_order)?;
        }
        mono.push(total / channels as f64);
    }
    Ok(mono)
}

fn decode_sample(bytes: &[u8], order: ByteOrder) -> Result<f64, DescriptorError> {
    match bytes.len() {
        1 => Ok((f64::from(*bytes.first().unwrap_or(&0)) - 128.0) / 128.0),
        2 => {
            let value = i16::from_be_bytes(be2(bytes, order));
            Ok(f64::from(value) / 32768.0)
        }
        3 => {
            let raw = be3(bytes, order);
            let unsigned =
                (u32::from(raw[0]) << 16) | (u32::from(raw[1]) << 8) | u32::from(raw[2]);
            let value = if unsigned & 0x0080_0000 != 0 {
                unsigned as i32 - 0x0100_0000
            } else {
                unsigned as i32
            };
            Ok(f64::from(value) / 8_388_608.0)
        }
        4 => {
            let value = i32::from_be_bytes(be4(bytes, order));
            Ok(f64::from(value) / 2_147_483_648.0)
        }
        width => Err(DescriptorError::Unsupported(format!(
            "unsupported PCM sample width: {width} bytes"
        ))),
    }
}

fn be2(bytes: &[u8], order: ByteOrder) -> [u8; 2] {
    let a = *bytes.first().unwrap_or(&0);
    let b = *bytes.get(1).unwrap_or(&0);
    match order {
        ByteOrder::Big => [a, b],
        ByteOrder::Little => [b, a],
    }
}

fn be3(bytes: &[u8], order: ByteOrder) -> [u8; 3] {
    let a = *bytes.first().unwrap_or(&0);
    let b = *bytes.get(1).unwrap_or(&0);
    let c = *bytes.get(2).unwrap_or(&0);
    match order {
        ByteOrder::Big => [a, b, c],
        ByteOrder::Little => [c, b, a],
    }
}

fn be4(bytes: &[u8], order: ByteOrder) -> [u8; 4] {
    let a = *bytes.first().unwrap_or(&0);
    let b = *bytes.get(1).unwrap_or(&0);
    let c = *bytes.get(2).unwrap_or(&0);
    let d = *bytes.get(3).unwrap_or(&0);
    match order {
        ByteOrder::Big => [a, b, c, d],
        ByteOrder::Little => [d, c, b, a],
    }
}

struct ChunkWalk {
    file: File,
    path: PathBuf,
    file_length: u64,
    position: u64,
    seen: usize,
}

impl ChunkWalk {
    fn open(path: &Path, container: &[u8; 4], forms: &[&[u8; 4]]) -> Result<Self, DescriptorError> {
        let mut file = File::open(path).map_err(|source| DescriptorError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let file_length = file
            .metadata()
            .map_err(|source| DescriptorError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .len();
        let mut header = [0u8; HEADER_BYTES];
        file.read_exact(&mut header)
            .map_err(|_| malformed(path, "file is shorter than its container header"))?;
        if header.get(..4) != Some(container.as_slice()) {
            return Err(DescriptorError::Unsupported(format!(
                "not a {} container",
                String::from_utf8_lossy(container)
            )));
        }
        let form = header.get(8..12).unwrap_or_default();
        if !forms.iter().any(|candidate| candidate.as_slice() == form) {
            return Err(DescriptorError::Unsupported(format!(
                "unsupported form type: {}",
                String::from_utf8_lossy(form)
            )));
        }
        Ok(ChunkWalk {
            file,
            path: path.to_path_buf(),
            file_length,
            position: HEADER_BYTES as u64,
            seen: 0,
        })
    }

    fn next_chunk(&mut self, order: ByteOrder) -> Result<Option<([u8; 4], u64, u64)>, DescriptorError> {
        if self.seen == MAX_CHUNKS {
            return Err(malformed(&self.path, "more chunks than the parser accepts"));
        }
        if self.position.saturating_add(CHUNK_HEADER_BYTES) > self.file_length {
            return Ok(None);
        }
        self.file
            .seek(SeekFrom::Start(self.position))
            .map_err(|source| DescriptorError::Io {
                path: self.path.clone(),
                source,
            })?;
        let mut header = [0u8; 8];
        self.file
            .read_exact(&mut header)
            .map_err(|_| malformed(&self.path, "truncated chunk header"))?;
        let mut id = [0u8; 4];
        id.copy_from_slice(header.get(..4).unwrap_or(&[0; 4]));
        let declared = u32::from_be_bytes(be4(header.get(4..8).unwrap_or_default(), order));
        let start = self.position.saturating_add(CHUNK_HEADER_BYTES);
        // IMPORTANT: a declared length is untrusted. Clamp it to what the file
        // actually holds instead of trusting it into a seek or an allocation.
        let available = self.file_length.saturating_sub(start);
        let length = u64::from(declared).min(available);
        self.position = start
            .saturating_add(length)
            .saturating_add(u64::from(declared) % 2);
        self.seen += 1;
        Ok(Some((id, start, length)))
    }

    fn read_chunk(&mut self, start: u64, length: u64, cap: u64) -> Result<Vec<u8>, DescriptorError> {
        let wanted = length.min(cap);
        self.file
            .seek(SeekFrom::Start(start))
            .map_err(|source| DescriptorError::Io {
                path: self.path.clone(),
                source,
            })?;
        let mut buffer = vec![0u8; wanted as usize];
        let read = read_at_most(&mut self.file, &mut buffer).map_err(|source| DescriptorError::Io {
            path: self.path.clone(),
            source,
        })?;
        buffer.truncate(read);
        Ok(buffer)
    }

    fn into_reader(
        mut self,
        format: PcmFormat,
        data_start: u64,
        data_length: u64,
    ) -> Result<PcmReader, DescriptorError> {
        self.file
            .seek(SeekFrom::Start(data_start))
            .map_err(|source| DescriptorError::Io {
                path: self.path.clone(),
                source,
            })?;
        Ok(PcmReader {
            file: self.file,
            path: self.path,
            format,
            remaining_bytes: data_length,
        })
    }
}

fn malformed(path: &Path, reason: &str) -> DescriptorError {
    DescriptorError::Malformed(format!("{}: {reason}", path.display()))
}

impl PcmReader {
    fn open_riff(path: &Path) -> Result<Self, DescriptorError> {
        let mut walk = ChunkWalk::open(path, b"RIFF", &[b"WAVE"])?;
        let mut format: Option<PcmFormat> = None;
        let mut data: Option<(u64, u64)> = None;
        while let Some((id, start, length)) = walk.next_chunk(ByteOrder::Little)? {
            match &id {
                b"fmt " => {
                    let bytes = walk.read_chunk(start, length, 64)?;
                    format = Some(parse_wave_format(&bytes)?);
                }
                b"data" => data = Some((start, length)),
                _ => {}
            }
        }
        let format =
            format.ok_or_else(|| malformed(path, "WAV file has no fmt chunk"))?;
        let (data_start, data_length) =
            data.ok_or_else(|| malformed(path, "WAV file has no data chunk"))?;
        walk.into_reader(format, data_start, data_length)
    }

    fn open_aiff(path: &Path) -> Result<Self, DescriptorError> {
        let mut walk = ChunkWalk::open(path, b"FORM", &[b"AIFF", b"AIFC"])?;
        let mut common: Option<(u16, u8, u32)> = None;
        let mut sound: Option<(u64, u64)> = None;
        while let Some((id, start, length)) = walk.next_chunk(ByteOrder::Big)? {
            match &id {
                b"COMM" => {
                    let bytes = walk.read_chunk(start, length, 64)?;
                    common = Some(parse_aiff_common(&bytes)?);
                }
                b"SSND" => {
                    let bytes = walk.read_chunk(start, length.min(8), 8)?;
                    let offset = u64::from(u32::from_be_bytes(be4(
                        bytes.get(..4).unwrap_or_default(),
                        ByteOrder::Big,
                    )));
                    let data_start = start.saturating_add(8).saturating_add(offset);
                    let data_length = length.saturating_sub(8).saturating_sub(offset);
                    sound = Some((data_start, data_length));
                }
                _ => {}
            }
        }
        let (channels, sample_width, sample_rate) =
            common.ok_or_else(|| malformed(path, "AIFF file has no COMM chunk"))?;
        let (data_start, data_length) =
            sound.ok_or_else(|| malformed(path, "AIFF file has no SSND chunk"))?;
        walk.into_reader(
            PcmFormat {
                sample_rate,
                channels,
                sample_width,
                byte_order: ByteOrder::Big,
            },
            data_start,
            data_length,
        )
    }
}

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;

fn parse_wave_format(bytes: &[u8]) -> Result<PcmFormat, DescriptorError> {
    if bytes.len() < 16 {
        return Err(DescriptorError::Malformed(
            "fmt chunk is shorter than 16 bytes".to_string(),
        ));
    }
    let tag = u16::from_le_bytes(le2(bytes, 0));
    match tag {
        WAVE_FORMAT_PCM => {}
        // IMPORTANT: 32-bit float WAV breaks this repo's stem/export feature
        // association, so it is refused with the remedy rather than half-handled.
        WAVE_FORMAT_IEEE_FLOAT => {
            return Err(DescriptorError::Unsupported(
                "32-bit float WAV is not supported; export 16-bit PCM WAV".to_string(),
            ))
        }
        WAVE_FORMAT_EXTENSIBLE => {
            return Err(DescriptorError::Unsupported(
                "WAVE_FORMAT_EXTENSIBLE is not supported; export 16-bit PCM WAV".to_string(),
            ))
        }
        _ => {
            return Err(DescriptorError::Unsupported(
                "compressed WAV is not supported".to_string(),
            ))
        }
    }
    let channels = u16::from_le_bytes(le2(bytes, 2));
    let sample_rate = u32::from_le_bytes(le4(bytes, 4));
    let bits = u16::from_le_bytes(le2(bytes, 14));
    build_format(channels, bits, sample_rate, ByteOrder::Little)
}

fn parse_aiff_common(bytes: &[u8]) -> Result<(u16, u8, u32), DescriptorError> {
    if bytes.len() < 18 {
        return Err(DescriptorError::Malformed(
            "COMM chunk is shorter than 18 bytes".to_string(),
        ));
    }
    if bytes.len() >= 22 {
        let compression = bytes.get(18..22).unwrap_or_default();
        if compression != b"NONE" {
            return Err(DescriptorError::Unsupported(
                "compressed AIFF is not supported".to_string(),
            ));
        }
    }
    let channels = u16::from_be_bytes(be2(bytes.get(0..2).unwrap_or_default(), ByteOrder::Big));
    let bits = u16::from_be_bytes(be2(bytes.get(6..8).unwrap_or_default(), ByteOrder::Big));
    let sample_rate = extended80_to_u32(bytes.get(8..18).unwrap_or_default())?;
    let format = build_format(channels, bits, sample_rate, ByteOrder::Big)?;
    Ok((format.channels, format.sample_width, format.sample_rate))
}

fn build_format(
    channels: u16,
    bits: u16,
    sample_rate: u32,
    byte_order: ByteOrder,
) -> Result<PcmFormat, DescriptorError> {
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(DescriptorError::Unsupported(format!(
            "channel count {channels} is outside the supported range 1..={MAX_CHANNELS}"
        )));
    }
    if sample_rate == 0 || sample_rate > MAX_SAMPLE_RATE {
        return Err(DescriptorError::Unsupported(format!(
            "sample rate {sample_rate} is outside the supported range 1..={MAX_SAMPLE_RATE}"
        )));
    }
    if bits % 8 != 0 || !(8..=32).contains(&bits) {
        return Err(DescriptorError::Unsupported(format!(
            "unsupported PCM sample width: {bits} bits"
        )));
    }
    Ok(PcmFormat {
        sample_rate,
        channels,
        sample_width: (bits / 8) as u8,
        byte_order,
    })
}

/// Decode the 80-bit IEEE 754 extended sample rate an AIFF COMM chunk carries.
fn extended80_to_u32(bytes: &[u8]) -> Result<u32, DescriptorError> {
    if bytes.len() < 10 {
        return Err(DescriptorError::Malformed(
            "COMM sample rate field is truncated".to_string(),
        ));
    }
    let exponent = i32::from(u16::from_be_bytes(be2(
        bytes.get(0..2).unwrap_or_default(),
        ByteOrder::Big,
    )) & 0x7fff);
    let mut mantissa = 0u64;
    for byte in bytes.get(2..10).unwrap_or_default() {
        mantissa = (mantissa << 8) | u64::from(*byte);
    }
    if exponent == 0 && mantissa == 0 {
        return Ok(0);
    }
    let shift = exponent - 16383 - 63;
    let value = if shift >= 0 {
        (mantissa as f64) * 2f64.powi(shift)
    } else {
        (mantissa as f64) / 2f64.powi(-shift)
    };
    if !(0.0..=f64::from(u32::MAX)).contains(&value) {
        return Err(DescriptorError::Unsupported(
            "AIFF sample rate is out of range".to_string(),
        ));
    }
    Ok(value.round() as u32)
}

fn le2(bytes: &[u8], offset: usize) -> [u8; 2] {
    [
        *bytes.get(offset).unwrap_or(&0),
        *bytes.get(offset + 1).unwrap_or(&0),
    ]
}

fn le4(bytes: &[u8], offset: usize) -> [u8; 4] {
    [
        *bytes.get(offset).unwrap_or(&0),
        *bytes.get(offset + 1).unwrap_or(&0),
        *bytes.get(offset + 2).unwrap_or(&0),
        *bytes.get(offset + 3).unwrap_or(&0),
    ]
}
