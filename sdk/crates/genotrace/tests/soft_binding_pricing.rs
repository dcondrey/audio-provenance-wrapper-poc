#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// A test asserts; the workspace bans these in production code, where a panic is a defect rather
// than the reporting mechanism.

//! The forcing function: a soft binding with no measured false-positive rate cannot verify.
//!
//! `FalsePositiveRate` has no public constructor, so the only way into
//! `BindingEvaluation::SoftMark` is a real `audio-provenance-bench` null-test report. This file pins the
//! observable consequence, and the ceiling that makes `match == 1.0` an iff-test for a hard
//! binding.

use audio_provenance_core::VerificationStatus;
use genotrace::nulltest::{NullTestTable, RateBasis};
use genotrace::result::{
    BindingEvaluation, DEFAULT_SOFT_BINDING_THRESHOLD, MarkConfidence, MatchScore,
};
use genotrace::status::{
    BindingClass, SignatureOutcome, StatusInputs, TrustOutcome, derive_status,
};

fn status_for(binding: BindingClass) -> VerificationStatus {
    derive_status(StatusInputs::Candidate {
        signature: SignatureOutcome::Valid,
        binding,
        // The most favourable trust available, so a non-`verified` result is the binding's doing.
        trust: TrustOutcome::Anchored,
    })
    .status()
}

/// A GenoMark detection strong enough and covering enough to verify, with no null-test row behind
/// it, must not verify. A confidence with no false-positive rate is not actionable.
#[test]
fn an_unpriced_soft_binding_cannot_verify() {
    let unpriced = BindingEvaluation::SoftMarkUnpriced {
        confidence: MarkConfidence::Strong,
        score: MatchScore::soft(0.94),
        blocks_agreeing: 17,
        blocks_present: 18,
    };
    let class = unpriced.class(DEFAULT_SOFT_BINDING_THRESHOLD);
    assert_eq!(class, BindingClass::SoftMarkFalsePositiveRateUnknown);
    assert_ne!(status_for(class), VerificationStatus::Verified);
    assert!(
        unpriced.false_positive_rate().is_none(),
        "an unpriced binding must report no rate rather than an invented one"
    );

    // The same evidence, priced, is the one soft path that does verify.
    let table = NullTestTable::from_bench_report(GENOMARK_NULL_TEST.as_bytes()).unwrap();
    let rate = table.rate_for(genomark::ALGORITHM_ID).unwrap();
    let priced = BindingEvaluation::SoftMark {
        confidence: MarkConfidence::Strong,
        score: MatchScore::soft(0.94),
        blocks_agreeing: 17,
        blocks_present: 18,
        false_positive_rate: rate,
    };
    assert_eq!(
        priced.class(DEFAULT_SOFT_BINDING_THRESHOLD),
        BindingClass::SoftMarkStrongAtOrAboveThreshold
    );
    assert_eq!(
        status_for(priced.class(DEFAULT_SOFT_BINDING_THRESHOLD)),
        VerificationStatus::Verified
    );
}

/// The coverage guard: one agreeing block out of 27 is `match` 0.037, and reading that as
/// `verified` is exactly the hole the threshold closes.
#[test]
fn thin_coverage_fails_the_guard_even_when_priced() {
    let table = NullTestTable::from_bench_report(GENOMARK_NULL_TEST.as_bytes()).unwrap();
    let rate = table.rate_for(genomark::ALGORITHM_ID).unwrap();
    let thin = BindingEvaluation::SoftMark {
        confidence: MarkConfidence::Strong,
        score: MatchScore::soft(1.0 / 27.0),
        blocks_agreeing: 1,
        blocks_present: 27,
        false_positive_rate: rate,
    };
    let class = thin.class(DEFAULT_SOFT_BINDING_THRESHOLD);
    assert_eq!(class, BindingClass::SoftMarkStrongBelowThreshold);
    assert_eq!(status_for(class), VerificationStatus::Untrusted);

    // A genuinely short marked file with its one block present still passes.
    let short = BindingEvaluation::SoftMark {
        confidence: MarkConfidence::Strong,
        score: MatchScore::soft(1.0),
        blocks_agreeing: 1,
        blocks_present: 1,
        false_positive_rate: rate,
    };
    assert_eq!(
        short.class(DEFAULT_SOFT_BINDING_THRESHOLD),
        BindingClass::SoftMarkStrongAtOrAboveThreshold
    );
}

/// `match == 1.0` is an iff-test for a hard binding, so every soft basis is capped below it.
#[test]
fn only_a_hard_binding_reaches_one() {
    assert_eq!(MatchScore::hard_exact().value(), 1.0);
    assert_eq!(MatchScore::soft(1.0).value(), 0.99);
    assert_eq!(MatchScore::soft(2.5).value(), 0.99);
    assert_eq!(MatchScore::soft(f64::NAN).value(), 0.0);
    assert_eq!(MatchScore::soft(-1.0).value(), 0.0);
    assert_eq!(BindingEvaluation::HardExactContent.score().value(), 1.0);
    assert_eq!(
        BindingEvaluation::HardExactDecodedAudioOnly.score().value(),
        1.0
    );
}

/// Zero false accepts is not a false-positive rate of zero. The trial count only supports the
/// rule-of-three bound, and that is what the table reports.
#[test]
fn zero_accepts_reports_the_rule_of_three_bound() {
    let table = NullTestTable::from_bench_report(GENOMARK_NULL_TEST.as_bytes()).unwrap();
    let rate = table.rate_for(genomark::ALGORITHM_ID).unwrap();
    assert_eq!(rate.rate_basis(), RateBasis::UpperBound95);
    assert!(rate.value() > 0.0, "a zero rate would overstate the trials");
}

/// A bench fixture codec is the deliberately weak reference mark the bench uses to prove itself
/// honest. Its null-test result says nothing about GenoMark and must not price a GenoMark binding.
#[test]
fn a_bench_fixture_report_is_refused() {
    let fixture = GENOMARK_NULL_TEST.replace(
        r#""is_bench_fixture": false"#,
        r#""is_bench_fixture": true"#,
    );
    assert!(NullTestTable::from_bench_report(fixture.as_bytes()).is_err());

    let no_trials = GENOMARK_NULL_TEST.replace(
        r#""false_positive_trials": 370"#,
        r#""false_positive_trials": 0"#,
    );
    assert!(NullTestTable::from_bench_report(no_trials.as_bytes()).is_err());

    assert!(
        NullTestTable::empty()
            .rate_for(genomark::ALGORITHM_ID)
            .is_none()
    );
}

/// Shaped exactly as `audio-provenance-bench` emits it, with the fields this crate reads.
const GENOMARK_NULL_TEST: &str = r#"{
  "schema": "audio-provenance-bench/1",
  "codec": {"name": "genomark-lepqim-v1", "description": "", "is_bench_fixture": false, "payload_len": 7},
  "totals": {
    "false_positive_trials": 370,
    "false_positive_accepts": 0,
    "overall_false_positive_rate": 0.0,
    "false_positive_upper_bound_95": 0.008108108108108109
  }
}"#;

/// The coverage guard's own worked short-file case, driven through the real denominator rather than
/// a hand-built score. A file holding one whole block, with that block accepted, is `match` 1.0.
#[test]
fn a_short_marked_file_scores_full_coverage() {
    let rate = 44_100u32;
    let block_seconds = genomark::Capabilities::at(rate).block_seconds;

    // Just under one whole block: the floor of the duration is zero, and a naive denominator would
    // score an accepted block at zero and send the guard's passing example to `untrusted`.
    let short = silence(rate, block_seconds * 0.95);
    let (present, score) = genotrace::ladder::mark_coverage(1, &short);
    assert_eq!(present, 1);
    assert_eq!(score.value(), 0.99, "a soft basis is capped below 1.0");

    // A four-minute track with one agreeing block is thin coverage, and must stay thin.
    let long = silence(rate, 240.0);
    let (present, score) = genotrace::ladder::mark_coverage(1, &long);
    assert!(present >= 24, "expected many whole blocks, found {present}");
    assert!(
        score.value() < DEFAULT_SOFT_BINDING_THRESHOLD,
        "one block in four minutes scored {}",
        score.value()
    );

    // Nothing decoded over silence is coverage zero, not a division by zero.
    let (present, score) = genotrace::ladder::mark_coverage(0, &short);
    assert_eq!(present, 0);
    assert_eq!(score.value(), 0.0);
}

fn silence(rate: u32, seconds: f64) -> audio_provenance_audio::AudioBuffer {
    let frames = (seconds * f64::from(rate)) as usize;
    audio_provenance_audio::AudioBuffer::silence(rate, 1, frames).unwrap()
}
