use std::io::Cursor;

use hound::{SampleFormat, WavSpec, WavWriter};

use crate::buffer::{AudioBuffer, MAX_CHANNELS, MAX_SAMPLE_RATE};
use crate::decode::DecodeLimits;
use crate::error::{AudioError, ChunkId};
use crate::iff::{ChunkWalker, Endian, form_header};

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitDepth {
    Int16,
    Int24,
    Int32,
    Float32,
}

impl BitDepth {
    pub const fn bits(self) -> u16 {
        match self {
            Self::Int16 => 16,
            Self::Int24 => 24,
            Self::Int32 | Self::Float32 => 32,
        }
    }

    const fn sample_format(self) -> SampleFormat {
        match self {
            Self::Float32 => SampleFormat::Float,
            _ => SampleFormat::Int,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WaveFormat {
    tag: u16,
    channels: usize,
    sample_rate: u32,
    bits: u16,
    block_align: usize,
}

pub fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

pub fn decode(bytes: &[u8], limits: &DecodeLimits) -> Result<AudioBuffer, AudioError> {
    if bytes.len() > limits.max_bytes {
        return Err(AudioError::ByteLimitExceeded {
            limit: limits.max_bytes,
            found: bytes.len(),
        });
    }
    form_header(bytes, b"RIFF", Endian::Little, "wav")?;
    if &bytes[8..12] != b"WAVE" {
        return Err(AudioError::UnsupportedContainer("wav"));
    }

    let mut walker = ChunkWalker::new(bytes, Endian::Little, "wav");
    let mut format: Option<WaveFormat> = None;
    let mut data: Option<&[u8]> = None;
    while let Some(chunk) = walker.next_chunk()? {
        if chunk.id == ChunkId::FMT && format.is_none() {
            format = Some(parse_format(walker.payload(&chunk)?)?);
        } else if chunk.id == ChunkId::DATA && data.is_none() {
            data = Some(walker.payload(&chunk)?);
        }
    }

    let format = format.ok_or(AudioError::MissingChunk {
        chunk: ChunkId::FMT,
    })?;
    let data = data.ok_or(AudioError::MissingChunk {
        chunk: ChunkId::DATA,
    })?;

    if !data.len().is_multiple_of(format.block_align) {
        return Err(AudioError::TruncatedChunk {
            chunk: ChunkId::DATA,
            declared: data.len() as u64,
            available: (data.len() - data.len() % format.block_align) as u64,
        });
    }
    let frames = data.len() / format.block_align;
    let total = (frames as u64).checked_mul(format.channels as u64).ok_or(
        AudioError::SampleLimitExceeded {
            limit: limits.max_samples,
            requested: u64::MAX,
        },
    )?;
    if total > limits.max_samples as u64 {
        return Err(AudioError::SampleLimitExceeded {
            limit: limits.max_samples,
            requested: total,
        });
    }

    let mut planes = vec![0.0f32; frames * format.channels];
    let bytes_per_sample = format.block_align / format.channels;
    for frame in 0..frames {
        let frame_start = frame * format.block_align;
        for channel in 0..format.channels {
            let at = frame_start + channel * bytes_per_sample;
            let raw = &data[at..at + bytes_per_sample];
            planes[channel * frames + frame] = decode_sample(raw, format.tag, format.bits);
        }
    }

    AudioBuffer::from_planes(format.sample_rate, format.channels, planes)
}

fn parse_format(payload: &[u8]) -> Result<WaveFormat, AudioError> {
    if payload.len() < 16 {
        return Err(AudioError::MalformedHeader {
            container: "wav",
            reason: "fmt chunk shorter than 16 bytes",
        });
    }
    let mut tag = u16::from_le_bytes([payload[0], payload[1]]);
    let channels = u16::from_le_bytes([payload[2], payload[3]]);
    let sample_rate = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let block_align = u16::from_le_bytes([payload[12], payload[13]]);
    let bits = u16::from_le_bytes([payload[14], payload[15]]);

    if tag == WAVE_FORMAT_EXTENSIBLE {
        if payload.len() < 26 {
            return Err(AudioError::MalformedHeader {
                container: "wav",
                reason: "WAVE_FORMAT_EXTENSIBLE fmt chunk shorter than 26 bytes",
            });
        }
        tag = u16::from_le_bytes([payload[24], payload[25]]);
    }

    if channels == 0 || usize::from(channels) > MAX_CHANNELS {
        return Err(AudioError::InvalidChannelCount {
            found: u64::from(channels),
        });
    }
    if sample_rate == 0 || sample_rate > MAX_SAMPLE_RATE {
        return Err(AudioError::InvalidSampleRate {
            found: u64::from(sample_rate),
        });
    }

    let supported = match tag {
        WAVE_FORMAT_PCM => matches!(bits, 8 | 16 | 24 | 32),
        WAVE_FORMAT_IEEE_FLOAT => bits == 32,
        _ => false,
    };
    if !supported {
        return Err(AudioError::UnsupportedSampleFormat {
            format_tag: tag,
            bits,
        });
    }

    let expected_align = usize::from(channels) * usize::from(bits / 8);
    if usize::from(block_align) != expected_align {
        return Err(AudioError::MalformedHeader {
            container: "wav",
            reason: "block align disagrees with channel count and bit depth",
        });
    }

    Ok(WaveFormat {
        tag,
        channels: usize::from(channels),
        sample_rate,
        bits,
        block_align: expected_align,
    })
}

/// Scale factors match `daemon/audio_association.py::_decode_pcm` so a Audio Provenance
/// feature sequence is directly comparable with a POC one. Float input is a
/// bit-exact passthrough: the POC's reader rejected float WAV outright, and
/// rescaling or clamping here would silently move the samples the hash covers.
fn decode_sample(raw: &[u8], tag: u16, bits: u16) -> f32 {
    if tag == WAVE_FORMAT_IEEE_FLOAT {
        return f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    }
    match bits {
        8 => (f32::from(raw[0]) - 128.0) / 128.0,
        16 => f32::from(i16::from_le_bytes([raw[0], raw[1]])) / 32_768.0,
        24 => {
            let value = i32::from_le_bytes([0, raw[0], raw[1], raw[2]]) >> 8;
            value as f32 / 8_388_608.0
        }
        _ => i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as f32 / 2_147_483_648.0,
    }
}

pub fn encode(buffer: &AudioBuffer, depth: BitDepth) -> Result<Vec<u8>, AudioError> {
    let spec = WavSpec {
        channels: buffer.channels() as u16,
        sample_rate: buffer.sample_rate(),
        bits_per_sample: depth.bits(),
        sample_format: depth.sample_format(),
    };
    let mut sink = Cursor::new(Vec::new());
    {
        let mut writer = WavWriter::new(&mut sink, spec)?;
        let frames = buffer.frames();
        let channels = buffer.channels();
        let planes = buffer.planes();
        for frame in 0..frames {
            for channel in 0..channels {
                let sample = planes[channel * frames + frame];
                match depth {
                    BitDepth::Int16 => writer.write_sample(quantize(sample, 15) as i16)?,
                    BitDepth::Int24 | BitDepth::Int32 => {
                        let bits = if depth == BitDepth::Int24 { 23 } else { 31 };
                        writer.write_sample(quantize(sample, bits))?;
                    }
                    BitDepth::Float32 => writer.write_sample(sample)?,
                }
            }
        }
        writer.finalize()?;
    }
    Ok(sink.into_inner())
}

/// Round-to-nearest with a symmetric clamp. `full_scale` is `2^fractional_bits`,
/// which is what the decoder divides by, so an integer sample that was decoded
/// and re-encoded lands back on its original code.
fn quantize(sample: f32, fractional_bits: u32) -> i32 {
    let full_scale = f64::from(1u32 << fractional_bits);
    let max = full_scale - 1.0;
    let scaled = f64::from(sample) * full_scale;
    if scaled.is_nan() {
        return 0;
    }
    scaled.round().clamp(-full_scale, max) as i32
}
