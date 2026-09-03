use super::{Channel, ChannelFamily, Params, param};
use crate::audio::{from_f32le_bytes, to_f32le_bytes};
use crate::error::{ChannelError, PortError};
use crate::ports::CommandRunner;
use audio_provenance_audio::AudioBuffer;
use serde::Serialize;

pub const DEFAULT_FFMPEG: &str = "/opt/homebrew/bin/ffmpeg";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LossyFormat {
    Mp3,
    AacLc,
    Opus,
}

impl LossyFormat {
    /// The encoder is part of the channel's identity: ffmpeg ships both a native `aac` and an
    /// `aac_at` AudioToolbox encoder, and they are different channels.
    pub const fn encoder(self) -> &'static str {
        match self {
            Self::Mp3 => "libmp3lame",
            Self::AacLc => "aac",
            Self::Opus => "libopus",
        }
    }

    pub const fn mux_format(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::AacLc => "adts",
            Self::Opus => "ogg",
        }
    }

    pub const fn demux_format(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::AacLc => "aac",
            Self::Opus => "ogg",
        }
    }

    pub const fn note(self) -> Option<&'static str> {
        match self {
            Self::Opus => Some(
                "libopus encodes at 48 kHz regardless of input rate; the decode leg resamples back \
                 to the corpus rate, so this row includes one extra sample-rate conversion.",
            ),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LossyCodec {
    name: String,
    format: LossyFormat,
    bitrate_kbps: u32,
    program: String,
}

impl LossyCodec {
    pub fn new(
        name: impl Into<String>,
        format: LossyFormat,
        bitrate_kbps: u32,
        program: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            format,
            bitrate_kbps,
            program: program.into(),
        }
    }
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

impl Channel for LossyCodec {
    fn name(&self) -> &str {
        &self.name
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::LossyCodec
    }

    fn params(&self) -> Params {
        let mut params = Params::from([
            param(
                "format",
                serde_json::to_value(self.format).unwrap_or_default(),
            ),
            param("encoder", self.format.encoder()),
            param("mux_format", self.format.mux_format()),
            param("bitrate_kbps", self.bitrate_kbps),
            param("program", self.program.clone()),
        ]);
        if let Some(note) = self.format.note() {
            params.insert("note".to_owned(), note.into());
        }
        params
    }

    fn required_programs(&self) -> Vec<String> {
        vec![self.program.clone()]
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        _seed: u64,
        runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        let rate = audio.sample_rate().to_string();
        let channels = audio.channels().to_string();
        let bitrate = format!("{}k", self.bitrate_kbps);

        let encode = args(&[
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-f",
            "f32le",
            "-ar",
            &rate,
            "-ac",
            &channels,
            "-i",
            "pipe:0",
            "-c:a",
            self.format.encoder(),
            "-b:a",
            &bitrate,
            "-f",
            self.format.mux_format(),
            "pipe:1",
        ]);
        let encoded = runner.run(&self.program, &encode, &to_f32le_bytes(audio))?;
        if encoded.status != 0 || encoded.stdout.is_empty() {
            return Err(ChannelError::Port(PortError::Status {
                program: self.program.clone(),
                status: encoded.status,
                stderr: if encoded.stderr.is_empty() {
                    "encoder produced no output".to_owned()
                } else {
                    encoded.stderr
                },
            }));
        }

        let decode = args(&[
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-f",
            self.format.demux_format(),
            "-i",
            "pipe:0",
            "-f",
            "f32le",
            "-ar",
            &rate,
            "-ac",
            &channels,
            "pipe:1",
        ]);
        let decoded = runner.run(&self.program, &decode, &encoded.stdout)?;
        if decoded.status != 0 || decoded.stdout.is_empty() {
            return Err(ChannelError::Port(PortError::Status {
                program: self.program.clone(),
                status: decoded.status,
                stderr: if decoded.stderr.is_empty() {
                    "decoder produced no output".to_owned()
                } else {
                    decoded.stderr
                },
            }));
        }

        let frame_bytes = audio.channels() * 4;
        if decoded.stdout.len() % frame_bytes != 0 {
            return Err(ChannelError::RaggedDecode {
                bytes: decoded.stdout.len(),
                channels: audio.channels(),
            });
        }
        Ok(from_f32le_bytes(
            &decoded.stdout,
            audio.sample_rate(),
            audio.channels(),
        )?)
    }
}
