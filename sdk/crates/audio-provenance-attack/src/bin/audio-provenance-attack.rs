//! Runs the adversarial campaign against `apw-watermark-lepqim-v1` and writes the report it measured.

use audio_provenance_attack::{campaign, spec};
use audio_provenance_bench::attacks::report::RemovalVerdict;
use audio_provenance_bench::corpus::{CorpusItem, CorpusSpec, load_wav_directory, load_wav_files};
use audio_provenance_bench::ports::FileStore;
use audio_provenance_bench::ports::native::{DiskFileStore, ProcessRunner};
use apw_watermark::Watermark;
use apw_watermark::payload::Payload;
use std::process::ExitCode;
use std::time::Instant;

const USAGE: &str = "audio-provenance-attack [probe|campaign|null] --corpus-dir DIR [--out PATH]
                     [--items N] [--duration SECONDS] [--ffmpeg PATH] [--seed N]
                     [--collusion-copies N] [--null-trials N]

The victim key is generated here and never leaves this process. Every attack labelled `spec_only`
or `one_marked_file` runs without it.";

#[derive(Debug)]
struct Options {
    mode: String,
    corpus_dir: String,
    files: Vec<String>,
    out: String,
    items: usize,
    duration: f64,
    ffmpeg: String,
    seed: u64,
    collusion_copies: usize,
    null_trials: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: "campaign".to_owned(),
            corpus_dir: "corpus/characterisation".to_owned(),
            files: Vec::new(),
            out: "bench-out/attacks/report.json".to_owned(),
            items: 4,
            duration: 30.0,
            ffmpeg: "/opt/homebrew/bin/ffmpeg".to_owned(),
            seed: 0x4154_5441_434B,
            collusion_copies: 8,
            null_trials: 64,
        }
    }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    if let Some(first) = args.next() {
        if first.starts_with("--") {
            return parse_flags(std::iter::once(first).chain(args), options);
        }
        options.mode = first;
    }
    parse_flags(args, options)
}

fn parse_flags(
    mut args: impl Iterator<Item = String>,
    mut options: Options,
) -> Result<Options, String> {
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("`{flag}` needs a value"));
        match flag.as_str() {
            "--corpus-dir" => options.corpus_dir = value()?,
            "--file" => options.files.push(value()?),
            "--out" => options.out = value()?,
            "--ffmpeg" => options.ffmpeg = value()?,
            "--items" => {
                options.items = value()?.parse().map_err(|_| "--items must be an integer")?;
            }
            "--duration" => {
                options.duration = value()?
                    .parse()
                    .map_err(|_| "--duration must be a number")?;
            }
            "--seed" => {
                options.seed = value()?.parse().map_err(|_| "--seed must be an integer")?;
            }
            "--collusion-copies" => {
                options.collusion_copies = value()?
                    .parse()
                    .map_err(|_| "--collusion-copies must be an integer")?;
            }
            "--null-trials" => {
                options.null_trials = value()?
                    .parse()
                    .map_err(|_| "--null-trials must be an integer")?;
            }
            "--help" | "-h" => return Err(USAGE.to_owned()),
            other => return Err(format!("unknown flag `{other}`\n\n{USAGE}")),
        }
    }
    Ok(options)
}

fn load(options: &Options) -> Result<Vec<CorpusItem>, String> {
    let store = DiskFileStore;
    let spec = CorpusSpec {
        sample_rate: 48_000,
        channels: 2,
        duration_seconds: options.duration,
        max_real_items: options.items,
        max_real_seconds: options.duration,
    };
    let mut items = if options.files.is_empty() {
        load_wav_directory(&store, &options.corpus_dir, &spec)
            .map_err(|error| format!("corpus load failed: {error}"))?
    } else {
        load_wav_files(&store, &options.files, &spec)
            .map_err(|error| format!("corpus load failed: {error}"))?
    };
    items.truncate(options.items);
    if items.is_empty() {
        return Err(format!("no .wav files under {}", options.corpus_dir));
    }
    if !options.files.is_empty() && items.len() != options.files.len() {
        return Err(format!(
            "{} files named but {} loaded; raise --items",
            options.files.len(),
            items.len()
        ));
    }
    Ok(items)
}

fn keys() -> Result<(Watermark, Watermark), String> {
    let victim = Watermark::keyed(b"victim/secret/profile/key/never-published".to_vec(), 1)
        .map_err(|error| error.to_string())?;
    let attacker = Watermark::keyed(b"attacker/own/profile/key".to_vec(), 2)
        .map_err(|error| error.to_string())?;
    Ok((victim, attacker))
}

fn probe(options: &Options) -> Result<(), String> {
    let items = load(options)?;
    let (victim, _) = keys()?;
    let puncture = spec::puncture();
    for item in &items {
        let geometry = spec::geometry(item.audio.sample_rate())?;
        println!(
            "item {} ({:.1} s, {} Hz, {} ch), band {:.1}-{:.1} Hz, frame {}, {} slots per block",
            item.meta.id,
            item.meta.duration_seconds,
            item.meta.sample_rate,
            item.meta.channels,
            geometry.band_hz().0,
            geometry.band_hz().1,
            geometry.frame(),
            geometry.slots_per_block()
        );

        let payload = Payload::new(apw_watermark::payload::VERSION, 1, 0x0000_A100_0000)
            .map_err(|error| error.to_string())?;
        let started = Instant::now();
        let marked = victim
            .embed(&item.audio, payload)
            .map_err(|error| error.to_string())?;
        println!("embed: {:.2} s", started.elapsed().as_secs_f64());

        let started = Instant::now();
        let outcome = victim.detect(&marked).map_err(|error| error.to_string())?;
        let detect_seconds = started.elapsed().as_secs_f64();
        println!(
            "detect: {detect_seconds:.2} s, payload {:?}, class {}, blocks {}",
            outcome
                .payload()
                .map(|value| format!("{:012x}", value.locator())),
            outcome.class().as_str(),
            outcome.blocks_accepted()
        );

        let (unmarked, marked_estimate) =
            campaign::residue_leak(&item.audio, &marked, &geometry, puncture)?;
        println!(
            "residue concentration: unmarked {:?}, marked {:?} over {} blocks ({} of {} slot indices estimated)",
            unmarked.concentration,
            marked_estimate.concentration,
            marked_estimate.blocks_observed,
            marked_estimate.slots_estimated,
            marked_estimate.period
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let options = match parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };

    let runner = ProcessRunner::default();
    let store = DiskFileStore;

    let result = match options.mode.as_str() {
        "probe" => probe(&options),
        "campaign" | "null" => (|| {
            let items = load(&options)?;
            let (victim, attacker) = keys()?;
            let geometry =
                spec::geometry(items.first().ok_or("corpus is empty")?.audio.sample_rate())?;
            let campaign_options = campaign::Options {
                victim: &victim,
                attacker: &attacker,
                geometry,
                puncture: spec::puncture(),
                runner: &runner,
                ffmpeg: options.ffmpeg.clone(),
                seed: options.seed,
                collusion_copies: options.collusion_copies,
                forgery_null_trials: options.null_trials,
            };
            if options.mode == "null" {
                let started = Instant::now();
                let (trials, accepts, payloads) =
                    campaign::forgery_null(&items, &campaign_options)?;
                println!(
                    "forgery null: {accepts} accepts in {trials} random-coset trials in {:.1} s {payloads:?}",
                    started.elapsed().as_secs_f64()
                );
                return Ok(());
            }
            let started = Instant::now();
            let report = campaign::run(&items, &campaign_options)?;
            let elapsed = started.elapsed().as_secs_f64();
            let json = serde_json::to_vec_pretty(&report)
                .map_err(|error| format!("serialisation failed: {error}"))?;
            store
                .write(&options.out, &json)
                .map_err(|error| format!("write {}: {error}", options.out))?;
            let succeeded = report
                .rows
                .iter()
                .filter(|row| row.attack_succeeded)
                .count();
            let unusable = report
                .rows
                .iter()
                .filter(|row| row.verdict == RemovalVerdict::BaselineMissing)
                .count();
            println!(
                "{} rows in {elapsed:.1} s, {succeeded} attacks succeeded, {unusable} rows had no \
                 detecting baseline. Report at {}",
                report.rows.len(),
                options.out
            );
            Ok(())
        })(),
        other => Err(format!("unknown mode `{other}`\n\n{USAGE}")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(3)
        }
    }
}
