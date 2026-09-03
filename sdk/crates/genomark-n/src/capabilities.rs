use crate::envelope::AcousticEnvelope;
use crate::params::{
    ALGORITHM_ID, BAND_BIN_HIGH, BAND_BIN_LOW, MAX_SAMPLE_RATE, MESSAGE_BITS, MIN_SAMPLE_RATE,
    N_FFT, PAYLOAD_BITS, UNSUPPORTED,
};
use crate::thresholds::Thresholds;

/// What `capabilities().acousticRerecording` may be.
///
/// IMPORTANT: `MeasuredLimited` holds an [`AcousticEnvelope`], which has no public constructor.
/// A build with no validated calibration artifact cannot construct this variant at all, so it is
/// structurally incapable of reporting anything but the literal `"unsupported"`. That is the
/// type-level version of GENOMARK_N_SPEC.md section 8.6's rule, and it is why the field is never a
/// boolean: a boolean is what lets marketing outrun engineering.
#[derive(Debug, Clone, PartialEq)]
pub enum AcousticRerecording {
    Unsupported,
    MeasuredLimited(AcousticEnvelope),
}

impl AcousticRerecording {
    pub fn status(&self) -> &'static str {
        match self {
            Self::Unsupported => UNSUPPORTED,
            Self::MeasuredLimited(envelope) => envelope.status(),
        }
    }

    pub const fn envelope(&self) -> Option<&AcousticEnvelope> {
        match self {
            Self::Unsupported => None,
            Self::MeasuredLimited(envelope) => Some(envelope),
        }
    }

    pub const fn is_supported(&self) -> bool {
        matches!(self, Self::MeasuredLimited(_))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Capabilities {
    pub algorithm: &'static str,
    pub model_id: String,
    pub epoch: u32,
    /// True when the loaded graphs are the plumbing fixture rather than a trained model. A fixture
    /// detects nothing; every rate it produces is an artifact of arithmetic.
    pub is_fixture: bool,
    pub payload_bits: usize,
    pub message_bits: usize,
    pub sample_rate_range: (u32, u32),
    pub band_hz: (f64, f64),
    pub acoustic_rerecording: AcousticRerecording,
    /// The window the presence tier integrates over, and the shortest capture it will read.
    pub presence_window_s: f64,
    pub min_presence_s: f64,
    /// Reported, never assumed: the SDK refuses a locator read below this rather than returning a
    /// low-confidence guess (spec 4.6).
    pub min_locator_s: f64,
    pub locator_window_s: f64,
    /// The encoder graph is optional. Without it the crate detects and cannot embed.
    pub can_embed: bool,
    /// Playback speed or pitch changes beyond this fraction break the learned features' frequency
    /// alignment. There is no rate grid and none is searched (spec 4.7).
    pub rate_tolerance: f64,
    pub time_stretch: &'static str,
}

pub const RATE_TOLERANCE: f64 = 0.005;

impl Capabilities {
    pub(crate) fn build(
        model_id: String,
        epoch: u32,
        is_fixture: bool,
        can_embed: bool,
        thresholds: &Thresholds,
        acoustic_rerecording: AcousticRerecording,
    ) -> Self {
        let bin_hz = f64::from(crate::params::ANALYSIS_SAMPLE_RATE) / N_FFT as f64;
        Self {
            algorithm: ALGORITHM_ID,
            model_id,
            epoch,
            is_fixture,
            payload_bits: PAYLOAD_BITS,
            message_bits: MESSAGE_BITS,
            sample_rate_range: (MIN_SAMPLE_RATE, MAX_SAMPLE_RATE),
            band_hz: (BAND_BIN_LOW as f64 * bin_hz, BAND_BIN_HIGH as f64 * bin_hz),
            acoustic_rerecording,
            presence_window_s: thresholds.presence_window_seconds,
            min_presence_s: thresholds.min_presence_seconds,
            min_locator_s: thresholds.min_locator_seconds,
            locator_window_s: thresholds.locator_window_seconds,
            can_embed,
            rate_tolerance: RATE_TOLERANCE,
            time_stretch: UNSUPPORTED,
        }
    }
}
