use crate::buffer::AudioBuffer;
use crate::error::AudioError;
use crate::iff;
use crate::wav;

/// Ceilings applied to untrusted input before anything is allocated from it.
///
/// `max_samples` is a frame-times-channel count, so the worst-case decode
/// allocation is `4 * max_samples` bytes regardless of what a header claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeLimits {
    pub max_bytes: usize,
    pub max_samples: usize,
}

impl DecodeLimits {
    pub const DEFAULT_MAX_BYTES: usize = 1 << 30;
    pub const DEFAULT_MAX_SAMPLES: usize = 1 << 27;

    pub const fn new(max_bytes: usize, max_samples: usize) -> Self {
        Self {
            max_bytes,
            max_samples,
        }
    }
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_BYTES, Self::DEFAULT_MAX_SAMPLES)
    }
}

/// Decodes any supported container from memory.
///
/// WAV is handled by this crate's strict RIFF reader; every other format goes
/// through symphonia when the `codecs` feature is on.
pub fn decode_bytes(bytes: &[u8], limits: &DecodeLimits) -> Result<AudioBuffer, AudioError> {
    if bytes.len() > limits.max_bytes {
        return Err(AudioError::ByteLimitExceeded {
            limit: limits.max_bytes,
            found: bytes.len(),
        });
    }
    if wav::is_wav(bytes) {
        return wav::decode(bytes, limits);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"FORM" {
        iff::validate_aiff(bytes)?;
    }
    decode_with_codecs(bytes, limits)
}

#[cfg(feature = "codecs")]
fn decode_with_codecs(bytes: &[u8], limits: &DecodeLimits) -> Result<AudioBuffer, AudioError> {
    use symphonia::core::audio::AudioSpec;
    use symphonia::core::codecs::CodecParameters;
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error as SymphoniaError;
    use symphonia::core::formats::TrackType;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::io::MediaSourceStream;

    let source = MediaSourceStream::new(
        Box::new(std::io::Cursor::new(bytes.to_vec())),
        Default::default(),
    );
    let mut reader = symphonia::default::get_probe().probe(
        &Hint::new(),
        source,
        Default::default(),
        Default::default(),
    )?;

    let track = reader
        .first_track_known_codec(TrackType::Audio)
        .ok_or(AudioError::UnsupportedContainer("no decodable audio track"))?;
    let track_id = track.id;
    let params = match track.codec_params.as_ref() {
        Some(CodecParameters::Audio(params)) => params.clone(),
        _ => return Err(AudioError::UnsupportedContainer("track is not audio")),
    };

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())?;

    let mut spec: Option<AudioSpec> = None;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut chunk: Vec<f32> = Vec::new();
    loop {
        let packet = match reader.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(err) => return Err(err.into()),
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A corrupt packet is skipped rather than failing the whole decode;
            // symphonia documents both of these as recoverable.
            Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::ResetRequired) => continue,
            Err(err) => return Err(err.into()),
        };
        if spec.is_none() {
            spec = Some(decoded.spec().clone());
        }
        decoded.copy_to_vec_interleaved(&mut chunk);
        if interleaved.len() + chunk.len() > limits.max_samples {
            return Err(AudioError::SampleLimitExceeded {
                limit: limits.max_samples,
                requested: (interleaved.len() + chunk.len()) as u64,
            });
        }
        interleaved.append(&mut chunk);
    }

    let spec = spec.ok_or(AudioError::UnsupportedContainer(
        "stream decoded to zero packets",
    ))?;
    AudioBuffer::from_interleaved(spec.rate(), spec.channels().count(), &interleaved)
}

#[cfg(not(feature = "codecs"))]
fn decode_with_codecs(_bytes: &[u8], _limits: &DecodeLimits) -> Result<AudioBuffer, AudioError> {
    Err(AudioError::UnsupportedContainer(
        "only wav is available without the `codecs` feature",
    ))
}
