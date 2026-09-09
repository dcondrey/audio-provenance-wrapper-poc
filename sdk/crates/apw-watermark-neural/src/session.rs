use std::sync::Mutex;

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{Tensor, ValueType};

use crate::error::NeuralWatermarkError;
use crate::params::{BAND_BINS, MESSAGE_BITS};

/// A dimension the contract fixes, or the dynamic time axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dim {
    Fixed(i64),
    DynamicTime,
}

fn describe(dims: &[Dim]) -> String {
    let parts: Vec<String> = dims
        .iter()
        .map(|dim| match dim {
            Dim::Fixed(value) => value.to_string(),
            Dim::DynamicTime => "T".to_owned(),
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

fn build(graph: &'static str, bytes: &[u8]) -> Result<Session, NeuralWatermarkError> {
    let mut builder = Session::builder()
        .map_err(|error| NeuralWatermarkError::SessionBuild {
            graph,
            reason: error.to_string(),
        })?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|error| NeuralWatermarkError::SessionBuild {
            graph,
            reason: error.to_string(),
        })?
        .with_intra_threads(1)
        .map_err(|error| NeuralWatermarkError::SessionBuild {
            graph,
            reason: error.to_string(),
        })?;
    builder
        .commit_from_memory(bytes)
        .map_err(|error| NeuralWatermarkError::SessionBuild {
            graph,
            reason: error.to_string(),
        })
}

/// Refuses a graph whose declared tensor shape is not the one the model card's transform contract
/// implies. This is what turns "the contract is documented" into "the contract is enforced": a
/// PyTorch export that transposes frequency and time, or that bakes a fixed clip length into the
/// time axis, is rejected at load rather than producing plausible garbage at detect.
fn check_shape(
    graph: &'static str,
    outlets: &[ort::value::Outlet],
    name: &str,
    expected: &[Dim],
) -> Result<(), NeuralWatermarkError> {
    let outlet = outlets
        .iter()
        .find(|outlet| outlet.name() == name)
        .ok_or_else(|| NeuralWatermarkError::GraphIoMissing {
            graph,
            name: name.to_owned(),
        })?;
    let ValueType::Tensor { shape, .. } = outlet.dtype() else {
        return Err(NeuralWatermarkError::GraphShapeMismatch {
            graph,
            name: name.to_owned(),
            expected: describe(expected),
            found: "not a tensor".to_owned(),
        });
    };
    let dims: Vec<i64> = shape.iter().copied().collect();
    let matches = dims.len() == expected.len()
        && dims
            .iter()
            .zip(expected.iter())
            .all(|(found, want)| match want {
                Dim::Fixed(value) => found == value,
                Dim::DynamicTime => *found < 0,
            });
    if !matches {
        return Err(NeuralWatermarkError::GraphShapeMismatch {
            graph,
            name: name.to_owned(),
            expected: describe(expected),
            found: format!("{dims:?}"),
        });
    }
    Ok(())
}

/// The decoder. Fully convolutional over time and pooled inside the graph, so there is no offset
/// search here and no API that could accept one.
#[derive(Debug)]
pub struct DecoderSession {
    session: Mutex<Session>,
    input: String,
    presence: String,
    message: String,
}

pub struct DecoderOutput {
    /// One logit per STFT frame at 93.75 Hz, in the input's own time base.
    pub presence_logits: Vec<f32>,
    /// The 56 pooled bit logits. Sign is the hard decision, magnitude is the confidence the flip
    /// search orders by.
    pub message_logits: [f32; MESSAGE_BITS],
}

impl std::fmt::Debug for DecoderOutput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DecoderOutput")
            .field("frames", &self.presence_logits.len())
            .finish_non_exhaustive()
    }
}

impl DecoderSession {
    pub fn new(
        bytes: &[u8],
        input: &str,
        presence: &str,
        message: &str,
    ) -> Result<Self, NeuralWatermarkError> {
        let session = build("decoder", bytes)?;
        check_shape(
            "decoder",
            session.inputs(),
            input,
            &[
                Dim::Fixed(1),
                Dim::Fixed(1),
                Dim::Fixed(BAND_BINS as i64),
                Dim::DynamicTime,
            ],
        )?;
        check_shape(
            "decoder",
            session.outputs(),
            presence,
            &[Dim::Fixed(1), Dim::DynamicTime],
        )?;
        check_shape(
            "decoder",
            session.outputs(),
            message,
            &[Dim::Fixed(1), Dim::Fixed(MESSAGE_BITS as i64)],
        )?;
        Ok(Self {
            session: Mutex::new(session),
            input: input.to_owned(),
            presence: presence.to_owned(),
            message: message.to_owned(),
        })
    }

    /// `log_mag` is `[1, 1, 320, time]` frequency-major, exactly as
    /// [`crate::spectral::Analysis::band_log_magnitude`] lays it out.
    pub fn run(&self, log_mag: &[f32], time: usize) -> Result<DecoderOutput, NeuralWatermarkError> {
        let tensor = Tensor::from_array((
            vec![1i64, 1, BAND_BINS as i64, time as i64],
            log_mag.to_vec(),
        ))
        .map_err(|error| NeuralWatermarkError::Inference {
            graph: "decoder",
            reason: error.to_string(),
        })?;
        let mut guard = self
            .session
            .lock()
            .map_err(|_| NeuralWatermarkError::SessionPoisoned { graph: "decoder" })?;
        let outputs = guard
            .run(ort::inputs![self.input.as_str() => tensor])
            .map_err(|error| NeuralWatermarkError::Inference {
                graph: "decoder",
                reason: error.to_string(),
            })?;
        let presence_logits = extract("decoder", &outputs, &self.presence)?.to_vec();
        let message_slice = extract("decoder", &outputs, &self.message)?;
        let message_logits: [f32; MESSAGE_BITS] =
            message_slice
                .try_into()
                .map_err(|_| NeuralWatermarkError::GraphShapeMismatch {
                    graph: "decoder",
                    name: self.message.clone(),
                    expected: format!("[1, {MESSAGE_BITS}]"),
                    found: format!("{} values", message_slice.len()),
                })?;
        if presence_logits.len() != time {
            return Err(NeuralWatermarkError::GraphShapeMismatch {
                graph: "decoder",
                name: self.presence.clone(),
                expected: format!("[1, {time}]"),
                found: format!("[1, {}]", presence_logits.len()),
            });
        }
        Ok(DecoderOutput {
            presence_logits,
            message_logits,
        })
    }
}

/// The encoder. Emits the raw per-bin direction; the perceptual budget, not the network, decides
/// absolute magnitude (spec 2.5).
#[derive(Debug)]
pub struct EncoderSession {
    session: Mutex<Session>,
    log_mag_input: String,
    message_input: String,
    output: String,
}

impl EncoderSession {
    pub fn new(
        bytes: &[u8],
        log_mag_input: &str,
        message_input: &str,
        output: &str,
    ) -> Result<Self, NeuralWatermarkError> {
        let session = build("encoder", bytes)?;
        let spectrogram = [
            Dim::Fixed(1),
            Dim::Fixed(1),
            Dim::Fixed(BAND_BINS as i64),
            Dim::DynamicTime,
        ];
        check_shape("encoder", session.inputs(), log_mag_input, &spectrogram)?;
        check_shape(
            "encoder",
            session.inputs(),
            message_input,
            &[Dim::Fixed(1), Dim::Fixed(MESSAGE_BITS as i64)],
        )?;
        check_shape("encoder", session.outputs(), output, &spectrogram)?;
        Ok(Self {
            session: Mutex::new(session),
            log_mag_input: log_mag_input.to_owned(),
            message_input: message_input.to_owned(),
            output: output.to_owned(),
        })
    }

    pub fn run(
        &self,
        log_mag: &[f32],
        time: usize,
        message: &[u8; MESSAGE_BITS],
    ) -> Result<Vec<f32>, NeuralWatermarkError> {
        let spectrogram = Tensor::from_array((
            vec![1i64, 1, BAND_BINS as i64, time as i64],
            log_mag.to_vec(),
        ))
        .map_err(|error| NeuralWatermarkError::Inference {
            graph: "encoder",
            reason: error.to_string(),
        })?;
        let bits: Vec<f32> = message.iter().map(|bit| f32::from(*bit)).collect();
        let message_tensor =
            Tensor::from_array((vec![1i64, MESSAGE_BITS as i64], bits)).map_err(|error| {
                NeuralWatermarkError::Inference {
                    graph: "encoder",
                    reason: error.to_string(),
                }
            })?;
        let mut guard = self
            .session
            .lock()
            .map_err(|_| NeuralWatermarkError::SessionPoisoned { graph: "encoder" })?;
        let outputs = guard
            .run(ort::inputs![
                self.log_mag_input.as_str() => spectrogram,
                self.message_input.as_str() => message_tensor,
            ])
            .map_err(|error| NeuralWatermarkError::Inference {
                graph: "encoder",
                reason: error.to_string(),
            })?;
        let gain = extract("encoder", &outputs, &self.output)?;
        if gain.len() != BAND_BINS * time {
            return Err(NeuralWatermarkError::GraphShapeMismatch {
                graph: "encoder",
                name: self.output.clone(),
                expected: format!("[1, 1, {BAND_BINS}, {time}]"),
                found: format!("{} values", gain.len()),
            });
        }
        Ok(gain.to_vec())
    }
}

fn extract<'a>(
    graph: &'static str,
    outputs: &'a ort::session::SessionOutputs<'_>,
    name: &str,
) -> Result<&'a [f32], NeuralWatermarkError> {
    let value = outputs
        .get(name)
        .ok_or_else(|| NeuralWatermarkError::GraphIoMissing {
            graph,
            name: name.to_owned(),
        })?;
    let (_, data) =
        value
            .try_extract_tensor::<f32>()
            .map_err(|error| NeuralWatermarkError::Inference {
                graph,
                reason: error.to_string(),
            })?;
    Ok(data)
}
