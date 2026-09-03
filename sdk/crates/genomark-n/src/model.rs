use std::path::Path;

use audio_provenance_audio::AudioBuffer;

use crate::budget::MaskingModel;
use crate::capabilities::{AcousticRerecording, Capabilities};
use crate::card::ModelCard;
use crate::detect::NeuralDetection;
use crate::error::GenoMarkNError;
use crate::params::{ANALYSIS_SAMPLE_RATE, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE, N_FFT, PAYLOAD_BYTES};
use crate::payload::Payload;
use crate::session::{DecoderSession, EncoderSession};
use crate::spectral::{Analysis, at_analysis_rate};

/// A loaded GenoMark-N model: a card, its hash-verified graphs, and the transform that feeds them.
#[derive(Debug)]
pub struct GenoMarkN {
    card: ModelCard,
    analysis: Analysis,
    masking: MaskingModel,
    decoder: DecoderSession,
    encoder: Option<EncoderSession>,
    acoustic: AcousticRerecording,
}

impl GenoMarkN {
    /// Loads from a model card. The card's directory anchors its graph paths, every graph's
    /// SHA-256 must match the digest the card pins, and every declared tensor shape must match the
    /// transform contract this build implements.
    pub fn load(card_path: &Path) -> Result<Self, GenoMarkNError> {
        let card = ModelCard::read(card_path)?;
        let directory = card_path.parent().unwrap_or(Path::new("."));

        let decoder_bytes = card.read_graph(directory, "decoder", &card.graphs.decoder)?;
        let decoder = DecoderSession::new(
            &decoder_bytes,
            &card.io.decoder_input,
            &card.io.decoder_presence_output,
            &card.io.decoder_message_output,
        )?;

        let encoder = match &card.graphs.encoder {
            Some(reference) => {
                let bytes = card.read_graph(directory, "encoder", reference)?;
                Some(EncoderSession::new(
                    &bytes,
                    &card.io.encoder_log_mag_input,
                    &card.io.encoder_message_input,
                    &card.io.encoder_output,
                )?)
            }
            None => None,
        };

        let analysis = Analysis::new()?;
        let masking = MaskingModel::new(
            ANALYSIS_SAMPLE_RATE,
            N_FFT / 2 + 1,
            analysis.window_power(),
            card.budget.kappa,
            card.budget.max_nepers,
        );

        // A card that carries no envelope, or one whose envelope fails any gate in spec 12.3,
        // leaves the capability at the literal "unsupported". There is no other path to
        // `MeasuredLimited`, because `AcousticEnvelope` has no public constructor.
        let acoustic = match &card.operating_envelope {
            Some(claim) => AcousticRerecording::MeasuredLimited(claim.validate()?),
            None => AcousticRerecording::Unsupported,
        };

        Ok(Self {
            card,
            analysis,
            masking,
            decoder,
            encoder,
            acoustic,
        })
    }

    pub fn card(&self) -> &ModelCard {
        &self.card
    }

    pub fn capabilities(&self) -> Capabilities {
        Capabilities::build(
            self.card.model_id.clone(),
            self.card.epoch,
            self.card.fixture,
            self.encoder.is_some(),
            &self.card.thresholds,
            self.acoustic.clone(),
        )
    }

    pub const fn is_fixture(&self) -> bool {
        self.card.fixture
    }

    /// Blind detection. The only input is audio: no original, no expected payload, no offset hint,
    /// and no threshold. Everything that decides an accept is frozen in the model card.
    pub fn detect(&self, audio: &AudioBuffer) -> Result<NeuralDetection, GenoMarkNError> {
        validate(audio)?;
        let work = at_analysis_rate(audio)?;
        let mono = work.mono_sum();
        crate::detect::detect(&self.decoder, &self.analysis, &self.card.thresholds, &mono)
    }

    pub fn embed(
        &self,
        audio: &AudioBuffer,
        payload: Payload,
    ) -> Result<AudioBuffer, GenoMarkNError> {
        validate(audio)?;
        let encoder = self
            .encoder
            .as_ref()
            .ok_or(GenoMarkNError::EncoderUnavailable)?;
        crate::embed::embed(encoder, &self.analysis, &self.masking, audio, payload)
    }

    pub fn embed_bytes(
        &self,
        audio: &AudioBuffer,
        payload: &[u8],
    ) -> Result<AudioBuffer, GenoMarkNError> {
        if payload.len() != PAYLOAD_BYTES {
            return Err(GenoMarkNError::PayloadLength {
                found: payload.len(),
                expected: PAYLOAD_BYTES,
            });
        }
        self.embed(audio, Payload::from_bytes(payload)?)
    }
}

fn validate(audio: &AudioBuffer) -> Result<(), GenoMarkNError> {
    if audio.is_empty() {
        return Err(GenoMarkNError::Empty);
    }
    if audio.sample_rate() < MIN_SAMPLE_RATE {
        return Err(GenoMarkNError::SampleRateTooLow {
            found: audio.sample_rate(),
            min: MIN_SAMPLE_RATE,
        });
    }
    if audio.sample_rate() > MAX_SAMPLE_RATE {
        return Err(GenoMarkNError::SampleRateTooHigh {
            found: audio.sample_rate(),
            max: MAX_SAMPLE_RATE,
        });
    }
    for channel in 0..audio.channels() {
        let Some(plane) = audio.channel(channel) else {
            continue;
        };
        if let Some(frame) = plane.iter().position(|sample| !sample.is_finite()) {
            return Err(GenoMarkNError::NonFinite { channel, frame });
        }
    }
    Ok(())
}
