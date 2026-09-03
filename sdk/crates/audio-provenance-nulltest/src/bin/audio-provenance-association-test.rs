//! Full-corpus recording-association qualification for classical GenoMark.
//!
//! This is deliberately separate from locator recovery. It embeds the production GenoMark-Q mark,
//! constructs the exact signed reference constellation used by `genotrace`, and measures the
//! candidate with `fingerprint::compare`, including 0.5 s local regions and the 0.75 s maximum
//! unexplained-gap predicate. No `genomark-n` dependency or feature exists in this crate.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use audio_provenance_audio::AudioBuffer;
use audio_provenance_bench::channel::Channel;
use audio_provenance_bench::channel::codec::DEFAULT_FFMPEG;
use audio_provenance_bench::channel::matrix::default_matrix;
use audio_provenance_bench::corpus::{CorpusItem, CorpusSpec, load_wav_directory};
use audio_provenance_bench::ports::FileStore;
use audio_provenance_bench::ports::native::{DiskFileStore, ProcessRunner};
use audio_provenance_bench::report::cell_seed;
use audio_provenance_bench::watermark::WatermarkCodec;
use genomark::bench::GenoMarkCodec;
use genomark::payload::Payload;
use genotrace::fingerprint::reference::{AFFIRMATION_REGION_SECONDS, MAX_UNEXPLAINED_GAP_SECONDS};
use genotrace::fingerprint::score::ScoreLimits;
use genotrace::fingerprint::{
    AFFIRMATION_COVERAGE, FingerprintQuery, ReferenceFingerprint, compare,
};
use serde::Serialize;

const SCHEMA: &str = "audio-provenance-association-qualification-v1";
const IMPLEMENTATION: &str = "genomark-lepqim-v1";
const DEFAULT_SEED: u64 = 0x4153_534f_4349_4154;
const LONG_SECONDS: f64 = 120.0;

const USAGE: &str = "audio-provenance-association-test --corpus-dir DIR --out PATH
    [--ffmpeg PATH] [--workers N] [--duration SECONDS] [--seed N]";

#[derive(Debug)]
struct Options {
    corpus_dir: Option<String>,
    out: Option<String>,
    ffmpeg: String,
    workers: usize,
    duration: f64,
    seed: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            corpus_dir: None,
            out: None,
            ffmpeg: DEFAULT_FFMPEG.to_owned(),
            workers: thread::available_parallelism().map_or(1, usize::from),
            duration: 30.0,
            seed: DEFAULT_SEED,
        }
    }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("`{flag}` needs a value"));
        match flag.as_str() {
            "--corpus-dir" => options.corpus_dir = Some(value()?),
            "--out" => options.out = Some(value()?),
            "--ffmpeg" => options.ffmpeg = value()?,
            "--workers" => {
                options.workers = value()?
                    .parse()
                    .map_err(|_| "--workers must be a positive integer".to_owned())?;
            }
            "--duration" => {
                options.duration = value()?
                    .parse()
                    .map_err(|_| "--duration must be a positive number".to_owned())?;
            }
            "--seed" => {
                options.seed = value()?
                    .parse()
                    .map_err(|_| "--seed must be an unsigned integer".to_owned())?;
            }
            "-h" | "--help" => return Err(USAGE.to_owned()),
            other => return Err(format!("unrecognised argument `{other}`\n\n{USAGE}")),
        }
    }
    if options.workers == 0 || !options.duration.is_finite() || options.duration <= 0.0 {
        return Err(USAGE.to_owned());
    }
    Ok(options)
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Expectation {
    Associated,
    Rejected,
}

#[derive(Debug, Serialize)]
struct CaseResult {
    item: String,
    material_class: String,
    case: String,
    family: String,
    expectation: Expectation,
    passed: bool,
    affirmed: bool,
    gates_passed: bool,
    coverage: f64,
    regions_expected: usize,
    regions_covered: usize,
    max_unexplained_gap_seconds: f64,
    offset_discontinuities: usize,
    duration_consistent: bool,
    query_duration_seconds: f64,
    error: Option<String>,
}

impl CaseResult {
    fn error(
        item: &CorpusItem,
        case: impl Into<String>,
        family: &str,
        expectation: Expectation,
        error: impl Into<String>,
    ) -> Self {
        Self {
            item: item.meta.id.clone(),
            material_class: format!("{:?}", item.meta.class).to_lowercase(),
            case: case.into(),
            family: family.to_owned(),
            expectation,
            passed: false,
            affirmed: false,
            gates_passed: false,
            coverage: 0.0,
            regions_expected: 0,
            regions_covered: 0,
            max_unexplained_gap_seconds: 0.0,
            offset_discontinuities: 0,
            duration_consistent: false,
            query_duration_seconds: 0.0,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Serialize)]
struct Summary {
    trials: usize,
    passed: usize,
    failed: usize,
    errors: usize,
    association_expected: usize,
    association_recovered: usize,
    rejection_expected: usize,
    alterations_rejected: usize,
    mark_recovery_rate: Option<f64>,
    association_recovery_rate: Option<f64>,
    alteration_rejection_rate: Option<f64>,
}

#[derive(Debug, Serialize)]
struct Report {
    schema: &'static str,
    production_implementation: &'static str,
    experimental_implementations_included: Vec<String>,
    seed: u64,
    corpus_items: usize,
    workers: usize,
    predicates: Predicates,
    summary: Summary,
    cases: Vec<CaseResult>,
}

#[derive(Debug, Serialize)]
struct Predicates {
    local_region_seconds: f64,
    maximum_unexplained_gap_seconds: f64,
    minimum_local_coverage: f64,
}

fn evaluate(
    item: &CorpusItem,
    case: impl Into<String>,
    family: &str,
    expectation: Expectation,
    reference: &ReferenceFingerprint,
    candidate: &AudioBuffer,
) -> CaseResult {
    let case = case.into();
    let query = match FingerprintQuery::from_audio(candidate) {
        Ok(query) => query,
        Err(error) => return CaseResult::error(item, case, family, expectation, error.to_string()),
    };
    let measured = match compare(reference, &query, ScoreLimits::default()) {
        Ok(measured) => measured,
        Err(error) => return CaseResult::error(item, case, family, expectation, error.to_string()),
    };
    let passed = match expectation {
        Expectation::Associated => measured.affirmed,
        Expectation::Rejected => !measured.affirmed,
    };
    CaseResult {
        item: item.meta.id.clone(),
        material_class: format!("{:?}", item.meta.class).to_lowercase(),
        case,
        family: family.to_owned(),
        expectation,
        passed,
        affirmed: measured.affirmed,
        gates_passed: measured.gates_passed,
        coverage: measured.coverage,
        regions_expected: measured.regions_expected,
        regions_covered: measured.regions_covered,
        max_unexplained_gap_seconds: measured.max_unexplained_gap_seconds,
        offset_discontinuities: measured.offset_discontinuities,
        duration_consistent: measured.duration_consistent,
        query_duration_seconds: measured.query_duration_seconds,
        error: None,
    }
}

fn from_channels(rate: u32, channels: Vec<Vec<f32>>) -> Result<AudioBuffer, String> {
    AudioBuffer::from_channels(rate, &channels).map_err(|error| error.to_string())
}

fn concatenate(buffers: &[&AudioBuffer], seconds: f64) -> Result<AudioBuffer, String> {
    let Some(first) = buffers.first() else {
        return Err("cannot concatenate an empty buffer list".to_owned());
    };
    let target = (seconds * f64::from(first.sample_rate())).round() as usize;
    let mut channels = vec![Vec::with_capacity(target); first.channels()];
    let mut written = 0usize;
    let mut source_index = 0usize;
    while written < target {
        let source = buffers[source_index % buffers.len()];
        if source.sample_rate() != first.sample_rate() || source.channels() != first.channels() {
            return Err("corpus buffers do not share one shape".to_owned());
        }
        let count = source.frames().min(target - written);
        for (channel, output) in channels.iter_mut().enumerate() {
            let Some(input) = source.channel(channel) else {
                return Err("corpus channel disappeared".to_owned());
            };
            output.extend_from_slice(&input[..count]);
        }
        written += count;
        source_index += 1;
    }
    from_channels(first.sample_rate(), channels)
}

fn replace_fraction(
    source: &AudioBuffer,
    donor: &AudioBuffer,
    fraction: f64,
    start_fraction: f64,
) -> Result<AudioBuffer, String> {
    let mut channels = Vec::with_capacity(source.channels());
    let count = ((source.frames() as f64 * fraction).round() as usize).max(1);
    let start = ((source.frames().saturating_sub(count)) as f64 * start_fraction).round() as usize;
    for channel in 0..source.channels() {
        let Some(input) = source.channel(channel) else {
            return Err("source channel disappeared".to_owned());
        };
        let Some(donor_input) = donor.channel(channel) else {
            return Err("donor channel disappeared".to_owned());
        };
        let mut output = input.to_vec();
        for offset in 0..count {
            output[start + offset] = donor_input[offset % donor_input.len()];
        }
        channels.push(output);
    }
    from_channels(source.sample_rate(), channels)
}

fn crop(
    source: &AudioBuffer,
    offset_seconds: f64,
    length_seconds: f64,
) -> Result<AudioBuffer, String> {
    let start = (offset_seconds * f64::from(source.sample_rate())).round() as usize;
    let count = (length_seconds * f64::from(source.sample_rate())).round() as usize;
    let end = start.saturating_add(count).min(source.frames());
    if start >= end {
        return Err("crop is empty".to_owned());
    }
    let mut channels = Vec::with_capacity(source.channels());
    for channel in 0..source.channels() {
        let Some(input) = source.channel(channel) else {
            return Err("source channel disappeared".to_owned());
        };
        channels.push(input[start..end].to_vec());
    }
    from_channels(source.sample_rate(), channels)
}

fn reorder_quarters(source: &AudioBuffer) -> Result<AudioBuffer, String> {
    let quarter = source.frames() / 4;
    let order = [0usize, 2, 1, 3];
    let mut channels = Vec::with_capacity(source.channels());
    for channel in 0..source.channels() {
        let Some(input) = source.channel(channel) else {
            return Err("source channel disappeared".to_owned());
        };
        let mut output = Vec::with_capacity(source.frames());
        for part in order {
            let start = part * quarter;
            let end = if part == 3 {
                source.frames()
            } else {
                start + quarter
            };
            output.extend_from_slice(&input[start..end]);
        }
        channels.push(output);
    }
    from_channels(source.sample_rate(), channels)
}

fn duplicate_quarter(source: &AudioBuffer) -> Result<AudioBuffer, String> {
    let quarter = source.frames() / 4;
    let mut channels = Vec::with_capacity(source.channels());
    for channel in 0..source.channels() {
        let Some(input) = source.channel(channel) else {
            return Err("source channel disappeared".to_owned());
        };
        let mut output = input.to_vec();
        output[2 * quarter..3 * quarter].copy_from_slice(&input[quarter..2 * quarter]);
        channels.push(output);
    }
    from_channels(source.sample_rate(), channels)
}

fn overlay(source: &AudioBuffer, donor: &AudioBuffer, db: f64) -> Result<AudioBuffer, String> {
    let gain = 10.0f32.powf(db as f32 / 20.0);
    let mut channels = Vec::with_capacity(source.channels());
    for channel in 0..source.channels() {
        let Some(input) = source.channel(channel) else {
            return Err("source channel disappeared".to_owned());
        };
        let Some(donor_input) = donor.channel(channel) else {
            return Err("donor channel disappeared".to_owned());
        };
        channels.push(
            input
                .iter()
                .enumerate()
                .map(|(index, sample)| *sample + donor_input[index % donor_input.len()] * gain)
                .collect(),
        );
    }
    from_channels(source.sample_rate(), channels)
}

fn selected_channels(ffmpeg: &str) -> Vec<Arc<dyn Channel>> {
    default_matrix(ffmpeg)
        .into_iter()
        .filter(|channel| {
            let name = channel.name();
            name == "identity"
                || name.starts_with("mp3_")
                || name.starts_with("aac_")
                || name.starts_with("gain_")
                || name == "normalize_peak"
                || name == "compress_dynamic"
                || name == "limit_brickwall"
                || name == "chain_transcode_mp3_128_aac_128"
                || name == "chain_broadcast_limit_mp3_192"
        })
        .map(Arc::<dyn Channel>::from)
        .collect()
}

fn evaluate_item(
    index: usize,
    corpus: &[CorpusItem],
    channels: &[Arc<dyn Channel>],
    options: &Options,
) -> Vec<CaseResult> {
    let item = &corpus[index];
    let codec = GenoMarkCodec::public();
    let payload = match Payload::new(1, 0, 0x9AC3_5E00_11A7) {
        Ok(payload) => payload.to_bytes(),
        Err(error) => {
            return vec![CaseResult::error(
                item,
                "embed",
                "locator_recovery",
                Expectation::Associated,
                error.to_string(),
            )];
        }
    };
    let marked = match codec.embed(&item.audio, &payload) {
        Ok(marked) => marked,
        Err(error) => {
            return vec![CaseResult::error(
                item,
                "embed",
                "locator_recovery",
                Expectation::Associated,
                error.to_string(),
            )];
        }
    };
    let reference = match ReferenceFingerprint::from_audio(&marked) {
        Ok(Some(reference)) => reference,
        Ok(None) => {
            return vec![CaseResult::error(
                item,
                "reference",
                "recording_association",
                Expectation::Associated,
                "reference exceeded the signed peak bound",
            )];
        }
        Err(error) => {
            return vec![CaseResult::error(
                item,
                "reference",
                "recording_association",
                Expectation::Associated,
                error.to_string(),
            )];
        }
    };

    let runner = ProcessRunner::default();
    let mut results = Vec::new();
    for channel in channels {
        if channel.required_programs().iter().any(|program| {
            !audio_provenance_bench::ports::CommandRunner::is_available(&runner, program)
        }) {
            results.push(CaseResult::error(
                item,
                channel.name(),
                "preservation_channel",
                Expectation::Associated,
                "required codec executable is unavailable",
            ));
            continue;
        }
        let seed = cell_seed(options.seed, &item.meta.id, channel.name());
        match channel.apply(&marked, seed, &runner) {
            Ok(candidate) => results.push(evaluate(
                item,
                channel.name(),
                "preservation_channel",
                Expectation::Associated,
                &reference,
                &candidate,
            )),
            Err(error) => results.push(CaseResult::error(
                item,
                channel.name(),
                "preservation_channel",
                Expectation::Associated,
                error.to_string(),
            )),
        }
    }

    let source_buffers = [
        &marked,
        &corpus[(index + 1) % corpus.len()].audio,
        &corpus[(index + 2) % corpus.len()].audio,
        &corpus[(index + 3) % corpus.len()].audio,
    ];
    let long = match concatenate(&source_buffers, LONG_SECONDS) {
        Ok(long) => long,
        Err(error) => {
            results.push(CaseResult::error(
                item,
                "long_corpus",
                "recording_association",
                Expectation::Associated,
                error,
            ));
            return results;
        }
    };
    let donor_buffers = [
        &corpus[(index + 4) % corpus.len()].audio,
        &corpus[(index + 5) % corpus.len()].audio,
        &corpus[(index + 6) % corpus.len()].audio,
        &corpus[(index + 7) % corpus.len()].audio,
    ];
    let donor = match concatenate(&donor_buffers, LONG_SECONDS) {
        Ok(donor) => donor,
        Err(error) => {
            results.push(CaseResult::error(
                item,
                "long_donor",
                "recording_association",
                Expectation::Rejected,
                error,
            ));
            return results;
        }
    };
    let long_reference = match ReferenceFingerprint::from_audio(&long) {
        Ok(Some(reference)) => reference,
        Ok(None) => {
            results.push(CaseResult::error(
                item,
                "long_reference",
                "recording_association",
                Expectation::Associated,
                "120 second reference exceeded the signed peak bound",
            ));
            return results;
        }
        Err(error) => {
            results.push(CaseResult::error(
                item,
                "long_reference",
                "recording_association",
                Expectation::Associated,
                error.to_string(),
            ));
            return results;
        }
    };

    for percent in [1u32, 2, 5, 10, 25, 50] {
        for (position, start) in [("head", 0.0), ("interior", 0.5), ("tail", 1.0)] {
            let label = format!("{position}_replacement_{percent}_percent");
            match replace_fraction(&long, &donor, f64::from(percent) / 100.0, start) {
                Ok(candidate) => results.push(evaluate(
                    item,
                    label,
                    "material_replacement",
                    Expectation::Rejected,
                    &long_reference,
                    &candidate,
                )),
                Err(error) => results.push(CaseResult::error(
                    item,
                    label,
                    "material_replacement",
                    Expectation::Rejected,
                    error,
                )),
            }
        }
    }

    for offset in [0.137, 3.37, 7.3, 31.113] {
        let label = format!("arbitrary_crop_{offset:.3}_seconds");
        match crop(&long, offset, 60.0) {
            Ok(candidate) => results.push(evaluate(
                item,
                label,
                "crop",
                Expectation::Associated,
                &long_reference,
                &candidate,
            )),
            Err(error) => results.push(CaseResult::error(
                item,
                label,
                "crop",
                Expectation::Associated,
                error,
            )),
        }
    }

    for (label, candidate) in [
        ("reordered_quarters", reorder_quarters(&long)),
        ("duplicated_quarter", duplicate_quarter(&long)),
    ] {
        match candidate {
            Ok(candidate) => results.push(evaluate(
                item,
                label,
                "structural_edit",
                Expectation::Rejected,
                &long_reference,
                &candidate,
            )),
            Err(error) => results.push(CaseResult::error(
                item,
                label,
                "structural_edit",
                Expectation::Rejected,
                error,
            )),
        }
    }

    for db in [-24.0, -12.0, -6.0, 0.0] {
        let label = format!("overlay_{db:.0}_db");
        match overlay(&long, &donor, db) {
            Ok(candidate) => results.push(evaluate(
                item,
                label,
                "overlay",
                Expectation::Rejected,
                &long_reference,
                &candidate,
            )),
            Err(error) => results.push(CaseResult::error(
                item,
                label,
                "overlay",
                Expectation::Rejected,
                error,
            )),
        }
    }

    results
}

fn main() -> std::process::ExitCode {
    let options = match parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return std::process::ExitCode::from(2);
        }
    };
    let Some(corpus_dir) = options.corpus_dir.as_deref() else {
        eprintln!("--corpus-dir is required\n\n{USAGE}");
        return std::process::ExitCode::from(2);
    };
    let Some(out) = options.out.as_deref() else {
        eprintln!("--out is required\n\n{USAGE}");
        return std::process::ExitCode::from(2);
    };

    let spec = CorpusSpec {
        sample_rate: 48_000,
        channels: 2,
        duration_seconds: options.duration,
        max_real_seconds: options.duration,
        max_real_items: usize::MAX,
    };
    let store = DiskFileStore;
    let corpus = match load_wav_directory(&store, corpus_dir, &spec) {
        Ok(corpus) if !corpus.is_empty() => Arc::new(corpus),
        Ok(_) => {
            eprintln!("no WAV corpus items under {corpus_dir}");
            return std::process::ExitCode::from(3);
        }
        Err(error) => {
            eprintln!("could not load association corpus: {error}");
            return std::process::ExitCode::from(3);
        }
    };
    let channels = Arc::new(selected_channels(&options.ffmpeg));
    let next = Arc::new(AtomicUsize::new(0));
    let results = Arc::new(Mutex::new(Vec::<CaseResult>::new()));
    let worker_count = options.workers.min(corpus.len()).max(1);

    thread::scope(|scope| {
        for _ in 0..worker_count {
            let corpus = Arc::clone(&corpus);
            let channels = Arc::clone(&channels);
            let next = Arc::clone(&next);
            let results = Arc::clone(&results);
            let options = &options;
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= corpus.len() {
                        break;
                    }
                    eprintln!(
                        "association item {}/{}: {}",
                        index + 1,
                        corpus.len(),
                        corpus[index].meta.id
                    );
                    let mut item_results = evaluate_item(index, &corpus, &channels, options);
                    match results.lock() {
                        Ok(mut held) => held.append(&mut item_results),
                        Err(_) => break,
                    }
                }
            });
        }
    });

    let mut cases = match Arc::try_unwrap(results) {
        Ok(mutex) => match mutex.into_inner() {
            Ok(cases) => cases,
            Err(_) => {
                eprintln!("association result mutex was poisoned");
                return std::process::ExitCode::from(4);
            }
        },
        Err(_) => {
            eprintln!("association workers did not release the result set");
            return std::process::ExitCode::from(4);
        }
    };
    cases.sort_by(|left, right| {
        (&left.item, &left.family, &left.case).cmp(&(&right.item, &right.family, &right.case))
    });

    let errors = cases.iter().filter(|case| case.error.is_some()).count();
    let passed = cases.iter().filter(|case| case.passed).count();
    let association_expected = cases
        .iter()
        .filter(|case| matches!(case.expectation, Expectation::Associated) && case.error.is_none())
        .count();
    let association_recovered = cases
        .iter()
        .filter(|case| {
            matches!(case.expectation, Expectation::Associated)
                && case.affirmed
                && case.error.is_none()
        })
        .count();
    let rejection_expected = cases
        .iter()
        .filter(|case| matches!(case.expectation, Expectation::Rejected) && case.error.is_none())
        .count();
    let alterations_rejected = cases
        .iter()
        .filter(|case| {
            matches!(case.expectation, Expectation::Rejected)
                && !case.affirmed
                && case.error.is_none()
        })
        .count();
    let rate = |numerator: usize, denominator: usize| {
        (denominator > 0).then_some(numerator as f64 / denominator as f64)
    };
    let summary = Summary {
        trials: cases.len(),
        passed,
        failed: cases.len().saturating_sub(passed + errors),
        errors,
        association_expected,
        association_recovered,
        rejection_expected,
        alterations_rejected,
        // Locator recovery is measured by the sibling corpus bench and deliberately absent here.
        mark_recovery_rate: None,
        association_recovery_rate: rate(association_recovered, association_expected),
        alteration_rejection_rate: rate(alterations_rejected, rejection_expected),
    };
    let report = Report {
        schema: SCHEMA,
        production_implementation: IMPLEMENTATION,
        experimental_implementations_included: Vec::new(),
        seed: options.seed,
        corpus_items: corpus.len(),
        workers: worker_count,
        predicates: Predicates {
            local_region_seconds: AFFIRMATION_REGION_SECONDS,
            maximum_unexplained_gap_seconds: MAX_UNEXPLAINED_GAP_SECONDS,
            minimum_local_coverage: AFFIRMATION_COVERAGE,
        },
        summary,
        cases,
    };
    let json = match serde_json::to_vec_pretty(&report) {
        Ok(json) => json,
        Err(error) => {
            eprintln!("could not serialise association report: {error}");
            return std::process::ExitCode::from(4);
        }
    };
    if let Some(parent) = Path::new(out).parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("could not create {}: {error}", parent.display());
        return std::process::ExitCode::from(4);
    }
    if let Err(error) = store.write(out, &json) {
        eprintln!("could not write {out}: {error}");
        return std::process::ExitCode::from(4);
    }
    eprintln!(
        "association qualification: {}/{} passed, {} execution errors; wrote {out}",
        report.summary.passed, report.summary.trials, report.summary.errors
    );
    if report.summary.errors == 0 && report.summary.failed == 0 {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
