#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// A test asserts; the workspace bans these in production code, where a panic is a defect rather
// than the reporting mechanism.

//! The false-positive measurement behind `reference::AFFIRMATION_COVERAGE`.
//!
//! Run it:
//!
//! ```text
//! cargo test -p apw_trace --test fingerprint_null_test -- --ignored --nocapture
//! ```
//!
//! It is `#[ignore]`d because it shells out to ffmpeg and takes minutes, not because it is
//! optional: the constant it prices is the acceptance threshold for a soft binding that can reach
//! `verified`, and a build that moves the constant without re-running this has an unmeasured gate.
//!
//! # What a trial is
//!
//! One ordered pair of DIFFERENT recordings: the reference constellation of item `i` compared
//! against the query of item `j`. That is exactly the predicate
//! [`apw_trace::fingerprint::compare`] applies at verification time, rate sweep included, so the
//! number priced here is the number the verifier uses.
//!
//! # Where the negatives come from
//!
//! Two sources, neither of which is this crate. The ffmpeg recipes are external synthesis with
//! per-item tempo, pitch and seed. The excerpts are disjoint slices of the bench's real music-like
//! corpus, which is the harder set: two slices of one recording share instrumentation, room and
//! mastering and differ only in time position, which is the closest an honest negative gets to a
//! positive.

use std::path::{Path, PathBuf};
use std::process::Command;

use audio_provenance_audio::AudioBuffer;
use apw_trace::fingerprint::reference::{AFFIRMATION_COVERAGE, ReferenceFingerprint, compare};
use apw_trace::fingerprint::score::ScoreLimits;
use apw_trace::fingerprint::{FingerprintQuery, MIN_QUERY_SECONDS};

/// Chosen BEFORE the measurement ran and before any transcode was verified: at most one accept in a
/// hundred non-matching comparisons, at 95% confidence. With zero accepts the rule of three needs
/// 300 trials to support it, which the pair construction below exceeds.
const FALSE_POSITIVE_TARGET: f64 = 0.01;

const ITEM_SECONDS: f64 = 10.0;
const RATE: u32 = 44_100;
const SYNTHETIC_ITEMS: usize = 20;
const EXCERPTS_PER_CORPUS_FILE: usize = 3;

fn ffmpeg() -> String {
    std::env::var("FFMPEG").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string())
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate sits two levels under the workspace root")
        .to_path_buf()
}

/// One music-like recipe per index: a kick and snare at the item's own tempo, a five-partial pad at
/// its own root, and a seeded noise layer. Nothing about the parameters is shared between items.
fn recipe(index: usize) -> String {
    let tempo = 0.32 + 0.031 * index as f64;
    let root = 82.0 + 11.7 * index as f64;
    let snare = 0.19 + 0.017 * ((index % 7) as f64);
    let seed = 100 + index;
    let voice = |detune: f64| {
        format!(
            "0.80*exp(-26*mod(t,{tempo}))*sin(2*PI*{root}*mod(t,{tempo}))\
             +0.40*exp(-13*mod(t+{snare},{tempo2}))*sin(2*PI*{mid}*mod(t,{tempo2}))\
             +0.20*exp(-120*mod(t,{hat}))*(2*random({seed})-1)\
             +0.16*(sin(2*PI*{p1}*t)+0.6*sin(2*PI*{p2}*t)+0.4*sin(2*PI*{p3}*t))",
            tempo = tempo,
            tempo2 = tempo * 2.0,
            hat = tempo / 2.0,
            root = root,
            mid = root * 3.17 + detune,
            snare = snare,
            seed = seed,
            p1 = root * 2.0 + detune,
            p2 = root * 3.01 + detune,
            p3 = root * 5.03 + detune,
        )
    };
    format!(
        "aevalsrc=exprs={}|{}:c=stereo:s={RATE}:d={ITEM_SECONDS}",
        voice(0.0).replace(',', "\\,"),
        voice(0.7).replace(',', "\\,"),
    )
}

fn run_ffmpeg(args: &[String]) -> Vec<u8> {
    let output = Command::new(ffmpeg())
        .args(args)
        .output()
        .expect("ffmpeg is on PATH or named by FFMPEG");
    assert!(
        output.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.is_empty(), "ffmpeg produced no audio");
    output.stdout
}

fn from_f32le(bytes: &[u8]) -> AudioBuffer {
    let frames = bytes.len() / (4 * 2);
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for frame in bytes.chunks_exact(8) {
        left.push(f32::from_le_bytes(frame[0..4].try_into().unwrap()));
        right.push(f32::from_le_bytes(frame[4..8].try_into().unwrap()));
    }
    AudioBuffer::from_channels(RATE, &[left, right]).unwrap()
}

fn synthetic(index: usize) -> AudioBuffer {
    let args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        &recipe(index),
        "-ac",
        "2",
        "-ar",
        &RATE.to_string(),
        "-f",
        "f32le",
        "pipe:1",
    ]
    .iter()
    .map(|value| (*value).to_string())
    .collect();
    from_f32le(&run_ffmpeg(&args))
}

fn excerpt(path: &Path, start: f64) -> AudioBuffer {
    let args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-ss",
        &start.to_string(),
        "-t",
        &ITEM_SECONDS.to_string(),
        "-i",
        &path.display().to_string(),
        "-ac",
        "2",
        "-ar",
        &RATE.to_string(),
        "-f",
        "f32le",
        "pipe:1",
    ]
    .iter()
    .map(|value| (*value).to_string())
    .collect();
    from_f32le(&run_ffmpeg(&args))
}

struct Item {
    id: String,
    /// Excerpts of one corpus file share a source, so a pair drawn from the same one is still a
    /// pair of different recordings but is NOT independent evidence about unrelated works. The
    /// group is kept so the report can separate the two populations.
    group: String,
    reference: ReferenceFingerprint,
    query: FingerprintQuery,
}

fn corpus_files() -> Vec<PathBuf> {
    let directory = repo_root().join("bench-out").join("corpus");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{} is unreadable: {error}", directory.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "wav"))
        .collect();
    paths.sort();
    paths
}

fn build_items() -> Vec<Item> {
    let mut items = Vec::new();
    let mut push = |id: String, group: String, audio: &AudioBuffer| {
        let Some(reference) = ReferenceFingerprint::from_audio(audio).unwrap() else {
            panic!("{id} is too long for a reference");
        };
        let query = FingerprintQuery::from_audio(audio).unwrap();
        assert!(
            query.seconds() >= MIN_QUERY_SECONDS,
            "{id} is {:.2} s, under the {MIN_QUERY_SECONDS} s query floor, so its trials would be \
             vacuous",
            query.seconds()
        );
        items.push(Item {
            id,
            group,
            reference,
            query,
        });
    };

    for index in 0..SYNTHETIC_ITEMS {
        push(
            format!("synth_{index:02}"),
            format!("synth_{index:02}"),
            &synthetic(index),
        );
    }
    for path in corpus_files() {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        for slice in 0..EXCERPTS_PER_CORPUS_FILE {
            let start = slice as f64 * ITEM_SECONDS;
            push(
                format!("{stem}@{start:.0}s"),
                stem.clone(),
                &excerpt(&path, start),
            );
        }
    }
    items
}

/// The measurement. Every ordered pair of different recordings, the exact acceptance predicate, and
/// a breakdown of which gate did the rejecting.
#[test]
#[ignore = "shells out to ffmpeg and takes minutes; run it when the threshold changes"]
fn the_affirmation_threshold_meets_its_false_positive_target() {
    let items = build_items();
    println!("items: {}", items.len());

    let limits = ScoreLimits::default();
    let mut trials = 0usize;
    let mut accepts = 0usize;
    let mut gate_passes = 0usize;
    let mut same_source_trials = 0usize;
    let mut max_coverage: f64 = 0.0;
    let mut max_gate_passing_coverage: f64 = 0.0;
    let mut worst = String::new();

    for reference in &items {
        for query in &items {
            if reference.id == query.id {
                continue;
            }
            trials += 1;
            if reference.group == query.group {
                same_source_trials += 1;
            }
            let affirmation = compare(&reference.reference, &query.query, limits).unwrap();
            if affirmation.coverage > max_coverage {
                max_coverage = affirmation.coverage;
                worst = format!(
                    "{} vs {}: coverage {:.4}, peak {}, outside {}, span {:.2} s, gates {}",
                    reference.id,
                    query.id,
                    affirmation.coverage,
                    affirmation.peak,
                    affirmation.best_outside_peak,
                    affirmation.aligned_span_seconds,
                    affirmation.gates_passed
                );
            }
            if affirmation.gates_passed {
                gate_passes += 1;
                max_gate_passing_coverage = max_gate_passing_coverage.max(affirmation.coverage);
                println!(
                    "  gate pass: {} vs {} coverage {:.4} peak {} outside {} span {:.2}",
                    reference.id,
                    query.id,
                    affirmation.coverage,
                    affirmation.peak,
                    affirmation.best_outside_peak,
                    affirmation.aligned_span_seconds
                );
            }
            if affirmation.affirmed {
                accepts += 1;
                println!("  ACCEPT: {} vs {}", reference.id, query.id);
            }
        }
    }

    let rate = accepts as f64 / trials as f64;
    let upper_bound_95 = if accepts == 0 {
        3.0 / trials as f64
    } else {
        rate
    };
    println!("threshold (coverage)          {AFFIRMATION_COVERAGE:.2}");
    println!("trials                        {trials}");
    println!("  of which same-source pairs  {same_source_trials}");
    println!("alignment gates passed        {gate_passes}");
    println!("affirmed (false positives)    {accepts}");
    println!("observed rate                 {rate:.6}");
    println!("95% upper bound               {upper_bound_95:.6}");
    println!("max coverage, any pair        {max_coverage:.4}");
    println!("max coverage, gates passed    {max_gate_passing_coverage:.4}");
    println!("worst pair                    {worst}");

    assert!(
        trials >= 300,
        "{trials} trials cannot support a {FALSE_POSITIVE_TARGET} target at 95% confidence"
    );
    assert!(
        upper_bound_95 <= FALSE_POSITIVE_TARGET,
        "measured false-positive bound {upper_bound_95:.6} exceeds the {FALSE_POSITIVE_TARGET} \
         target: the threshold is not priced"
    );
}
