pub mod acoustic;
pub mod chain;
pub mod codec;
pub mod matrix;
pub mod native;

use crate::error::ChannelError;
use crate::ports::CommandRunner;
use audio_provenance_audio::AudioBuffer;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Debug;

pub type Params = BTreeMap<String, serde_json::Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelFamily {
    Control,
    LossyCodec,
    Sampling,
    Level,
    Editing,
    Noise,
    Bandwidth,
    Dynamics,
    SimulatedAcoustic,
    Chain,
}

/// A degradation an audio file plausibly survives between an embedder and a detector.
///
/// `apply` is a pure function of `(audio, seed)` plus whatever the runner does, so a row is
/// reproducible from the `(track, channel, seed)` triple alone.
pub trait Channel: Debug + Send + Sync {
    fn name(&self) -> &str;

    fn family(&self) -> ChannelFamily;

    fn params(&self) -> Params;

    fn apply(
        &self,
        audio: &AudioBuffer,
        seed: u64,
        runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError>;

    /// External programs without which this channel cannot run. A missing one produces an explicit
    /// error row; it never removes the row from the matrix.
    fn required_programs(&self) -> Vec<String> {
        Vec::new()
    }

    /// True when the channel models a physical path in software. Such a row is not a measurement of
    /// that physical path and the report must say so next to the number.
    fn is_simulated_physical_path(&self) -> bool {
        false
    }
}

pub(crate) fn param(key: &str, value: impl Into<serde_json::Value>) -> (String, serde_json::Value) {
    (key.to_owned(), value.into())
}
