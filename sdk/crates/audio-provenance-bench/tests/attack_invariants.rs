#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use audio_provenance_audio::AudioBuffer;
use audio_provenance_bench::attacks::erase::{ConstantOffset, estimate_residues};
use audio_provenance_bench::attacks::geometry::{PairBandGeometry, PunctureRule};
use audio_provenance_bench::attacks::statistic::{DEFAULT_MAX_PASSES, analyse, drive};
use audio_provenance_bench::audio::from_channels;
use audio_provenance_bench::dsp::Rng;

const SAMPLE_RATE: u32 = 48_000;
const STEP: f64 = 0.8;

fn geometry() -> PairBandGeometry {
    let edges: Vec<f64> = (0..41)
        .map(|index| (20 + 2 * index) as f64 * 44_100.0 / 1024.0)
        .collect();
    PairBandGeometry::new(SAMPLE_RATE, 1024, 2, 2, 416, STEP, edges, (0..20).collect())
        .expect("published geometry is valid")
}

fn noise(seconds: f64) -> AudioBuffer {
    let frames = (seconds * f64::from(SAMPLE_RATE)) as usize;
    let mut rng = Rng::new(0x9E37_79B9);
    let mut low = 0.0f32;
    let plane: Vec<f32> = (0..frames)
        .map(|_| {
            low = 0.98 * low + 0.02 * rng.next_gaussian();
            0.2 * (low * 6.0 + 0.3 * rng.next_gaussian())
        })
        .collect();
    from_channels(SAMPLE_RATE, &[plane.clone(), plane]).expect("noise is admissible")
}

/// An attack that applied one spectral multiply and booked the shift it ASKED for would land near
/// half strength, because weighted overlap-add returns only part of a per-frame change to the next
/// analysis of the same slot. Every removal rate in the attack report is only meaningful if the
/// attack actually moved the statistic where it said, so that closure is pinned here rather than
/// assumed.
#[test]
fn drive_reaches_the_shift_it_asked_for() {
    let geometry = geometry();
    let puncture = PunctureRule::new(1e-7, 1e-12, 10);
    let audio = noise(8.0);
    let target = ConstantOffset::new("half_step", 0.5);

    let (moved, report) = drive(
        &audio,
        &geometry,
        puncture,
        &target,
        7,
        DEFAULT_MAX_PASSES,
        STEP / 20.0,
    )
    .expect("drive runs on admissible audio");

    let asked = STEP / 2.0;
    let achieved = report
        .achieved_shift_median_nepers
        .expect("some slot carried a statistic");
    assert!(
        (achieved - asked).abs() < 0.1 * asked,
        "median achieved shift {achieved} is not within 10% of the {asked} asked for; report {report:?}"
    );
    assert!(
        report
            .residual_to_target_p95_nepers
            .is_some_and(|residual| residual <= STEP / 20.0),
        "the 95th percentile residual to target did not reach the embedder's own closure budget: {report:?}"
    );

    let before = analyse(&audio, &geometry, puncture).expect("analysis runs");
    let after = analyse(&moved, &geometry, puncture).expect("analysis runs");
    assert_eq!(before.len(), after.len());
}

/// The coset a slot sits on is the whole of what the detector reads, and it is estimable with no
/// key. Unmarked audio has no lattice, so its residues do not cluster; this pins the control the
/// transplant and forgery rows are read against.
#[test]
fn unmarked_audio_shows_no_lattice() {
    let geometry = geometry();
    let puncture = PunctureRule::new(1e-7, 1e-12, 10);
    let audio = noise(40.0);
    let measured = analyse(&audio, &geometry, puncture).expect("analysis runs");
    let estimate = estimate_residues(&measured, &geometry);
    let concentration = estimate.concentration.expect("residues were estimated");
    assert!(
        concentration < 0.85,
        "unmarked audio clustered at {concentration}, which would make the marked-file control meaningless"
    );
}
