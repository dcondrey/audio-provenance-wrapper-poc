use audio_provenance_bench::channel::codec::DEFAULT_FFMPEG;
use audio_provenance_bench::channel::matrix::default_matrix;
use audio_provenance_bench::corpus::{CorpusItem, CorpusSpec, generate_synthetic, load_wav_files};
use audio_provenance_bench::ports::FileStore;
use audio_provenance_bench::ports::native::{DiskFileStore, ProcessRunner};
use audio_provenance_bench::report::{RowVerdict, Thresholds};
use audio_provenance_bench::runner::{BenchConfig, run};
use audio_provenance_bench::watermark::WatermarkCodec;
use genomark::bench::GenoMarkCodec;
use genomark::payload::Payload;

const USAGE: &str = "genomark-bench [--out-dir DIR] [--ffmpeg PATH] [--seed N]
                    [--duration SECONDS] [--min-recovery RATE] [--real-wav PATH]...
                    [--no-perceptual]

One GenoMark block is 416 slots; at 48 kHz that is 8.875 s, and an arbitrary crop needs two of
them. A duration under about 20 s measures the block layout, not the mark.";

#[derive(Debug)]
struct Options {
    out_dir: String,
    ffmpeg: String,
    seed: u64,
    duration: f64,
    min_recovery: Option<f64>,
    real_wavs: Vec<String>,
    perceptual: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            out_dir: "bench-out".to_owned(),
            ffmpeg: DEFAULT_FFMPEG.to_owned(),
            seed: 0x6765_6E6F_746F_6E65,
            duration: 30.0,
            min_recovery: None,
            real_wavs: Vec::new(),
            perceptual: true,
        }
    }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("`{flag}` needs a value"));
        match flag.as_str() {
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
            "--real-wav" => options.real_wavs.push(value()?),
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

    let payload = match Payload::new(1, 0, 0x9AC3_5E00_11A7) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("payload is not representable: {error}");
            return std::process::ExitCode::from(2);
        }
    };

    let runner = ProcessRunner::default();
    let store = DiskFileStore;
    let spec = CorpusSpec {
        duration_seconds: options.duration,
        max_real_seconds: options.duration,
        ..CorpusSpec::default()
    };

    let codec = GenoMarkCodec::public();
    let mut corpus: Vec<CorpusItem> = match generate_synthetic(&runner, &options.ffmpeg, &spec) {
        Ok(items) => items,
        Err(error) => {
            eprintln!("corpus generation failed: {error}");
            return std::process::ExitCode::from(3);
        }
    };
    if !options.real_wavs.is_empty() {
        match load_wav_files(&store, &options.real_wavs, &spec) {
            Ok(items) => corpus.extend(items),
            Err(error) => {
                eprintln!("real corpus load failed: {error}");
                return std::process::ExitCode::from(3);
            }
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

    let matrix = default_matrix(&options.ffmpeg);
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
            let path = format!("{}/{}.json", options.out_dir, codec.name());
            if let Err(error) = store.write(&path, json.as_bytes()) {
                eprintln!("could not write {path}: {error}");
                return std::process::ExitCode::from(4);
            }
            let table_path = format!("{}/{}.txt", options.out_dir, codec.name());
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
