use crate::spec;
use audio_provenance_audio::AudioBuffer;
use audio_provenance_bench::attacks::collusion;
use audio_provenance_bench::attacks::denoise::spectral_subtraction;
use audio_provenance_bench::attacks::desync;
use audio_provenance_bench::attacks::erase::{
    ConstantOffset, CosetTransplant, Flatten, RandomOffset, ResidueEstimate, estimate_residues,
};
use audio_provenance_bench::attacks::geometry::{PairBandGeometry, PunctureRule};
use audio_provenance_bench::attacks::report::{
    AttackFamily, AttackReport, AttackRow, AttackerKnowledge, RemovalVerdict, THREAT_MODEL,
    classify, hex,
};
use audio_provenance_bench::attacks::statistic::{
    DEFAULT_MAX_PASSES, DriveReport, SlotTargets, analyse, apply_fixed_tilt, drive, measured_shift,
};
use audio_provenance_bench::corpus::CorpusItem;
use audio_provenance_bench::perceptual::{self, PERCEPTUAL_LIMITS};
use audio_provenance_bench::ports::CommandRunner;
use genomark::payload::Payload;
use genomark::{DetectionOutcome, GenoMark};
use serde_json::Value;
use std::collections::BTreeMap;

/// Slot statistic residual the attacks converge to, one twentieth of the lattice period. It is the
/// embedder's own closure budget, so an attack that reaches it has moved the statistic as precisely
/// as the embedder placed it.
const CONVERGENCE_NEPERS: f64 = genomark::params::DELTA / 20.0;

#[derive(Debug)]
pub struct Options<'a> {
    pub victim: &'a GenoMark,
    pub attacker: &'a GenoMark,
    pub geometry: PairBandGeometry,
    pub puncture: PunctureRule,
    pub runner: &'a dyn CommandRunner,
    pub ffmpeg: String,
    pub seed: u64,
    pub collusion_copies: usize,
    pub forgery_null_trials: usize,
}

#[derive(Debug, Clone)]
pub struct Observed {
    pub payload: Option<Vec<u8>>,
    pub class: String,
    pub blocks: usize,
}

pub fn observe(outcome: &DetectionOutcome) -> Observed {
    Observed {
        payload: outcome.payload().map(|value| value.to_bytes().to_vec()),
        class: outcome.class().as_str().to_owned(),
        blocks: outcome.blocks_accepted(),
    }
}

fn params(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

struct Case<'a> {
    item: &'a str,
    attack: String,
    family: AttackFamily,
    knowledge: AttackerKnowledge,
    params: BTreeMap<String, Value>,
    expected: &'a [u8],
    baseline: &'a Observed,
    note: Option<String>,
}

fn finish(
    case: Case<'_>,
    attacked: &Observed,
    reference: Option<&AudioBuffer>,
    produced: Option<&AudioBuffer>,
    drive_report: Option<DriveReport>,
) -> AttackRow {
    let verdict = classify(
        case.expected,
        case.baseline.payload.as_deref(),
        attacked.payload.as_deref(),
    );
    let perceptual = match (reference, produced) {
        (Some(before), Some(after)) => perceptual::measure(before, after).ok(),
        _ => None,
    };
    let succeeded = match case.family {
        AttackFamily::Forgery => verdict == RemovalVerdict::Survived,
        AttackFamily::Control => false,
        _ => matches!(
            verdict,
            RemovalVerdict::Erased | RemovalVerdict::Substituted
        ),
    };
    AttackRow {
        item: case.item.to_owned(),
        attack: case.attack,
        family: case.family,
        knowledge: case.knowledge,
        knowledge_note: case.knowledge.describe(),
        params: case.params,
        verdict,
        attack_succeeded: succeeded,
        expected_payload_hex: hex(case.expected),
        baseline_payload_hex: case.baseline.payload.as_deref().map(hex),
        attacked_payload_hex: attacked.payload.as_deref().map(hex),
        baseline_class: Some(case.baseline.class.clone()),
        attacked_class: Some(attacked.class.clone()),
        baseline_blocks_accepted: case.baseline.blocks,
        attacked_blocks_accepted: attacked.blocks,
        length_preserving: reference.is_some(),
        segmental_snr_db: perceptual.as_ref().and_then(|value| value.segmental_snr_db),
        noise_to_mask_mean_db: perceptual
            .as_ref()
            .and_then(|value| value.noise_to_mask_mean_db),
        noise_to_mask_max_db: perceptual
            .as_ref()
            .and_then(|value| value.noise_to_mask_max_db),
        peak_residual_dbfs: perceptual
            .as_ref()
            .and_then(|value| value.peak_residual_dbfs),
        drive: drive_report,
        note: case.note,
    }
}

fn payload_for(index: usize) -> Result<Payload, String> {
    Payload::new(
        genomark::payload::VERSION,
        1,
        0x0000_A100_0000 + index as u64,
    )
    .map_err(|error| error.to_string())
}

/// One removal attack: what to drive the statistic to, what the attacker must hold, and the
/// parameters the row records.
type StatisticAttack = (
    Box<dyn SlotTargets>,
    AttackerKnowledge,
    BTreeMap<String, Value>,
);

fn statistic_attacks() -> Vec<StatisticAttack> {
    vec![
        (
            Box::new(ConstantOffset::new("static_eq_half_step", 0.5)),
            AttackerKnowledge::SpecOnly,
            params(&[
                ("offset_fraction_of_step", 0.5.into()),
                ("needs_analysis_of_the_file", false.into()),
            ]),
        ),
        (
            Box::new(ConstantOffset::new("static_eq_quarter_step", 0.25)),
            AttackerKnowledge::SpecOnly,
            params(&[("offset_fraction_of_step", 0.25.into())]),
        ),
        (
            Box::new(ConstantOffset::new("static_eq_eighth_step", 0.125)),
            AttackerKnowledge::SpecOnly,
            params(&[("offset_fraction_of_step", 0.125.into())]),
        ),
        (
            Box::new(RandomOffset::new("random_offset_full_step", 1.0)),
            AttackerKnowledge::SpecOnly,
            params(&[("span_fraction_of_step", 1.0.into())]),
        ),
        (
            Box::new(RandomOffset::new("random_offset_half_step", 0.5)),
            AttackerKnowledge::SpecOnly,
            params(&[("span_fraction_of_step", 0.5.into())]),
        ),
        (
            Box::new(Flatten::new("flatten_median_5_slots", 2)),
            AttackerKnowledge::SpecOnly,
            params(&[("median_half_window_slots", 2.into())]),
        ),
    ]
}

#[allow(clippy::too_many_lines)]
pub fn run(items: &[CorpusItem], options: &Options<'_>) -> Result<AttackReport, String> {
    let mut rows: Vec<AttackRow> = Vec::new();
    let mut marked_copies: Vec<AudioBuffer> = Vec::new();
    let mut colluded_item: Option<String> = None;
    let mut expected_for_collusion: Vec<u8> = Vec::new();
    let mut collusion_baseline: Option<Observed> = None;

    for (index, item) in items.iter().enumerate() {
        let payload = payload_for(index)?;
        let expected = payload.to_bytes().to_vec();
        let marked = options
            .victim
            .embed(&item.audio, payload)
            .map_err(|error| format!("embed {}: {error}", item.meta.id))?;
        let baseline = observe(
            &options
                .victim
                .detect(&marked)
                .map_err(|error| format!("detect {}: {error}", item.meta.id))?,
        );

        rows.push(finish(
            Case {
                item: &item.meta.id,
                attack: "embed_reference".to_owned(),
                family: AttackFamily::Control,
                knowledge: AttackerKnowledge::ProfileKey,
                params: params(&[("lattice_step_nepers", genomark::params::DELTA.into())]),
                expected: &expected,
                baseline: &baseline,
                note: Some(
                    "Not an attack. The perceptual cost of EMBEDDING the mark, measured original \
                     against marked, so every removal row can be read against what the mark itself \
                     cost."
                        .to_owned(),
                ),
            },
            &baseline,
            Some(&item.audio),
            Some(&marked),
            None,
        ));

        let marked_statistics = analyse(&marked, &options.geometry, options.puncture)
            .map_err(|error| format!("analyse {}: {error}", item.meta.id))?;
        let unmarked_statistics = analyse(&item.audio, &options.geometry, options.puncture)
            .map_err(|error| format!("analyse unmarked {}: {error}", item.meta.id))?;
        let marked_residues = estimate_residues(&marked_statistics, &options.geometry);
        let unmarked_residues = estimate_residues(&unmarked_statistics, &options.geometry);

        for (targets, knowledge, mut parameters) in statistic_attacks() {
            let (attacked_audio, report) = drive(
                &marked,
                &options.geometry,
                options.puncture,
                targets.as_ref(),
                options.seed ^ index as u64,
                DEFAULT_MAX_PASSES,
                CONVERGENCE_NEPERS,
            )
            .map_err(|error| format!("{}: {error}", targets.name()))?;
            let attacked = observe(
                &options
                    .victim
                    .detect(&attacked_audio)
                    .map_err(|error| format!("detect attacked {}: {error}", item.meta.id))?,
            );
            parameters.insert(
                "lattice_step_nepers".to_owned(),
                genomark::params::DELTA.into(),
            );
            rows.push(finish(
                Case {
                    item: &item.meta.id,
                    attack: targets.name().to_owned(),
                    family: AttackFamily::Estimation,
                    knowledge,
                    params: parameters,
                    expected: &expected,
                    baseline: &baseline,
                    note: None,
                },
                &attacked,
                Some(&marked),
                Some(&attacked_audio),
                Some(report),
            ));
        }

        for asked in [0.4f64, 0.9] {
            let tilted = apply_fixed_tilt(&marked, &options.geometry, asked)
                .map_err(|error| format!("fixed tilt: {error}"))?;
            let after = analyse(&tilted, &options.geometry, options.puncture)
                .map_err(|error| format!("analyse tilted: {error}"))?;
            let attacked = observe(
                &options
                    .victim
                    .detect(&tilted)
                    .map_err(|error| format!("detect tilted: {error}"))?,
            );
            rows.push(finish(
                Case {
                    item: &item.meta.id,
                    attack: format!("static_eq_open_loop_{asked}_nepers"),
                    family: AttackFamily::Estimation,
                    knowledge: AttackerKnowledge::SpecOnly,
                    params: params(&[
                        ("asked_shift_nepers", asked.into()),
                        ("cell_gain_db", (20.0 * (asked / 4.0).exp().log10()).into()),
                        ("passes", 1.into()),
                        ("measured_the_file", false.into()),
                    ]),
                    expected: &expected,
                    baseline: &baseline,
                    note: Some(
                        "One fixed equaliser curve of the band's published shape, applied open                          loop. The file is never analysed and no key is involved."
                            .to_owned(),
                    ),
                },
                &attacked,
                Some(&marked),
                Some(&tilted),
                Some(measured_shift(&marked_statistics, &after, 1)),
            ));
        }

        let denoised = spectral_subtraction(&marked, 1024, 2.0, 0.05, 0.10)
            .map_err(|error| format!("denoise {}: {error}", item.meta.id))?;
        let attacked = observe(
            &options
                .victim
                .detect(&denoised)
                .map_err(|error| format!("detect denoised: {error}"))?,
        );
        rows.push(finish(
            Case {
                item: &item.meta.id,
                attack: "spectral_subtraction_denoise".to_owned(),
                family: AttackFamily::Estimation,
                knowledge: AttackerKnowledge::SpecOnly,
                params: params(&[
                    ("frame", 1024.into()),
                    ("over_subtraction", 2.0.into()),
                    ("spectral_floor", 0.05.into()),
                    ("noise_percentile", 0.10.into()),
                ]),
                expected: &expected,
                baseline: &baseline,
                note: Some(
                    "An off-the-shelf stationary-noise restoration pass, not a spec-aware attack."
                        .to_owned(),
                ),
            },
            &attacked,
            Some(&marked),
            Some(&denoised),
            None,
        ));

        for fraction in [0.0005, 0.001, 0.002, 0.003, 0.004, 0.005, 0.0075, -0.005] {
            let warped = desync::playback_rate(&marked, fraction)
                .map_err(|error| format!("playback rate: {error}"))?;
            let attacked = observe(
                &options
                    .victim
                    .detect(&warped)
                    .map_err(|error| format!("detect rate: {error}"))?,
            );
            rows.push(finish(
                Case {
                    item: &item.meta.id,
                    attack: format!("playback_rate_{}pct", fraction * 100.0),
                    family: AttackFamily::Desynchronisation,
                    knowledge: AttackerKnowledge::SpecOnly,
                    params: params(&[("speed_fraction", fraction.into())]),
                    expected: &expected,
                    baseline: &baseline,
                    note: Some(
                        "Time and pitch move together. Length changes, so a sample-aligned \
                         perceptual figure against the marked file is not defined and is null."
                            .to_owned(),
                    ),
                },
                &attacked,
                None,
                None,
                None,
            ));
        }

        for fraction in [0.001, 0.0025, 0.005, 0.006, 0.0075, 0.01, -0.01] {
            let stretched =
                desync::time_stretch(&marked, fraction, &options.ffmpeg, options.runner)
                    .map_err(|error| format!("time stretch: {error}"))?;
            let attacked = observe(
                &options
                    .victim
                    .detect(&stretched)
                    .map_err(|error| format!("detect stretch: {error}"))?,
            );
            rows.push(finish(
                Case {
                    item: &item.meta.id,
                    attack: format!("time_stretch_{}pct", fraction * 100.0),
                    family: AttackFamily::Desynchronisation,
                    knowledge: AttackerKnowledge::SpecOnly,
                    params: params(&[
                        ("tempo_fraction", fraction.into()),
                        ("filter", "atempo".into()),
                        ("pitch_preserved", true.into()),
                    ]),
                    expected: &expected,
                    baseline: &baseline,
                    note: None,
                },
                &attacked,
                None,
                None,
                None,
            ));
        }

        for cents in [1.0, 2.0, 3.0, 5.0, 6.0, 7.0, 8.0, 10.0, 25.0, -10.0] {
            let shifted = desync::pitch_shift(&marked, cents, &options.ffmpeg, options.runner)
                .map_err(|error| format!("pitch shift: {error}"))?;
            let attacked = observe(
                &options
                    .victim
                    .detect(&shifted)
                    .map_err(|error| format!("detect pitch: {error}"))?,
            );
            rows.push(finish(
                Case {
                    item: &item.meta.id,
                    attack: format!("pitch_shift_{cents}_cents"),
                    family: AttackFamily::Desynchronisation,
                    knowledge: AttackerKnowledge::SpecOnly,
                    params: params(&[
                        ("cents", cents.into()),
                        ("filter", "asetrate+aresample+atempo".into()),
                        ("tempo_preserved", true.into()),
                    ]),
                    expected: &expected,
                    baseline: &baseline,
                    note: None,
                },
                &attacked,
                None,
                None,
                None,
            ));
        }

        for period in [100_000usize, 10_000, 1_000, 200] {
            let dropped = desync::drop_samples(&marked, period)
                .map_err(|error| format!("drop samples: {error}"))?;
            let attacked = observe(
                &options
                    .victim
                    .detect(&dropped)
                    .map_err(|error| format!("detect drop: {error}"))?,
            );
            rows.push(finish(
                Case {
                    item: &item.meta.id,
                    attack: format!("drop_1_sample_in_{period}"),
                    family: AttackFamily::Desynchronisation,
                    knowledge: AttackerKnowledge::SpecOnly,
                    params: params(&[("drop_period_frames", period.into())]),
                    expected: &expected,
                    baseline: &baseline,
                    note: None,
                },
                &attacked,
                None,
                None,
                None,
            ));
        }

        for max_fraction in [0.0005, 0.002] {
            let warped = desync::random_warp(&marked, max_fraction, 2.0, options.seed ^ 0x5A5A)
                .map_err(|error| format!("random warp: {error}"))?;
            let attacked = observe(
                &options
                    .victim
                    .detect(&warped)
                    .map_err(|error| format!("detect warp: {error}"))?,
            );
            rows.push(finish(
                Case {
                    item: &item.meta.id,
                    attack: format!("random_warp_{}pct_2s_segments", max_fraction * 100.0),
                    family: AttackFamily::Desynchronisation,
                    knowledge: AttackerKnowledge::SpecOnly,
                    params: params(&[
                        ("max_segment_speed_fraction", max_fraction.into()),
                        ("segment_seconds", 2.0.into()),
                    ]),
                    expected: &expected,
                    baseline: &baseline,
                    note: None,
                },
                &attacked,
                None,
                None,
                None,
            ));
        }

        let attacker_payload = payload_for(index + 900)?;
        let overwritten = options
            .attacker
            .embed(&marked, attacker_payload)
            .map_err(|error| format!("overwrite embed: {error}"))?;
        let attacked = observe(
            &options
                .victim
                .detect(&overwritten)
                .map_err(|error| format!("detect overwritten: {error}"))?,
        );
        let attacker_view = observe(
            &options
                .attacker
                .detect(&overwritten)
                .map_err(|error| format!("attacker detect: {error}"))?,
        );
        rows.push(finish(
            Case {
                item: &item.meta.id,
                attack: "overwrite_with_attacker_key".to_owned(),
                family: AttackFamily::Overwriting,
                knowledge: AttackerKnowledge::SpecOnly,
                params: params(&[
                    (
                        "attacker_namespace",
                        i64::from(options.attacker.namespace()).into(),
                    ),
                    (
                        "attacker_payload_recovered",
                        attacker_view.payload.as_deref().map(hex).into(),
                    ),
                ]),
                expected: &expected,
                baseline: &baseline,
                note: Some(format!(
                    "The attacker ran the published embedder under a key of their own. Under the \
                     attacker's key the file now reads {}.",
                    attacker_view
                        .payload
                        .as_deref()
                        .map_or_else(|| "nothing".to_owned(), hex)
                )),
            },
            &attacked,
            Some(&marked),
            Some(&overwritten),
            None,
        ));

        let carrier = &items[(index + 1) % items.len()].audio;
        let carrier_id = items[(index + 1) % items.len()].meta.id.clone();

        let transplant =
            CosetTransplant::new("transplant_full_file", marked_residues.residues.clone());
        let (forged, report) = drive(
            carrier,
            &options.geometry,
            options.puncture,
            &transplant,
            options.seed ^ 0xF0F0,
            DEFAULT_MAX_PASSES,
            CONVERGENCE_NEPERS,
        )
        .map_err(|error| format!("transplant: {error}"))?;
        let attacked = observe(
            &options
                .victim
                .detect(&forged)
                .map_err(|error| format!("detect transplant: {error}"))?,
        );
        rows.push(finish(
            Case {
                item: &item.meta.id,
                attack: "transplant_coset_onto_other_audio".to_owned(),
                family: AttackFamily::Forgery,
                knowledge: AttackerKnowledge::OneMarkedFile,
                params: params(&[
                    ("carrier", carrier_id.clone().into()),
                    ("donor_blocks", marked_residues.blocks_observed.into()),
                    (
                        "donor_residue_concentration",
                        marked_residues.concentration.into(),
                    ),
                    (
                        "unmarked_residue_concentration",
                        unmarked_residues.concentration.into(),
                    ),
                ]),
                expected: &expected,
                baseline: &baseline,
                note: Some(format!(
                    "The carrier is UNRELATED audio ({carrier_id}). A `survived` verdict here means \
                     unrelated audio now decodes to the donor's payload. No key was used."
                )),
            },
            &attacked,
            Some(carrier),
            Some(&forged),
            Some(report),
        ));

        let one_block: Vec<Option<f64>> = marked_statistics
            .iter()
            .take(genomark::params::BLOCK_SLOTS)
            .copied()
            .collect();
        let single = estimate_residues(&one_block, &options.geometry);
        let transplant = CosetTransplant::new("transplant_one_block", single.residues.clone());
        let (forged, report) = drive(
            carrier,
            &options.geometry,
            options.puncture,
            &transplant,
            options.seed ^ 0xF0F1,
            DEFAULT_MAX_PASSES,
            CONVERGENCE_NEPERS,
        )
        .map_err(|error| format!("transplant one block: {error}"))?;
        let attacked = observe(
            &options
                .victim
                .detect(&forged)
                .map_err(|error| format!("detect transplant one block: {error}"))?,
        );
        rows.push(finish(
            Case {
                item: &item.meta.id,
                attack: "transplant_from_one_block".to_owned(),
                family: AttackFamily::Forgery,
                knowledge: AttackerKnowledge::OneMarkedFile,
                params: params(&[
                    ("carrier", carrier_id.clone().into()),
                    ("donor_blocks", 1.into()),
                    (
                        "donor_seconds",
                        (genomark::params::BLOCK_FRAMES as f64
                            * f64::from(options.geometry.hop() as u32)
                            / f64::from(options.geometry.sample_rate()))
                        .into(),
                    ),
                ]),
                expected: &expected,
                baseline: &baseline,
                note: Some("How little donor audio the transplant needs: one block.".to_owned()),
            },
            &attacked,
            Some(carrier),
            Some(&forged),
            Some(report),
        ));

        let chosen = payload_for(index + 500)?;
        let chosen_bytes = chosen.to_bytes().to_vec();
        let forged_residues = spec::forged_residues(&marked_residues.residues, payload, chosen);
        let transplant = CosetTransplant::new("forge_chosen_payload", forged_residues);
        let (forged, report) = drive(
            carrier,
            &options.geometry,
            options.puncture,
            &transplant,
            options.seed ^ 0xF0F2,
            DEFAULT_MAX_PASSES,
            CONVERGENCE_NEPERS,
        )
        .map_err(|error| format!("forge: {error}"))?;
        let attacked = observe(
            &options
                .victim
                .detect(&forged)
                .map_err(|error| format!("detect forged: {error}"))?,
        );
        let planted = Observed {
            payload: Some(chosen_bytes.clone()),
            class: baseline.class.clone(),
            blocks: baseline.blocks,
        };
        rows.push(finish(
            Case {
                item: &item.meta.id,
                attack: "forge_chosen_payload_under_victim_key".to_owned(),
                family: AttackFamily::Forgery,
                knowledge: AttackerKnowledge::MarkedFileAndKnownPayload,
                params: params(&[
                    ("carrier", carrier_id.into()),
                    (
                        "chosen_locator",
                        format!("{:012x}", chosen.locator()).into(),
                    ),
                    (
                        "donor_locator",
                        format!("{:012x}", payload.locator()).into(),
                    ),
                ]),
                expected: &chosen_bytes,
                baseline: &planted,
                note: Some(
                    "The attacker knows one marked file and the payload it carries, which for a \
                     registered release is public registry data. Subtracting the known coded bit \
                     from the observed coset leaves the key-derived dither, and any chosen payload \
                     can then be written against it. A `survived` verdict means the detector, \
                     holding the victim's secret key, accepted a payload the attacker chose."
                        .to_owned(),
                ),
            },
            &attacked,
            Some(carrier),
            Some(&forged),
            Some(report),
        ));

        if index == 0 {
            colluded_item = Some(item.meta.id.clone());
            expected_for_collusion = expected.clone();
            collusion_baseline = Some(baseline.clone());
            marked_copies.push(marked.clone());
            for copy in 1..options.collusion_copies {
                let other = payload_for(index + 100 + copy)?;
                marked_copies.push(
                    options
                        .victim
                        .embed(&item.audio, other)
                        .map_err(|error| format!("collusion embed: {error}"))?,
                );
            }
        }
    }

    if let (Some(item), Some(baseline)) = (colluded_item.as_ref(), collusion_baseline.as_ref())
        && marked_copies.len() >= 2
    {
        for count in [2usize, 3, 5, options.collusion_copies] {
            if count > marked_copies.len() {
                continue;
            }
            let mean = collusion::average(&marked_copies[..count])
                .map_err(|error| format!("collusion average: {error}"))?;
            let attacked = observe(
                &options
                    .victim
                    .detect(&mean)
                    .map_err(|error| format!("detect collusion: {error}"))?,
            );
            rows.push(finish(
                Case {
                    item,
                    attack: format!("collude_average_{count}_copies"),
                    family: AttackFamily::Collusion,
                    knowledge: AttackerKnowledge::ManyMarkedCopiesOfOneRecording,
                    params: params(&[("copies", count.into())]),
                    expected: &expected_for_collusion,
                    baseline,
                    note: Some(
                        "Every copy is the same recording marked with a DIFFERENT payload under \
                         one key. The mean is what a colluding set of licensees can publish."
                            .to_owned(),
                    ),
                },
                &attacked,
                Some(&marked_copies[0]),
                Some(&mean),
                None,
            ));

            if count == options.collusion_copies {
                for strength in [1.0f64, 1.6] {
                    let scrubbed =
                        collusion::remove_estimated_residual(&marked_copies[0], &mean, strength)
                            .map_err(|error| format!("collusion subtract: {error}"))?;
                    let attacked = observe(
                        &options
                            .victim
                            .detect(&scrubbed)
                            .map_err(|error| format!("detect collusion subtract: {error}"))?,
                    );
                    rows.push(finish(
                        Case {
                            item,
                            attack: format!("collude_subtract_residual_x{strength}"),
                            family: AttackFamily::Collusion,
                            knowledge: AttackerKnowledge::ManyMarkedCopiesOfOneRecording,
                            params: params(&[
                                ("copies", count.into()),
                                ("subtraction_strength", strength.into()),
                            ]),
                            expected: &expected_for_collusion,
                            baseline,
                            note: Some(
                                "One colluder's own copy, with the difference between it and the \
                                 colluded mean scaled and removed."
                                    .to_owned(),
                            ),
                        },
                        &attacked,
                        Some(&marked_copies[0]),
                        Some(&scrubbed),
                        None,
                    ));
                }
            }
        }
    }

    Ok(AttackReport {
        algorithm: genomark::ALGORITHM_ID.to_owned(),
        band_hz: options.geometry.band_hz(),
        lattice_step_nepers: genomark::params::DELTA,
        items: items.iter().map(|item| item.meta.id.clone()).collect(),
        rows,
        perceptual_limits: PERCEPTUAL_LIMITS,
        threat_model: THREAT_MODEL,
    })
}

/// How sharply the marked statistic's residues cluster on the lattice, against unmarked audio as
/// the control. This is the leak every transplant and forgery row rests on and it needs no key.
pub fn residue_leak(
    audio: &AudioBuffer,
    marked: &AudioBuffer,
    geometry: &PairBandGeometry,
    puncture: PunctureRule,
) -> Result<(ResidueEstimate, ResidueEstimate), String> {
    let unmarked = analyse(audio, geometry, puncture).map_err(|error| error.to_string())?;
    let marked = analyse(marked, geometry, puncture).map_err(|error| error.to_string())?;
    Ok((
        estimate_residues(&unmarked, geometry),
        estimate_residues(&marked, geometry),
    ))
}

/// Random cosets against the victim's key: the empirical arm of the false-accept bound.
pub fn forgery_null(
    carriers: &[CorpusItem],
    options: &Options<'_>,
) -> Result<(usize, usize, Vec<String>), String> {
    let mut accepts = Vec::new();
    let mut trials = 0usize;
    let period = options.geometry.slots_per_block();
    let mut state = options.seed | 1;
    for trial in 0..options.forgery_null_trials {
        let carrier = &carriers[trial % carriers.len()].audio;
        let residues: Vec<Option<f64>> = (0..period)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                Some(genomark::params::DELTA * ((state >> 11) as f64) / (1u64 << 53) as f64)
            })
            .collect();
        let transplant = CosetTransplant::new("random_coset", residues);
        let (forged, _) = drive(
            carrier,
            &options.geometry,
            options.puncture,
            &transplant,
            options.seed ^ trial as u64,
            DEFAULT_MAX_PASSES,
            CONVERGENCE_NEPERS,
        )
        .map_err(|error| format!("null forge: {error}"))?;
        let observed = observe(
            &options
                .victim
                .detect(&forged)
                .map_err(|error| format!("null detect: {error}"))?,
        );
        trials += 1;
        if let Some(payload) = observed.payload {
            accepts.push(hex(&payload));
        }
    }
    Ok((trials, accepts.len(), accepts))
}
