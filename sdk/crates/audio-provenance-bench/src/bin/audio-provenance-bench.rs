use audio_provenance_bench::channel::codec::DEFAULT_FFMPEG;
use audio_provenance_bench::channel::matrix::default_matrix;
use audio_provenance_bench::corpus::{CorpusItem, CorpusSpec, generate_synthetic, load_wav_files};
use audio_provenance_bench::fixtures::{AlwaysAcceptFixture, Lsb16Fixture, SilentFixture};
use audio_provenance_bench::ports::FileStore;
use audio_provenance_bench::ports::native::{DiskFileStore, ProcessRunner};
use audio_provenance_bench::report::{RowVerdict, Thresholds};
use audio_provenance_bench::runner::{BenchConfig, run};
use audio_provenance_bench::watermark::WatermarkCodec;

const USAGE: &str =
    "audio-provenance-bench --codec <fixture_lsb16|fixture_always_accept|fixture_silent>
                [--out-dir DIR] [--ffmpeg PATH] [--seed N] [--duration SECONDS]
                [--min-recovery RATE] [--real-wav PATH]... [--no-perceptual]

Every channel in the matrix appears in the report. A channel that could not run appears as an
explicit error row and fails the run; it is never omitted.";

#[derive(Debug)]
struct Options {
    codec: String,
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
            codec: "fixture_lsb16".to_owned(),
            out_dir: "bench-out".to_owned(),
            ffmpeg: DEFAULT_FFMPEG.to_owned(),
            seed: 0x6765_6E6F_746F_6E65,
            duration: 12.0,
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
            "--codec" => options.codec = value()?,
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

fn select_codec(name: &str) -> Result<Box<dyn WatermarkCodec>, String> {
    match name {
        "fixture_lsb16" => Ok(Box::new(Lsb16Fixture::new(8))),
        "fixture_always_accept" => Ok(Box::new(AlwaysAcceptFixture::new(vec![
            0x47, 0x54, 0x01, 0x9A, 0xC3, 0x5E, 0x00, 0x11,
        ]))),
        "fixture_silent" => Ok(Box::new(SilentFixture::new(8))),
        other => Err(format!("unknown codec `{other}`\n\n{USAGE}")),
    }
}

fn main() -> std::process::ExitCode {
    let options = match parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return std::process::ExitCode::from(2);
        }
    };

    let runner = ProcessRunner::default();
    let store = DiskFileStore;
    let spec = CorpusSpec {
        duration_seconds: options.duration,
        ..CorpusSpec::default()
    };

    let codec = match select_codec(&options.codec) {
        Ok(codec) => codec,
        Err(message) => {
            eprintln!("{message}");
            return std::process::ExitCode::from(2);
        }
    };

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
        thresholds: options
            .min_recovery
            .map_or_else(Thresholds::product_targets, Thresholds::uniform_minimum),
        measure_perceptual: options.perceptual,
        generated_at: std::env::var("AUDIO_PROVENANCE_BENCH_TIMESTAMP").ok(),
        ..BenchConfig::default()
    };

    let matrix = default_matrix(&options.ffmpeg);
    let report = match run(codec.as_ref(), &corpus, &matrix, &runner, &config) {
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
