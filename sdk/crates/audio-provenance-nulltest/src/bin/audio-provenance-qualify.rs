//! Parallel classical-Watermark qualification orchestrator and static baseline writer.
//!
//! The four measurement arms run as independent release binaries so null detection, watermark
//! recovery, adversarial attacks, and recording association can use separate worker pools. The
//! orchestrator never loads `apw-watermark-neural`; its report records an empty experimental implementation
//! list and hashes every raw artifact it summarizes.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use audio_provenance_core::sha256_hex;
use serde::Serialize;
use serde_json::Value;

const SCHEMA: &str = "audio-provenance-classical-qualification-v1";
const IMPLEMENTATION: &str = "apw-watermark-lepqim-v1";
const DEFAULT_SEED: u64 = 0x6765_6E6F_746F_6E65;

const USAGE: &str = "audio-provenance-qualify --null-dir DIR --corpus-dir DIR --out-dir DIR
    --report PATH [--null-manifest PATH] [--corpus-manifest PATH] [--ffmpeg PATH]
    [--workers N] [--seed N] [--timestamp RFC3339] [--aggregate-only]
    [--null-report PATH] [--recovery-report PATH] [--association-report PATH]
    [--attack-report PATH]";

#[derive(Debug)]
struct Options {
    null_dir: Option<PathBuf>,
    corpus_dir: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    report: Option<PathBuf>,
    null_manifest: Option<PathBuf>,
    corpus_manifest: Option<PathBuf>,
    null_report: Option<PathBuf>,
    recovery_report: Option<PathBuf>,
    association_report: Option<PathBuf>,
    attack_report: Option<PathBuf>,
    ffmpeg: String,
    workers: usize,
    seed: u64,
    timestamp: Option<String>,
    aggregate_only: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            null_dir: None,
            corpus_dir: None,
            out_dir: None,
            report: None,
            null_manifest: None,
            corpus_manifest: None,
            null_report: None,
            recovery_report: None,
            association_report: None,
            attack_report: None,
            ffmpeg: "/opt/homebrew/bin/ffmpeg".to_owned(),
            workers: thread::available_parallelism().map_or(1, usize::from),
            seed: DEFAULT_SEED,
            timestamp: None,
            aggregate_only: false,
        }
    }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    while let Some(flag) = args.next() {
        if flag == "--aggregate-only" {
            options.aggregate_only = true;
            continue;
        }
        if matches!(flag.as_str(), "-h" | "--help") {
            return Err(USAGE.to_owned());
        }
        let value = args
            .next()
            .ok_or_else(|| format!("`{flag}` needs a value"))?;
        match flag.as_str() {
            "--null-dir" => options.null_dir = Some(value.into()),
            "--corpus-dir" => options.corpus_dir = Some(value.into()),
            "--out-dir" => options.out_dir = Some(value.into()),
            "--report" => options.report = Some(value.into()),
            "--null-manifest" => options.null_manifest = Some(value.into()),
            "--corpus-manifest" => options.corpus_manifest = Some(value.into()),
            "--null-report" => options.null_report = Some(value.into()),
            "--recovery-report" => options.recovery_report = Some(value.into()),
            "--association-report" => options.association_report = Some(value.into()),
            "--attack-report" => options.attack_report = Some(value.into()),
            "--ffmpeg" => options.ffmpeg = value,
            "--workers" => {
                options.workers = value
                    .parse()
                    .map_err(|_| "--workers must be a positive integer".to_owned())?;
            }
            "--seed" => {
                options.seed = value
                    .parse()
                    .map_err(|_| "--seed must be an unsigned integer".to_owned())?;
            }
            "--timestamp" => options.timestamp = Some(value),
            other => return Err(format!("unrecognised argument `{other}`\n\n{USAGE}")),
        }
    }
    if options.workers == 0 {
        return Err("--workers must be positive".to_owned());
    }
    Ok(options)
}

#[derive(Debug)]
struct Task {
    name: &'static str,
    program: PathBuf,
    args: Vec<String>,
    log: PathBuf,
}

#[derive(Debug, Serialize)]
struct TaskOutcome {
    name: &'static str,
    exit_code: Option<i32>,
    measurement_completed: bool,
    log: String,
    error: Option<String>,
}

fn run_task(task: Task, timestamp: Option<&str>) -> TaskOutcome {
    let result = (|| -> Result<Option<i32>, String> {
        let stdout = File::create(&task.log)
            .map_err(|error| format!("create {}: {error}", task.log.display()))?;
        let stderr = stdout
            .try_clone()
            .map_err(|error| format!("clone {}: {error}", task.log.display()))?;
        let mut command = Command::new(&task.program);
        command
            .args(&task.args)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        if let Some(timestamp) = timestamp {
            command.env("AUDIO_PROVENANCE_BENCH_TIMESTAMP", timestamp);
        }
        command
            .status()
            .map(|status| status.code())
            .map_err(|error| format!("run {}: {error}", task.program.display()))
    })();
    match result {
        Ok(exit_code) => TaskOutcome {
            name: task.name,
            exit_code,
            // Exit 1 is a completed measurement whose predeclared quality threshold failed.
            measurement_completed: matches!(exit_code, Some(0 | 1)),
            log: task.log.display().to_string(),
            error: None,
        },
        Err(error) => TaskOutcome {
            name: task.name,
            exit_code: None,
            measurement_completed: false,
            log: task.log.display().to_string(),
            error: Some(error),
        },
    }
}

fn sibling_binary(name: &str) -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let parent = current
        .parent()
        .ok_or_else(|| "qualification executable has no parent directory".to_owned())?;
    let path = parent.join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "{} is missing; build the qualification binaries together before running",
            path.display()
        ))
    }
}

fn read_json(path: &Path) -> Result<(Value, String), String> {
    let bytes = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    Ok((value, sha256_hex(&bytes)))
}

fn u64_at(value: &Value, pointer: &str) -> Result<u64, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("report field {pointer} is missing or not an integer"))
}

fn f64_at(value: &Value, pointer: &str) -> Option<f64> {
    value.pointer(pointer).and_then(Value::as_f64)
}

#[derive(Debug, Serialize)]
struct Artifact {
    path: String,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct FalsePositiveBaseline {
    trials: u64,
    accepts: u64,
    measured_rate: Option<f64>,
    upper_bound_95: Option<f64>,
}

#[derive(Debug, Serialize)]
struct RecoveryBaseline {
    trials: u64,
    exact_recoveries: u64,
    exact_recovery_rate: Option<f64>,
}

#[derive(Debug, Serialize)]
struct AssociationBaseline {
    trials: u64,
    passed: u64,
    failed: u64,
    errors: u64,
    association_recovery_rate: Option<f64>,
    alteration_rejection_rate: Option<f64>,
    local_region_seconds: f64,
    maximum_unexplained_gap_seconds: f64,
    minimum_local_coverage: f64,
}

#[derive(Debug, Serialize)]
struct AttackBaseline {
    trials: usize,
    successful_attacks: usize,
    collusion_trials: usize,
    collusion_successes: usize,
}

#[derive(Debug, Serialize)]
struct DatasetCoverage {
    null_items: usize,
    recovery_items: usize,
    recovery_real_items: usize,
    association_items: usize,
    adversarial_items: usize,
    required_null_items: usize,
    required_characterisation_items: usize,
    complete: bool,
}

#[derive(Debug, Serialize)]
struct BaselineReport {
    schema: &'static str,
    generated_at: Option<String>,
    production_implementation: &'static str,
    experimental_implementations_included: Vec<String>,
    feature_isolation: &'static str,
    seed: u64,
    qualification_complete: bool,
    meets_predeclared_targets: bool,
    false_positive: FalsePositiveBaseline,
    mark_recovery: RecoveryBaseline,
    recording_association: AssociationBaseline,
    adversarial: AttackBaseline,
    dataset_coverage: DatasetCoverage,
    artifacts: Vec<Artifact>,
    runs: Vec<TaskOutcome>,
    notes: Vec<&'static str>,
}

fn artifact(path: &Path, hash: String) -> Artifact {
    Artifact {
        path: path.display().to_string(),
        sha256: hash,
    }
}

fn markdown(report: &BaselineReport) -> String {
    let rate = |value: Option<f64>| value.map_or_else(|| "n/a".to_owned(), |v| format!("{v:.8}"));
    format!(
        "# Classical Watermark qualification baseline\n\n\
         - Production implementation: `{}`\n\
         - Experimental implementations included: none\n\
         - Qualification complete: `{}`\n\
         - Meets every predeclared target: `{}`\n\
         - False positives: {} / {} (rate {}, 95% upper bound {})\n\
         - Exact mark recovery: {} / {} (rate {})\n\
         - Recording association recovery: {}\n\
         - Material-alteration rejection: {}\n\
         - Association predicates: {:.2} s local regions, {:.2} s maximum unexplained gap, {:.2} minimum coverage\n\
         - Dataset coverage: {} null; {} recovery real; {} association; {} adversarial items\n\n\
         `apw-watermark-neural` is not linked into the qualification runner. Mark recovery is a locator \
         measurement, not authentication; signed-record association and signature verification \
         remain separate gates.\n",
        report.production_implementation,
        report.qualification_complete,
        report.meets_predeclared_targets,
        report.false_positive.accepts,
        report.false_positive.trials,
        rate(report.false_positive.measured_rate),
        rate(report.false_positive.upper_bound_95),
        report.mark_recovery.exact_recoveries,
        report.mark_recovery.trials,
        rate(report.mark_recovery.exact_recovery_rate),
        rate(report.recording_association.association_recovery_rate),
        rate(report.recording_association.alteration_rejection_rate),
        report.recording_association.local_region_seconds,
        report.recording_association.maximum_unexplained_gap_seconds,
        report.recording_association.minimum_local_coverage,
        report.dataset_coverage.null_items,
        report.dataset_coverage.recovery_real_items,
        report.dataset_coverage.association_items,
        report.dataset_coverage.adversarial_items,
    )
}

fn main() -> std::process::ExitCode {
    let options = match parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return std::process::ExitCode::from(2);
        }
    };
    let Some(report_path) = options.report.as_deref() else {
        eprintln!("--report is required\n\n{USAGE}");
        return std::process::ExitCode::from(2);
    };
    let out_dir = options.out_dir.as_deref();
    if let Some(out_dir) = out_dir
        && let Err(error) = std::fs::create_dir_all(out_dir)
    {
        eprintln!("create {}: {error}", out_dir.display());
        return std::process::ExitCode::from(3);
    }
    let null_out = out_dir.map(|path| path.join("null"));
    let corpus_out = out_dir.map(|path| path.join("recovery"));
    let null_report = options.null_report.clone().or_else(|| {
        null_out
            .as_ref()
            .map(|path| path.join(format!("{IMPLEMENTATION}-null-test.json")))
    });
    let corpus_report = options.recovery_report.clone().or_else(|| {
        corpus_out
            .as_ref()
            .map(|path| path.join(format!("{IMPLEMENTATION}-real-corpus.json")))
    });
    let association_out = options
        .association_report
        .clone()
        .or_else(|| out_dir.map(|path| path.join("association.json")));
    let attack_out = options
        .attack_report
        .clone()
        .or_else(|| out_dir.map(|path| path.join("attacks.json")));
    let (Some(null_report), Some(corpus_report), Some(association_out), Some(attack_out)) =
        (null_report, corpus_report, association_out, attack_out)
    else {
        eprintln!("--out-dir or all four explicit artifact report paths are required\n\n{USAGE}");
        return std::process::ExitCode::from(2);
    };

    let mut outcomes = Vec::new();
    if !options.aggregate_only {
        let (Some(null_dir), Some(corpus_dir), Some(out_dir), Some(null_out), Some(corpus_out)) = (
            options.null_dir.as_deref(),
            options.corpus_dir.as_deref(),
            out_dir,
            null_out.as_deref(),
            corpus_out.as_deref(),
        ) else {
            return fail(
                "--null-dir, --corpus-dir, and --out-dir are required unless --aggregate-only is used"
                    .to_owned(),
                2,
            );
        };
        let null_binary = match sibling_binary("audio-provenance-null-test") {
            Ok(path) => path,
            Err(error) => return fail(error, 3),
        };
        let corpus_binary = match sibling_binary("audio-provenance-corpus-bench") {
            Ok(path) => path,
            Err(error) => return fail(error, 3),
        };
        let association_binary = match sibling_binary("audio-provenance-association-test") {
            Ok(path) => path,
            Err(error) => return fail(error, 3),
        };
        let attack_binary = match sibling_binary("audio-provenance-attack") {
            Ok(path) => path,
            Err(error) => return fail(error, 3),
        };
        let null_workers = (options.workers / 2).max(1);
        let association_workers = options.workers.saturating_sub(null_workers).max(1);
        let mut null_args = vec![
            "--corpus-dir".to_owned(),
            null_dir.display().to_string(),
            "--out-dir".to_owned(),
            null_out.display().to_string(),
            "--ffmpeg".to_owned(),
            options.ffmpeg.clone(),
            "--workers".to_owned(),
            null_workers.to_string(),
            "--seed".to_owned(),
            options.seed.to_string(),
            "--duration".to_owned(),
            "30".to_owned(),
        ];
        if let Some(path) = &options.null_manifest {
            null_args.extend(["--manifest".to_owned(), path.display().to_string()]);
        }
        let tasks = vec![
            Task {
                name: "false_acceptance",
                program: null_binary,
                args: null_args,
                log: out_dir.join("null.log"),
            },
            Task {
                name: "locator_recovery",
                program: corpus_binary,
                args: vec![
                    "--real-dir".to_owned(),
                    corpus_dir.display().to_string(),
                    "--out-dir".to_owned(),
                    corpus_out.display().to_string(),
                    "--ffmpeg".to_owned(),
                    options.ffmpeg.clone(),
                    "--seed".to_owned(),
                    options.seed.to_string(),
                    "--duration".to_owned(),
                    "30".to_owned(),
                    "--no-synthetic".to_owned(),
                ],
                log: out_dir.join("recovery.log"),
            },
            Task {
                name: "recording_association",
                program: association_binary,
                args: vec![
                    "--corpus-dir".to_owned(),
                    corpus_dir.display().to_string(),
                    "--out".to_owned(),
                    association_out.display().to_string(),
                    "--ffmpeg".to_owned(),
                    options.ffmpeg.clone(),
                    "--workers".to_owned(),
                    association_workers.to_string(),
                    "--seed".to_owned(),
                    options.seed.to_string(),
                ],
                log: out_dir.join("association.log"),
            },
            Task {
                name: "adversarial",
                program: attack_binary,
                args: vec![
                    "campaign".to_owned(),
                    "--corpus-dir".to_owned(),
                    corpus_dir.display().to_string(),
                    "--out".to_owned(),
                    attack_out.display().to_string(),
                    "--ffmpeg".to_owned(),
                    options.ffmpeg.clone(),
                    "--items".to_owned(),
                    "13".to_owned(),
                    "--duration".to_owned(),
                    "30".to_owned(),
                    "--collusion-copies".to_owned(),
                    "16".to_owned(),
                    "--null-trials".to_owned(),
                    "1000".to_owned(),
                    "--seed".to_owned(),
                    options.seed.to_string(),
                ],
                log: out_dir.join("attacks.log"),
            },
        ];
        thread::scope(|scope| {
            let mut handles = Vec::new();
            for task in tasks {
                let timestamp = options.timestamp.as_deref();
                handles.push(scope.spawn(move || run_task(task, timestamp)));
            }
            for handle in handles {
                match handle.join() {
                    Ok(outcome) => outcomes.push(outcome),
                    Err(_) => outcomes.push(TaskOutcome {
                        name: "unknown",
                        exit_code: None,
                        measurement_completed: false,
                        log: String::new(),
                        error: Some("measurement thread panicked".to_owned()),
                    }),
                }
            }
        });
    }

    let (null, null_hash) = match read_json(&null_report) {
        Ok(value) => value,
        Err(error) => return fail(error, 4),
    };
    let (recovery, recovery_hash) = match read_json(&corpus_report) {
        Ok(value) => value,
        Err(error) => return fail(error, 4),
    };
    let (association, association_hash) = match read_json(&association_out) {
        Ok(value) => value,
        Err(error) => return fail(error, 4),
    };
    let (attacks, attacks_hash) = match read_json(&attack_out) {
        Ok(value) => value,
        Err(error) => return fail(error, 4),
    };

    if null.pointer("/codec/name").and_then(Value::as_str) != Some(IMPLEMENTATION)
        || recovery.pointer("/codec/name").and_then(Value::as_str) != Some(IMPLEMENTATION)
        || association
            .pointer("/production_implementation")
            .and_then(Value::as_str)
            != Some(IMPLEMENTATION)
    {
        return fail(
            "an artifact does not describe classical Watermark".to_owned(),
            4,
        );
    }
    let fp_trials = match u64_at(&null, "/totals/false_positive_trials") {
        Ok(value) => value,
        Err(error) => return fail(error, 4),
    };
    let fp_accepts = match u64_at(&null, "/totals/false_positive_accepts") {
        Ok(value) => value,
        Err(error) => return fail(error, 4),
    };
    let recovery_trials = match u64_at(&recovery, "/totals/trials") {
        Ok(value) => value,
        Err(error) => return fail(error, 4),
    };
    let exact_recoveries = match u64_at(&recovery, "/totals/exact_recoveries") {
        Ok(value) => value,
        Err(error) => return fail(error, 4),
    };
    let attack_rows = attacks
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let collusion = attack_rows
        .iter()
        .filter(|row| row.get("family").and_then(Value::as_str) == Some("collusion"));
    let collusion_trials = collusion.clone().count();
    let collusion_successes = collusion
        .filter(|row| row.get("attack_succeeded").and_then(Value::as_bool) == Some(true))
        .count();
    let association_trials = match u64_at(&association, "/summary/trials") {
        Ok(value) => value,
        Err(error) => return fail(error, 4),
    };
    let association_failed = match u64_at(&association, "/summary/failed") {
        Ok(value) => value,
        Err(error) => return fail(error, 4),
    };
    let association_errors = match u64_at(&association, "/summary/errors") {
        Ok(value) => value,
        Err(error) => return fail(error, 4),
    };
    let association_passed = match u64_at(&association, "/summary/passed") {
        Ok(value) => value,
        Err(error) => return fail(error, 4),
    };
    let null_items = null
        .get("corpus")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let recovery_items = recovery
        .get("corpus")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let recovery_real_items = recovery
        .get("corpus")
        .and_then(Value::as_array)
        .map_or(0, |items| {
            items
                .iter()
                .filter(|item| item.pointer("/source/kind").and_then(Value::as_str) == Some("file"))
                .count()
        });
    let association_items = association
        .get("corpus_items")
        .and_then(Value::as_u64)
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or(0);
    let adversarial_items = attacks
        .get("items")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let dataset_complete = null_items >= 1_000
        && recovery_real_items >= 13
        && association_items >= 13
        && adversarial_items >= 13;
    let qualification_complete = outcomes.iter().all(|run| run.measurement_completed)
        && association_errors == 0
        && dataset_complete;
    let meets_predeclared_targets = null.get("verdict").and_then(Value::as_str) == Some("pass")
        && recovery.get("verdict").and_then(Value::as_str) == Some("pass")
        && association_failed == 0
        && association_errors == 0;
    let mut artifacts = vec![
        artifact(&null_report, null_hash),
        artifact(&corpus_report, recovery_hash),
        artifact(&association_out, association_hash),
        artifact(&attack_out, attacks_hash),
    ];
    for manifest in [&options.null_manifest, &options.corpus_manifest]
        .into_iter()
        .flatten()
    {
        let bytes = match std::fs::read(manifest) {
            Ok(bytes) => bytes,
            Err(error) => return fail(format!("read {}: {error}", manifest.display()), 4),
        };
        artifacts.push(artifact(manifest, sha256_hex(&bytes)));
    }
    let report = BaselineReport {
        schema: SCHEMA,
        generated_at: options.timestamp,
        production_implementation: IMPLEMENTATION,
        experimental_implementations_included: Vec::new(),
        feature_isolation: "classical-apw-watermark default feature; apw-watermark-neural absent from dependency graph",
        seed: options.seed,
        qualification_complete,
        meets_predeclared_targets,
        false_positive: FalsePositiveBaseline {
            trials: fp_trials,
            accepts: fp_accepts,
            measured_rate: f64_at(&null, "/totals/overall_false_positive_rate"),
            upper_bound_95: f64_at(&null, "/totals/false_positive_upper_bound_95"),
        },
        mark_recovery: RecoveryBaseline {
            trials: recovery_trials,
            exact_recoveries,
            exact_recovery_rate: (recovery_trials > 0)
                .then_some(exact_recoveries as f64 / recovery_trials as f64),
        },
        recording_association: AssociationBaseline {
            trials: association_trials,
            passed: association_passed,
            failed: association_failed,
            errors: association_errors,
            association_recovery_rate: f64_at(&association, "/summary/association_recovery_rate"),
            alteration_rejection_rate: f64_at(&association, "/summary/alteration_rejection_rate"),
            local_region_seconds: f64_at(&association, "/predicates/local_region_seconds")
                .unwrap_or(0.0),
            maximum_unexplained_gap_seconds: f64_at(
                &association,
                "/predicates/maximum_unexplained_gap_seconds",
            )
            .unwrap_or(0.0),
            minimum_local_coverage: f64_at(&association, "/predicates/minimum_local_coverage")
                .unwrap_or(0.0),
        },
        adversarial: AttackBaseline {
            trials: attack_rows.len(),
            successful_attacks: attack_rows
                .iter()
                .filter(|row| row.get("attack_succeeded").and_then(Value::as_bool) == Some(true))
                .count(),
            collusion_trials,
            collusion_successes,
        },
        dataset_coverage: DatasetCoverage {
            null_items,
            recovery_items,
            recovery_real_items,
            association_items,
            adversarial_items,
            required_null_items: 1_000,
            required_characterisation_items: 13,
            complete: dataset_complete,
        },
        artifacts,
        runs: outcomes,
        notes: vec![
            "Locator recovery is measured separately from recording association and is not authentication.",
            "A zero observed false-positive rate is accompanied by the rule-of-three 95% upper bound.",
            "Simulated acoustic rows are not physical re-recording measurements.",
        ],
    };

    if let Some(parent) = report_path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return fail(format!("create {}: {error}", parent.display()), 4);
    }
    let json = match serde_json::to_vec_pretty(&report) {
        Ok(json) => json,
        Err(error) => return fail(format!("serialize baseline: {error}"), 4),
    };
    if let Err(error) = std::fs::write(report_path, &json) {
        return fail(format!("write {}: {error}", report_path.display()), 4);
    }
    let markdown_path = report_path.with_extension("md");
    if let Err(error) = std::fs::write(&markdown_path, markdown(&report)) {
        return fail(format!("write {}: {error}", markdown_path.display()), 4);
    }
    eprintln!(
        "wrote {} and {} (complete {}, targets {})",
        report_path.display(),
        markdown_path.display(),
        report.qualification_complete,
        report.meets_predeclared_targets
    );
    if report.qualification_complete && report.meets_predeclared_targets {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

fn fail(message: String, code: u8) -> std::process::ExitCode {
    eprintln!("{message}");
    std::process::ExitCode::from(code)
}
