#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use serde_json::Value;

#[test]
fn the_permanent_adversarial_plan_cannot_silently_drop_a_required_case_or_metric() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("qualification/watermark-adversarial-v1.json");
    let document: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(document["production_implementation"], "apw-watermark-lepqim-v1");
    assert_eq!(
        document["policy"]["locator_recovery_is_authentication"],
        false
    );

    let required_metrics = BTreeSet::from([
        "false_acceptance",
        "locator_recovery",
        "recording_association_rejection",
    ]);
    let declared_metrics: BTreeSet<&str> = document["measurements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert_eq!(declared_metrics, required_metrics);

    let required_tags = BTreeSet::from([
        "interior_replacement",
        "head_replacement",
        "tail_replacement",
        "crop",
        "arbitrary_crop_offset",
        "reorder",
        "duplicated_section",
        "mark_lifting",
        "overlay",
        "gain",
        "normalization",
        "dynamic_compression",
        "mp3",
        "aac",
        "requantization",
        "resampling",
        "playback_rate_drift",
        "silence",
        "near_silence",
        "speech",
        "tonal",
        "dense_music",
        "multiple_encoding_passes",
        "collusion",
        "very_short",
        "multi_hour",
        "unmarked_audio",
        "unrelated_audio",
    ]);
    let cases = document["cases"].as_array().unwrap();
    let tags: BTreeSet<&str> = cases
        .iter()
        .flat_map(|case| case["tags"].as_array().unwrap())
        .map(|tag| tag.as_str().unwrap())
        .collect();
    assert_eq!(tags, required_tags);

    let fractions: Vec<u64> = cases
        .iter()
        .find(|case| case["id"] == "interior_replacement_1_through_50_percent")
        .unwrap()["parameters"]["percent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_u64().unwrap())
        .collect();
    assert_eq!(fractions, [1, 2, 5, 10, 25, 50]);

    for case in cases {
        assert!(!case["measures"].as_array().unwrap().is_empty());
        assert!(!case["gate"].as_str().unwrap().is_empty());
        for metric in case["measures"].as_array().unwrap() {
            assert!(required_metrics.contains(metric.as_str().unwrap()));
        }
    }
}

#[test]
fn the_production_qualification_runner_cannot_link_the_experimental_implementation() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("audio-provenance-nulltest/Cargo.toml");
    let text = std::fs::read_to_string(manifest).unwrap();
    assert!(text.contains("classical-apw-watermark"));
    assert!(!text.contains("apw-watermark-neural"));
}
