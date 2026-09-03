use audio_provenance_audio::AudioBuffer;
use audio_provenance_bench::error::{AudioError as BenchAudioError, CodecError};
use audio_provenance_bench::watermark::{Detection, WatermarkCodec};
use audio_provenance_core::CodedError;

use crate::capabilities::ACOUSTIC_RERECORDING;
use crate::params::{MAX_SAMPLE_RATE, MIN_SAMPLE_RATE, PAYLOAD_BYTES};
use crate::payload::Payload;
use crate::{ALGORITHM_ID, GenoMark, GenoMarkError};

/// GenoMark under the bench's own measurement contract.
///
/// The bench never reaches inside an implementation, so nothing here may remember what it embedded.
/// `detect` receives audio and returns whatever the CRC accepts.
#[derive(Debug)]
pub struct GenoMarkCodec {
    inner: GenoMark,
}

impl GenoMarkCodec {
    pub fn public() -> Self {
        Self {
            inner: GenoMark::public(),
        }
    }

    pub const fn new(inner: GenoMark) -> Self {
        Self { inner }
    }
}

impl Default for GenoMarkCodec {
    fn default() -> Self {
        Self::public()
    }
}

fn to_codec_error(error: GenoMarkError) -> CodecError {
    match error {
        GenoMarkError::PayloadLength { found, expected } => {
            CodecError::PayloadLength { found, expected }
        }
        GenoMarkError::TooShort { frames, needed } => CodecError::TooShort { frames, needed },
        GenoMarkError::Empty => CodecError::Audio(BenchAudioError::Empty),
        GenoMarkError::NonFinite { channel, frame } => {
            CodecError::Audio(BenchAudioError::NonFinite { channel, frame })
        }
        GenoMarkError::TooLarge {
            frames,
            channels,
            limit,
        } => CodecError::Audio(BenchAudioError::TooLarge {
            frames,
            channels,
            limit,
        }),
        GenoMarkError::SampleRateTooLow { found, min } => {
            CodecError::Audio(BenchAudioError::SampleRate {
                found,
                min,
                max: MAX_SAMPLE_RATE,
            })
        }
        GenoMarkError::SampleRateTooHigh { found, max } => {
            CodecError::Audio(BenchAudioError::SampleRate {
                found,
                min: MIN_SAMPLE_RATE,
                max,
            })
        }
        GenoMarkError::Audio(inner) => CodecError::Audio(BenchAudioError::Substrate(inner)),
        other => CodecError::Audio(BenchAudioError::Substrate(
            audio_provenance_audio::AudioError::InvalidFilterParameter(other.code()),
        )),
    }
}

impl WatermarkCodec for GenoMarkCodec {
    fn name(&self) -> &str {
        ALGORITHM_ID
    }

    fn payload_len(&self) -> usize {
        PAYLOAD_BYTES
    }

    fn is_bench_fixture(&self) -> bool {
        false
    }

    fn describe(&self) -> String {
        format!(
            "GenoMark-Q ({ALGORITHM_ID}). Dither-modulated QIM over the trimmed mean of \
             adjacent-cell log-spectral-energy differences in 861-4307 Hz, 1024-sample frames at \
             50% overlap, two frames per slot, K=9 rate-1/3 convolutional code interleaved over a \
             416-slot block, CRC-32C gating acceptance. Detection is blind: audio and the profile \
             key, no original and no expected payload. Gain invariance is exact. Acoustic \
             re-recording is {ACOUSTIC_RERECORDING}."
        )
    }

    fn embed(&self, audio: &AudioBuffer, payload: &[u8]) -> Result<AudioBuffer, CodecError> {
        let parsed = Payload::from_bytes(payload).map_err(to_codec_error)?;
        self.inner.embed(audio, parsed).map_err(to_codec_error)
    }

    fn detect(&self, audio: &AudioBuffer) -> Result<Detection, CodecError> {
        match self.inner.detect(audio) {
            Ok(outcome) => Ok(Detection {
                payload: outcome.payload().map(|payload| payload.to_bytes().to_vec()),
                confidence: outcome.confidence(),
                bits_corrected: outcome.bits_corrected(),
                candidates_examined: outcome.crc_attempts() as u64,
                notes: Some(format!(
                    "class {}, {} sync candidates, {} crc attempts, {} agreeing blocks, \
                     {} combined blocks{}{}",
                    outcome.class().as_str(),
                    outcome.sync_candidates(),
                    outcome.crc_attempts(),
                    outcome.blocks_accepted(),
                    outcome.blocks_combined(),
                    if outcome.candidate_cap_reached() {
                        ", CANDIDATE CAP REACHED"
                    } else {
                        ""
                    },
                    if outcome.namespace_mismatch() {
                        ", namespace mismatch"
                    } else {
                        ""
                    }
                )),
            }),
            Err(error) => Err(to_codec_error(error)),
        }
    }
}
