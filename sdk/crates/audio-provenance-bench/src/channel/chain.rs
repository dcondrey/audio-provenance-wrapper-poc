use super::{Channel, ChannelFamily, Params};
use crate::error::ChannelError;
use crate::ports::CommandRunner;
use audio_provenance_audio::AudioBuffer;

/// Channels applied in order, which is how audio actually reaches a listener: mastered, encoded,
/// played, captured, re-encoded.
#[derive(Debug)]
pub struct Chain {
    name: String,
    stages: Vec<Box<dyn Channel>>,
}

impl Chain {
    pub fn new(name: impl Into<String>, stages: Vec<Box<dyn Channel>>) -> Self {
        Self {
            name: name.into(),
            stages,
        }
    }
}

impl Channel for Chain {
    fn name(&self) -> &str {
        &self.name
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::Chain
    }

    fn params(&self) -> Params {
        let stages: Vec<serde_json::Value> = self
            .stages
            .iter()
            .map(|stage| {
                serde_json::json!({
                    "name": stage.name(),
                    "params": stage.params(),
                })
            })
            .collect();
        Params::from([("stages".to_owned(), serde_json::Value::Array(stages))])
    }

    fn required_programs(&self) -> Vec<String> {
        let mut programs: Vec<String> = self
            .stages
            .iter()
            .flat_map(|stage| stage.required_programs())
            .collect();
        programs.sort();
        programs.dedup();
        programs
    }

    fn is_simulated_physical_path(&self) -> bool {
        self.stages.iter().any(|s| s.is_simulated_physical_path())
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        seed: u64,
        runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        let mut current = audio.clone();
        for (index, stage) in self.stages.iter().enumerate() {
            let stage_seed = seed
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(index as u64 + 1);
            current = stage.apply(&current, stage_seed, runner)?;
        }
        Ok(current)
    }
}
