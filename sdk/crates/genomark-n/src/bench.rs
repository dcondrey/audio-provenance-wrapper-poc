use std::path::Path;

use audio_provenance_audio::AudioBuffer;
use audio_provenance_bench::error::{AudioError as BenchAudioError, CodecError};
use audio_provenance_bench::watermark::{Detection, WatermarkCodec};
use audio_provenance_core::CodedError;

use crate::error::GenoMarkNError;
use crate::model::GenoMarkN;
use crate::params::{ALGORITHM_ID, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE, PAYLOAD_BYTES, UNSUPPORTED};

/// GenoMark-N under the bench's own measurement contract, so the existing matrix measures it with
/// no bench changes.
///
/// The bench never reaches inside an implementation, so nothing here remembers what it embedded and
/// `detect` receives audio and nothing else. A presence-tier hit reports NO payload: the bench
/// counts `payload.is_some()` as an accept, and presence is a Finding, never an identity.
#[derive(Debug)]
pub struct GenoMarkNCodec {
    inner: GenoMarkN,
}

impl GenoMarkNCodec {
    pub fn load(card_path: &Path) -> Result<Self, GenoMarkNError> {
        Ok(Self {
            inner: GenoMarkN::load(card_path)?,
        })
    }

    pub const fn new(inner: GenoMarkN) -> Self {
        Self { inner }
    }

    pub const fn model(&self) -> &GenoMarkN {
        &self.inner
    }
}

fn to_codec_error(error: GenoMarkNError) -> CodecError {
    match error {
        GenoMarkNError::PayloadLength { found, expected } => {
            CodecError::PayloadLength { found, expected }
        }
        GenoMarkNError::Empty => CodecError::Audio(BenchAudioError::Empty),
        GenoMarkNError::NonFinite { channel, frame } => {
            CodecError::Audio(BenchAudioError::NonFinite { channel, frame })
        }
        GenoMarkNError::SampleRateTooLow { found, min } => {
            CodecError::Audio(BenchAudioError::SampleRate {
                found,
                min,
                max: MAX_SAMPLE_RATE,
            })
        }
        GenoMarkNError::SampleRateTooHigh { found, max } => {
            CodecError::Audio(BenchAudioError::SampleRate {
                found,
                min: MIN_SAMPLE_RATE,
                max,
            })
        }
        GenoMarkNError::Audio(inner) => CodecError::Audio(BenchAudioError::Substrate(inner)),
        other => CodecError::Audio(BenchAudioError::Substrate(
            audio_provenance_audio::AudioError::InvalidFilterParameter(other.code()),
        )),
    }
}

impl WatermarkCodec for GenoMarkNCodec {
    fn name(&self) -> &str {
        ALGORITHM_ID
    }

    fn payload_len(&self) -> usize {
        PAYLOAD_BYTES
    }

    /// A card marked `fixture` loads a hand-written arithmetic graph with no learned parameters.
    /// It is not a product watermark and the report labels every row it produces as a fixture.
    fn is_bench_fixture(&self) -> bool {
        self.inner.is_fixture()
    }

    fn describe(&self) -> String {
        let capabilities = self.inner.capabilities();
        format!(
            "GenoMark-N ({ALGORITHM_ID}), model {} epoch {}{}. Learned per-bin log-gain mask on \
             the 48 kHz STFT magnitude over bins 9-328 (211-7688 Hz), 2048-point transform at a \
             512 hop, read back by a fully convolutional decoder whose message pooling is a global \
             average over the presence-weighted span. NO OFFSET SEARCH AND NO ORACLE: detection \
             receives audio alone, and every threshold is frozen in the model card. 32 information \
             bits gated by CRC-24 with an ordered-statistics flip search of k=2 over the 4 least \
             confident bits, 11 CRC trials per window. Acoustic re-recording is {}. NOTHING HERE \
             IS A MEASURED CAPABILITY: no trained model and no physical capture campaign exist.",
            capabilities.model_id,
            capabilities.epoch,
            if capabilities.is_fixture {
                " [PLUMBING FIXTURE, NOT A TRAINED MODEL]"
            } else {
                ""
            },
            capabilities.acoustic_rerecording.status(),
        )
    }

    fn embed(&self, audio: &AudioBuffer, payload: &[u8]) -> Result<AudioBuffer, CodecError> {
        self.inner
            .embed_bytes(audio, payload)
            .map_err(to_codec_error)
    }

    fn detect(&self, audio: &AudioBuffer) -> Result<Detection, CodecError> {
        let outcome = self.inner.detect(audio).map_err(to_codec_error)?;
        Ok(Detection {
            payload: outcome.payload().map(|payload| payload.to_bytes().to_vec()),
            confidence: outcome.confidence(),
            bits_corrected: outcome.bits_corrected(),
            candidates_examined: outcome.crc_trials(),
            notes: Some(format!(
                "class {}, presence score {:.4}, {:.2} s presence-positive, {} windows read, \
                 {} crc trials, {} frames{}",
                outcome.class().as_str(),
                outcome.presence_score(),
                outcome.presence_positive_seconds(),
                outcome.windows_examined(),
                outcome.crc_trials(),
                outcome.frames_analysed(),
                if self.inner.capabilities().acoustic_rerecording.status() == UNSUPPORTED {
                    ", acoustic path unsupported"
                } else {
                    ""
                }
            )),
        })
    }
}
