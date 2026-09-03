//! The GenoMark null test.
//!
//! Never embeds anything. It loads a corpus of never-marked works, pushes every one of them through
//! every channel in the matrix, and counts how often the detector returned a payload. The report it
//! writes is the artefact `audio-provenance verify --null-test` demands before a soft binding may reach
//! `verified`, so the gate's evidence and the gate's consumer are the same file.

use audio_provenance_bench::channel::codec::DEFAULT_FFMPEG;
use audio_provenance_bench::channel::matrix::default_matrix;
use audio_provenance_bench::corpus::{CorpusFeed, CorpusSpec, WavDirectoryCorpus};
use audio_provenance_bench::null::{NullConfig, NullCorpusProvenance, run_null};
use audio_provenance_bench::ports::FileStore;
use audio_provenance_bench::ports::native::{DiskFileStore, ProcessRunner};
use audio_provenance_bench::report::RowVerdict;
use audio_provenance_bench::watermark::WatermarkCodec;
use genomark::bench::GenoMarkCodec;

const USAGE: &str = "audio-provenance-null-test --corpus-dir DIR [--out-dir DIR] [--ffmpeg PATH]
                   [--seed N] [--workers N] [--duration SECONDS] [--limit N]
                   [--manifest PATH] [--label TEXT] [--quiet]

--corpus-dir holds WAV files that were NEVER marked. Every one of them is run through every
channel; nothing is embedded at any point. --limit caps the corpus for a pilot run and the cap is
recorded in the report, because a bound is only worth its trial count.";

#[derive(Debug)]
struct Options {
    corpus_dir: Option<String>,
    out_dir: String,
    ffmpeg: String,
    seed: u64,
    workers: usize,
    duration: f64,
    limit: usize,
    manifest: Option<String>,
    label: Option<String>,
    progress: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            corpus_dir: None,
            out_dir: "bench-out/null".to_owned(),
            ffmpeg: DEFAULT_FFMPEG.to_owned(),
            seed: 0x6765_6E6F_746F_6E65,
            workers: 1,
            duration: 30.0,
            limit: usize::MAX,
            manifest: None,
            label: None,
            progress: true,
        }
    }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("`{flag}` needs a value"));
        match flag.as_str() {
            "--corpus-dir" => options.corpus_dir = Some(value()?),
            "--out-dir" => options.out_dir = value()?,
            "--ffmpeg" => options.ffmpeg = value()?,
            "--manifest" => options.manifest = Some(value()?),
            "--label" => options.label = Some(value()?),
            "--seed" => {
                options.seed = value()?
                    .parse()
                    .map_err(|_| "--seed must be an unsigned integer".to_owned())?;
            }
            "--workers" => {
                options.workers = value()?
                    .parse()
                    .map_err(|_| "--workers must be a positive integer".to_owned())?;
            }
            "--duration" => {
                options.duration = value()?
                    .parse()
                    .map_err(|_| "--duration must be a number of seconds".to_owned())?;
            }
            "--limit" => {
                options.limit = value()?
                    .parse()
                    .map_err(|_| "--limit must be a positive integer".to_owned())?;
            }
            "--quiet" => options.progress = false,
            "-h" | "--help" => return Err(USAGE.to_owned()),
            other => return Err(format!("unrecognised argument `{other}`\n\n{USAGE}")),
        }
    }
    Ok(options)
}

fn main() -> std::process::ExitCode {
    let options = match parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return std::process::ExitCode::from(2);
        }
    };
    let Some(corpus_dir) = options.corpus_dir.clone() else {
        eprintln!("--corpus-dir is required\n\n{USAGE}");
        return std::process::ExitCode::from(2);
    };

    let runner = ProcessRunner::default();
    let store = DiskFileStore;
    let spec = CorpusSpec {
        duration_seconds: options.duration,
        max_real_seconds: options.duration,
        max_real_items: options.limit,
        ..CorpusSpec::default()
    };

    let corpus = match WavDirectoryCorpus::scan(&store, &corpus_dir, &spec) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("null corpus scan failed: {error}");
            return std::process::ExitCode::from(3);
        }
    };
    if corpus.metas().is_empty() {
        eprintln!("no .wav files under {corpus_dir}");
        return std::process::ExitCode::from(3);
    }

    let manifest_sha256 = options.manifest.as_ref().and_then(|path| {
        store.read(path).ok().map(|bytes| {
            let digest = audio_provenance_core::sha256_hex(&bytes);
            eprintln!("corpus manifest {path} sha256 {digest}");
            digest
        })
    });
    if options.manifest.is_some() && manifest_sha256.is_none() {
        eprintln!(
            "--manifest was given but could not be read; refusing to publish a bound whose corpus provenance is unverifiable"
        );
        return std::process::ExitCode::from(3);
    }

    let description = options.label.clone().unwrap_or_else(|| {
        format!(
            "{} never-marked works loaded from {corpus_dir}",
            corpus.metas().len()
        )
    });
    let config = NullConfig {
        seed: options.seed,
        workers: options.workers.max(1),
        max_false_positive_rate: 0.0,
        generated_at: std::env::var("AUDIO_PROVENANCE_BENCH_TIMESTAMP").ok(),
        provenance: NullCorpusProvenance {
            description,
            distinct_works: corpus.metas().len(),
            manifest_sha256,
        },
        progress: options.progress,
    };

    let codec = GenoMarkCodec::public();
    let matrix = default_matrix(&options.ffmpeg);
    eprintln!(
        "null test: {} works x {} channels = {} trials, {} workers",
        corpus.metas().len(),
        matrix.len(),
        corpus.metas().len() * matrix.len(),
        config.workers
    );

    let report = match run_null(&codec, &corpus, &matrix, &runner, &config) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("null test failed: {error}");
            return std::process::ExitCode::from(3);
        }
    };

    let table = report.to_table();
    print!("{table}");

    match report.to_json() {
        Ok(json) => {
            let path = format!("{}/{}-null-test.json", options.out_dir, codec.name());
            if let Err(error) = store.write(&path, json.as_bytes()) {
                eprintln!("could not write {path}: {error}");
                return std::process::ExitCode::from(4);
            }
            let table_path = format!("{}/{}-null-test.txt", options.out_dir, codec.name());
            if let Err(error) = store.write(&table_path, table.as_bytes()) {
                eprintln!("could not write {table_path}: {error}");
                return std::process::ExitCode::from(4);
            }
            eprintln!("wrote {path} and {table_path}");
        }
        Err(error) => {
            eprintln!("report serialisation failed: {error}");
            return std::process::ExitCode::from(4);
        }
    }

    if report.verdict == RowVerdict::Pass {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
