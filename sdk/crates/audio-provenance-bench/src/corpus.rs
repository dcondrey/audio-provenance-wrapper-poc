use crate::audio::{self, decode_wav, from_channels, peak, to_f32le_bytes};
use crate::error::{AudioError, BenchError};
use crate::ports::{CommandRunner, FileStore};
use audio_provenance_audio::AudioBuffer;
use audio_provenance_audio::resample::resample;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentClass {
    SineSweep,
    WhiteNoise,
    PinkNoise,
    Transient,
    HarmonicPad,
    NearSilence,
    SquareWave,
    RealSession,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CorpusSource {
    Synthetic { generator: String, recipe: String },
    File { path: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct CorpusItemMeta {
    pub id: String,
    pub class: ContentClass,
    pub source: CorpusSource,
    pub sample_rate: u32,
    pub channels: usize,
    pub frames: usize,
    pub duration_seconds: f64,
    pub peak_dbfs: f64,
    /// SHA-256 of the exact f32 little-endian PCM the bench measured, so a row reproduces from its
    /// (track, channel, seed) triple rather than from a filename that may have changed underneath.
    pub pcm_sha256: String,
}

#[derive(Debug, Clone)]
pub struct CorpusItem {
    pub meta: CorpusItemMeta,
    pub audio: AudioBuffer,
}

impl CorpusItem {
    /// IMPORTANT: admission runs here, not only in the loaders. `AudioBuffer` is the permissive
    /// substrate type and will happily hold a NaN or a zero-frame buffer, and anything that reaches
    /// this constructor is about to be measured and printed as a recovery rate. This is the only
    /// way into the corpus, so it is the place the policy has to bind.
    pub fn new(
        id: impl Into<String>,
        class: ContentClass,
        source: CorpusSource,
        audio: AudioBuffer,
    ) -> Result<Self, AudioError> {
        let audio = audio::admit(audio)?;
        let level = peak(&audio);
        let meta = CorpusItemMeta {
            id: id.into(),
            class,
            source,
            sample_rate: audio.sample_rate(),
            channels: audio.channels(),
            frames: audio.frames(),
            duration_seconds: audio.duration_seconds(),
            peak_dbfs: if level > 0.0 {
                20.0 * f64::from(level).log10()
            } else {
                f64::NEG_INFINITY
            },
            pcm_sha256: audio_provenance_core::sha256_hex(&to_f32le_bytes(&audio)),
        };
        Ok(Self { meta, audio })
    }
}

#[derive(Debug, Clone)]
pub struct CorpusSpec {
    pub sample_rate: u32,
    pub channels: usize,
    pub duration_seconds: f64,
    pub max_real_items: usize,
    pub max_real_seconds: f64,
}

impl Default for CorpusSpec {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            duration_seconds: 12.0,
            max_real_items: 4,
            max_real_seconds: 24.0,
        }
    }
}

fn escape(expression: &str) -> String {
    expression.replace(',', "\\,")
}

fn stereo_source(left: &str, right: &str, spec: &CorpusSpec) -> String {
    format!(
        "aevalsrc=exprs={}|{}:c=stereo:s={}:d={}",
        escape(left),
        escape(right),
        spec.sample_rate,
        spec.duration_seconds
    )
}

#[derive(Debug, Clone)]
struct Recipe {
    id: &'static str,
    class: ContentClass,
    filter: String,
}

fn recipes(spec: &CorpusSpec) -> Vec<Recipe> {
    let duration = spec.duration_seconds.max(1.0);
    let sweep_k = (16_000f64 / 40.0).ln();
    let sweep = format!(
        "0.5*sin(2*PI*40*{duration}/{sweep_k}*(exp(t/{duration}*{sweep_k})-1))",
        duration = duration,
        sweep_k = sweep_k
    );
    let drum = "0.85*exp(-28*mod(t,0.5))*sin(2*PI*58*mod(t,0.5))\
                +0.45*exp(-14*mod(t,1.0))*sin(2*PI*185*mod(t,1.0))\
                +0.22*exp(-140*mod(t,0.25))*(2*random(2)-1)"
        .to_owned();
    let pad_left = "0.24*(sin(2*PI*110*t)+0.5*sin(2*PI*220*t)+0.33*sin(2*PI*330.5*t)\
                    +0.25*sin(2*PI*440*t)+0.18*sin(2*PI*661*t))*(0.7+0.3*sin(2*PI*0.31*t))"
        .to_owned();
    let pad_right = "0.24*(sin(2*PI*110.3*t)+0.5*sin(2*PI*219.4*t)+0.33*sin(2*PI*331*t)\
                     +0.25*sin(2*PI*441.2*t)+0.18*sin(2*PI*659*t))*(0.7+0.3*sin(2*PI*0.27*t))"
        .to_owned();

    vec![
        Recipe {
            id: "synth_sine_sweep",
            class: ContentClass::SineSweep,
            filter: stereo_source(&sweep, &sweep, spec),
        },
        Recipe {
            id: "synth_white_noise",
            class: ContentClass::WhiteNoise,
            filter: stereo_source("0.35*(2*random(0)-1)", "0.35*(2*random(11)-1)", spec),
        },
        Recipe {
            id: "synth_pink_noise",
            class: ContentClass::PinkNoise,
            filter: format!(
                "anoisesrc=color=pink:seed=7:amplitude=0.4:r={}:d={}",
                spec.sample_rate, spec.duration_seconds
            ),
        },
        Recipe {
            id: "synth_transient_pattern",
            class: ContentClass::Transient,
            filter: stereo_source(&drum, &drum, spec),
        },
        Recipe {
            id: "synth_harmonic_pad",
            class: ContentClass::HarmonicPad,
            filter: stereo_source(&pad_left, &pad_right, spec),
        },
        Recipe {
            id: "synth_near_silence",
            class: ContentClass::NearSilence,
            filter: stereo_source(
                "0.0005*sin(2*PI*300*t)+0.0002*(2*random(3)-1)",
                "0.0005*sin(2*PI*301*t)+0.0002*(2*random(4)-1)",
                spec,
            ),
        },
        Recipe {
            id: "synth_square_full_scale",
            class: ContentClass::SquareWave,
            filter: stereo_source(
                "if(gt(sin(2*PI*110*t),0),1,-1)",
                "if(gt(sin(2*PI*110*t),0),1,-1)",
                spec,
            ),
        },
    ]
}

pub fn generate_synthetic(
    runner: &dyn CommandRunner,
    program: &str,
    spec: &CorpusSpec,
) -> Result<Vec<CorpusItem>, BenchError> {
    let mut items = Vec::new();
    for recipe in recipes(spec) {
        let args: Vec<String> = [
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            &recipe.filter,
            "-ac",
            &spec.channels.to_string(),
            "-ar",
            &spec.sample_rate.to_string(),
            "-f",
            "f32le",
            "pipe:1",
        ]
        .iter()
        .map(|v| (*v).to_owned())
        .collect();
        let output = runner.run(program, &args, &[])?;
        if output.status != 0 || output.stdout.is_empty() {
            return Err(BenchError::Port(crate::error::PortError::Status {
                program: program.to_owned(),
                status: output.status,
                stderr: if output.stderr.is_empty() {
                    format!("`{}` produced no audio", recipe.id)
                } else {
                    output.stderr
                },
            }));
        }
        let audio = audio::from_f32le_bytes(&output.stdout, spec.sample_rate, spec.channels)?;
        items.push(CorpusItem::new(
            recipe.id,
            recipe.class,
            CorpusSource::Synthetic {
                generator: program.to_owned(),
                recipe: recipe.filter.clone(),
            },
            audio,
        )?);
    }
    Ok(items)
}

fn conform(audio: &AudioBuffer, spec: &CorpusSpec) -> Result<AudioBuffer, BenchError> {
    let converted = resample(audio, spec.sample_rate).map_err(AudioError::from)?;
    let limit = ((spec.max_real_seconds * f64::from(spec.sample_rate)).round() as usize)
        .max(1)
        .min(converted.frames());
    let mut wanted: Vec<Vec<f32>> = Vec::with_capacity(spec.channels);
    for channel in 0..spec.channels {
        // A file with fewer channels than the working layout is duplicated up from its first,
        // which is what the source field records; it is never zero-padded into silence.
        let plane = converted
            .channel(channel)
            .or_else(|| converted.channel(0))
            .ok_or(AudioError::Empty)?;
        wanted.push(plane[..limit].to_vec());
    }
    Ok(from_channels(spec.sample_rate, &wanted)?)
}

/// Loads real WAV files and conforms them to the working rate and channel count. The conversion is
/// recorded in the item source so a row is never read as a measurement on the untouched file.
pub fn load_wav_directory(
    store: &dyn FileStore,
    directory: &str,
    spec: &CorpusSpec,
) -> Result<Vec<CorpusItem>, BenchError> {
    let paths = store.list_files(directory, "wav")?;
    let mut items = Vec::new();
    for path in paths.into_iter().take(spec.max_real_items) {
        items.push(load_wav_path(store, &path, spec)?);
    }
    Ok(items)
}

/// One WAV, conformed to the working layout. The single place the id and the source field are
/// derived, so a streamed corpus and a preloaded one cannot disagree about what an item is called.
pub fn load_wav_path(
    store: &dyn FileStore,
    path: &str,
    spec: &CorpusSpec,
) -> Result<CorpusItem, BenchError> {
    let bytes = store.read(path)?;
    let decoded = decode_wav(&bytes)?;
    let conformed = conform(&decoded, spec)?;
    let stem = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".wav")
        .to_owned();
    Ok(CorpusItem::new(
        format!("real_{stem}"),
        ContentClass::RealSession,
        CorpusSource::File {
            path: path.to_owned(),
        },
        conformed,
    )?)
}

/// Loads named WAV files, conforming each the same way `load_wav_directory` does.
pub fn load_wav_files(
    store: &dyn FileStore,
    paths: &[String],
    spec: &CorpusSpec,
) -> Result<Vec<CorpusItem>, BenchError> {
    let mut items = Vec::new();
    for path in paths.iter().take(spec.max_real_items) {
        items.push(load_wav_path(store, path, spec)?);
    }
    Ok(items)
}

/// A corpus the runner materialises one item at a time.
///
/// A thousand 30 s stereo works is 11.5 GB of f32 planes held at once, which on this machine put
/// 27 GB into swap and turned every detection time into a measurement of the pager. Holding only
/// the items currently in flight costs one WAV decode per cell and bounds resident audio at the
/// worker count.
pub trait CorpusFeed: std::fmt::Debug + Send + Sync {
    fn metas(&self) -> &[CorpusItemMeta];
    fn item(&self, index: usize) -> Result<CorpusItem, BenchError>;
}

/// Every item already in memory. The metadata is the items' own, so a preloaded corpus and a
/// streamed one produce identical reports.
#[derive(Debug)]
pub struct PreloadedCorpus {
    items: Vec<CorpusItem>,
    metas: Vec<CorpusItemMeta>,
}

impl PreloadedCorpus {
    pub fn new(items: Vec<CorpusItem>) -> Self {
        let metas = items.iter().map(|item| item.meta.clone()).collect();
        Self { items, metas }
    }
}

impl CorpusFeed for PreloadedCorpus {
    fn metas(&self) -> &[CorpusItemMeta] {
        &self.metas
    }

    fn item(&self, index: usize) -> Result<CorpusItem, BenchError> {
        self.items
            .get(index)
            .cloned()
            .ok_or(BenchError::Audio(AudioError::Empty))
    }
}

/// A directory of WAVs, decoded on demand.
#[derive(Debug)]
pub struct WavDirectoryCorpus<'a> {
    store: &'a dyn FileStore,
    paths: Vec<String>,
    spec: CorpusSpec,
    metas: Vec<CorpusItemMeta>,
}

impl<'a> WavDirectoryCorpus<'a> {
    /// Reads every file once to fix its identity and its PCM digest, then drops the audio. The
    /// scan is what makes a row reproducible from `(track, channel, seed)`; without it the report
    /// could not name what it measured.
    pub fn scan(
        store: &'a dyn FileStore,
        directory: &str,
        spec: &CorpusSpec,
    ) -> Result<Self, BenchError> {
        let paths: Vec<String> = store
            .list_files(directory, "wav")?
            .into_iter()
            .take(spec.max_real_items)
            .collect();
        let mut metas = Vec::with_capacity(paths.len());
        for path in &paths {
            metas.push(load_wav_path(store, path, spec)?.meta);
        }
        Ok(Self {
            store,
            paths,
            spec: spec.clone(),
            metas,
        })
    }
}

impl CorpusFeed for WavDirectoryCorpus<'_> {
    fn metas(&self) -> &[CorpusItemMeta] {
        &self.metas
    }

    fn item(&self, index: usize) -> Result<CorpusItem, BenchError> {
        let path = self
            .paths
            .get(index)
            .ok_or(BenchError::Audio(AudioError::Empty))?;
        load_wav_path(self.store, path, &self.spec)
    }
}
