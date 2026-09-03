use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::envelope::EnvelopeClaim;
use crate::error::GenoMarkNError;
use crate::params::{
    ALGORITHM_ID, ANALYSIS_SAMPLE_RATE, BAND_BIN_HIGH, BAND_BIN_LOW, CARD_FORMAT, HOP, LOG_FLOOR,
    MAX_BUDGET_NEPERS, N_FFT,
};
use crate::thresholds::Thresholds;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct GraphRef {
    pub file: String,
    pub sha256: String,
}

/// The STFT contract, restated in the card so a Python export and a Rust load disagree loudly
/// rather than silently. Every field is checked against this build's constants at load time.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct TransformCard {
    pub sample_rate_hz: u32,
    pub n_fft: usize,
    pub hop: usize,
    pub window: String,
    pub lead_pad_samples: usize,
    pub band_bin_low: usize,
    pub band_bin_high: usize,
    pub magnitude: String,
    pub log_floor: f64,
    pub tensor_layout: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct IoCard {
    pub decoder_input: String,
    pub decoder_presence_output: String,
    pub decoder_message_output: String,
    pub encoder_log_mag_input: String,
    pub encoder_message_input: String,
    pub encoder_output: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct BudgetCard {
    /// Scales the masking-threshold-derived per-bin ceiling. Calibrated in build unit N-C0.
    pub kappa: f64,
    pub max_nepers: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct GraphSet {
    pub decoder: GraphRef,
    pub encoder: Option<GraphRef>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ModelCard {
    pub card_format: String,
    pub algorithm_id: String,
    pub model_id: String,
    pub epoch: u32,
    #[serde(default)]
    pub fixture: bool,
    pub graphs: GraphSet,
    pub transform: TransformCard,
    pub io: IoCard,
    pub budget: BudgetCard,
    pub thresholds: Thresholds,
    #[serde(default)]
    pub training_provenance: serde_json::Value,
    pub operating_envelope: Option<EnvelopeClaim>,
}

fn mismatch(field: &'static str, expected: impl ToString, found: impl ToString) -> GenoMarkNError {
    GenoMarkNError::TransformMismatch {
        field,
        expected: expected.to_string(),
        found: found.to_string(),
    }
}

impl ModelCard {
    pub fn read(path: &Path) -> Result<Self, GenoMarkNError> {
        let bytes = std::fs::read(path).map_err(|error| GenoMarkNError::CardUnreadable {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
        let card: Self =
            serde_json::from_slice(&bytes).map_err(|error| GenoMarkNError::CardMalformed {
                path: path.to_path_buf(),
                reason: error.to_string(),
            })?;
        card.validate()?;
        Ok(card)
    }

    fn validate(&self) -> Result<(), GenoMarkNError> {
        if self.card_format != CARD_FORMAT {
            return Err(GenoMarkNError::CardFormat {
                found: self.card_format.clone(),
                expected: CARD_FORMAT,
            });
        }
        if self.algorithm_id != ALGORITHM_ID {
            return Err(GenoMarkNError::AlgorithmMismatch {
                found: self.algorithm_id.clone(),
                expected: ALGORITHM_ID,
            });
        }
        if self.fixture && self.operating_envelope.is_some() {
            return Err(GenoMarkNError::FixtureCarriesEnvelope);
        }
        let t = &self.transform;
        if t.sample_rate_hz != ANALYSIS_SAMPLE_RATE {
            return Err(mismatch(
                "sample_rate_hz",
                ANALYSIS_SAMPLE_RATE,
                t.sample_rate_hz,
            ));
        }
        if t.n_fft != N_FFT {
            return Err(mismatch("n_fft", N_FFT, t.n_fft));
        }
        if t.hop != HOP {
            return Err(mismatch("hop", HOP, t.hop));
        }
        if t.window != "sqrt_hann_periodic" {
            return Err(mismatch("window", "sqrt_hann_periodic", &t.window));
        }
        if t.lead_pad_samples != N_FFT {
            return Err(mismatch("lead_pad_samples", N_FFT, t.lead_pad_samples));
        }
        if t.band_bin_low != BAND_BIN_LOW {
            return Err(mismatch("band_bin_low", BAND_BIN_LOW, t.band_bin_low));
        }
        if t.band_bin_high != BAND_BIN_HIGH {
            return Err(mismatch("band_bin_high", BAND_BIN_HIGH, t.band_bin_high));
        }
        if t.magnitude != "natural_log" {
            return Err(mismatch("magnitude", "natural_log", &t.magnitude));
        }
        if t.tensor_layout != "nchw_frequency_major" {
            return Err(mismatch(
                "tensor_layout",
                "nchw_frequency_major",
                &t.tensor_layout,
            ));
        }
        if (t.log_floor - f64::from(LOG_FLOOR)).abs() > f64::from(LOG_FLOOR) * 1e-3 {
            return Err(mismatch("log_floor", LOG_FLOOR, t.log_floor));
        }
        if !self.budget.kappa.is_finite() || self.budget.kappa <= 0.0 {
            return Err(GenoMarkNError::ThresholdRange {
                field: "budget.kappa",
                found: self.budget.kappa,
                range: "finite and greater than zero",
            });
        }
        if !self.budget.max_nepers.is_finite()
            || self.budget.max_nepers <= 0.0
            || self.budget.max_nepers > MAX_BUDGET_NEPERS
        {
            return Err(GenoMarkNError::ThresholdRange {
                field: "budget.max_nepers",
                found: self.budget.max_nepers,
                range: "the (0, 0.35] nepers spec 2.7 caps at; no calibration may raise it",
            });
        }
        self.thresholds.validate()
    }

    /// Reads a graph file and refuses it unless its SHA-256 matches the digest the card pins.
    pub fn read_graph(
        &self,
        directory: &Path,
        graph: &'static str,
        reference: &GraphRef,
    ) -> Result<Vec<u8>, GenoMarkNError> {
        let path = resolve(directory, &reference.file)?;
        let bytes = std::fs::read(&path).map_err(|error| GenoMarkNError::GraphUnreadable {
            path: path.clone(),
            reason: error.to_string(),
        })?;
        let found = hex::encode(Sha256::digest(&bytes));
        if !found.eq_ignore_ascii_case(&reference.sha256) {
            return Err(GenoMarkNError::GraphHashMismatch {
                graph,
                expected: reference.sha256.clone(),
                found,
            });
        }
        Ok(bytes)
    }
}

/// Graph paths are resolved against the card's own directory, and the card is untrusted input.
/// A file name that is absolute, rooted, or climbs out with `..` is refused rather than followed:
/// the digest pin means a swapped file cannot load anyway, but a loader that reads whatever path a
/// card names is a boundary this crate does not need to have.
fn resolve(directory: &Path, file: &str) -> Result<PathBuf, GenoMarkNError> {
    let candidate = Path::new(file);
    let safe = candidate.components().all(|component| {
        matches!(
            component,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    });
    if !safe || file.is_empty() {
        return Err(GenoMarkNError::GraphPathRejected {
            file: file.to_owned(),
        });
    }
    Ok(directory.join(candidate))
}
