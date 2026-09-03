//! The marked bench over real recorded music, run beside the synthetic corpus in one report.
//!
//! `genomark-bench` takes real files through `--real-wav`, but `CorpusSpec::max_real_items` caps
//! that at four and the loader takes rather than rejects, so a larger real corpus disappears
//! without an error. This binary exists to run the same measurement over a whole directory with the
//! cap set from the directory itself, and to keep the synthetic items in the same report so the
//! synthetic-to-real delta is a within-report comparison rather than a comparison between two runs.

use audio_provenance_bench::channel::codec::DEFAULT_FFMPEG;
use audio_provenance_bench::channel::matrix::default_matrix;
use audio_provenance_bench::corpus::{
    CorpusItem, CorpusSpec, generate_synthetic, load_wav_directory,
};
use audio_provenance_bench::ports::FileStore;
use audio_provenance_bench::ports::native::{DiskFileStore, ProcessRunner};
use audio_provenance_bench::report::{RowVerdict, Thresholds};
use audio_provenance_bench::runner::{BenchConfig, run};
use audio_provenance_bench::watermark::WatermarkCodec;
use genomark::bench::GenoMarkCodec;
use genomark::payload::Payload;

const USAGE: &str = "audio-provenance-corpus-bench --real-dir DIR [--out-dir DIR] [--ffmpeg PATH]
                     [--seed N] [--duration SECONDS] [--min-recovery RATE]
                     [--no-synthetic] [--no-perceptual]

Every .wav under --real-dir is loaded; there is no silent cap. The synthetic recipes are included
by default because they are the baseline the real items are read against.";

#[derive(Debug)]
struct Options {
    real_dir: Option<String>,
    out_dir: String,
    ffmpeg: String,
    seed: u64,
    duration: f64,
    min_recovery: Option<f64>,
    synthetic: bool,
    perceptual: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            real_dir: None,
            out_dir: "bench-out/real-corpus".to_owned(),
            ffmpeg: DEFAULT_FFMPEG.to_owned(),
            seed: 0x6765_6E6F_746F_6E65,
            duration: 30.0,
            min_recovery: None,
            synthetic: true,
            perceptual: true,
        }
    }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("`{flag}` needs a value"));
        match flag.as_str() {
            "--real-dir" => options.real_dir = Some(value()?),
            "--out-dir" => options.out_dir = value()?,
            "--ffmpeg" => options.ffmpeg = value()?,
            "--seed" => {
                options.seed = value()?
                    .parse()
                    .map_err(|_| "--seed must be an unsigned integer".to_owned())?;
            }
            "--duration" => {
                options.duration = value()?
                    .parse()
                    .map_err(|_| "--duration must be a number of seconds".to_owned())?;
            }
            "--min-recovery" => {
                options.min_recovery = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--min-recovery must be a rate in 0..=1".to_owned())?,
                );
            }
            "--no-synthetic" => options.synthetic = false,
            "--no-perceptual" => options.perceptual = false,
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
    let Some(real_dir) = options.real_dir.clone() else {
        eprintln!("--real-dir is required\n\n{USAGE}");
        return std::process::ExitCode::from(2);
    };

    let payload = match Payload::new(1, 0, 0x9AC3_5E00_11A7) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("payload is not representable: {error}");
            return std::process::ExitCode::from(2);
        }
    };

    let runner = ProcessRunner::default();
    let store = DiskFileStore;

    let found = match store.list_files(&real_dir, "wav") {
        Ok(paths) => paths.len(),
        Err(error) => {
            eprintln!("could not list {real_dir}: {error}");
            return std::process::ExitCode::from(3);
        }
    };
    if found == 0 {
        eprintln!("no .wav files under {real_dir}");
        return std::process::ExitCode::from(3);
    }
    let spec = CorpusSpec {
        duration_seconds: options.duration,
        max_real_seconds: options.duration,
        max_real_items: found,
        ..CorpusSpec::default()
    };

    let mut corpus: Vec<CorpusItem> = if options.synthetic {
        match generate_synthetic(&runner, &options.ffmpeg, &spec) {
            Ok(items) => items,
            Err(error) => {
                eprintln!("synthetic corpus generation failed: {error}");
                return std::process::ExitCode::from(3);
            }
        }
    } else {
        Vec::new()
    };
    match load_wav_directory(&store, &real_dir, &spec) {
        Ok(items) => {
            eprintln!("loaded {} real items from {real_dir}", items.len());
            corpus.extend(items);
        }
        Err(error) => {
            eprintln!("real corpus load failed: {error}");
            return std::process::ExitCode::from(3);
        }
    }

    let config = BenchConfig {
        seed: options.seed,
        payload: payload.to_bytes().to_vec(),
        thresholds: options
            .min_recovery
            .map_or_else(Thresholds::product_targets, Thresholds::uniform_minimum),
        measure_perceptual: options.perceptual,
        generated_at: std::env::var("AUDIO_PROVENANCE_BENCH_TIMESTAMP").ok(),
    };

    let codec = GenoMarkCodec::public();
    let matrix = default_matrix(&options.ffmpeg);
    eprintln!(
        "corpus bench: {} items x {} channels",
        corpus.len(),
        matrix.len()
    );

    let report = match run(&codec, &corpus, &matrix, &runner, &config) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("bench run failed: {error}");
            return std::process::ExitCode::from(3);
        }
    };

    let table = report.to_table();
    print!("{table}");

    match report.to_json() {
        Ok(json) => {
            let path = format!("{}/{}-real-corpus.json", options.out_dir, codec.name());
            if let Err(error) = store.write(&path, json.as_bytes()) {
                eprintln!("could not write {path}: {error}");
                return std::process::ExitCode::from(4);
            }
            let table_path = format!("{}/{}-real-corpus.txt", options.out_dir, codec.name());
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
