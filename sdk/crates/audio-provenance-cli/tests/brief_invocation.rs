//! The brief's invocation, driven through the real binary.
//!
//! `audio-provenance verify filename.wav --registry=public` must report verified, identity
//! "Signal Room Studios", signed 2026-03-13, match 1.00, and exit 0. Nothing short of running the
//! shipped binary over a real file proves that, so this test does exactly that.

#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;

use audio_provenance_audio::{AudioBuffer, BitDepth, DecodeLimits, wav};

const BIN: &str = env!("CARGO_BIN_EXE_audio-provenance");
const SIGNED_AT: &str = "2026-03-13";
const IDENTITY: &str = "Signal Room Studios";
const AUTHORITY: &str = "studio-ca";

fn audio_provenance(root: &Path, args: &[&str]) -> (String, String, i32) {
    let output = Command::new(BIN)
        .args(args)
        .current_dir(root)
        // The name `public` resolves through config, never through a built-in endpoint, so the
        // config file is what makes the brief's flag work at all.
        .env(
            "AUDIO_PROVENANCE_CONFIG",
            root.join("audio-provenance.config.json"),
        )
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

fn write_tone(path: &Path) {
    let sample_rate = 44_100;
    let frames = sample_rate as usize * 3;
    let samples: Vec<f32> = (0..frames)
        .map(|n| {
            let t = n as f32 / sample_rate as f32;
            (t * 440.0 * std::f32::consts::TAU).sin() * 0.25
        })
        .collect();
    let buffer = AudioBuffer::from_channels(sample_rate, &[samples.clone(), samples]).unwrap();
    std::fs::write(path, wav::encode(&buffer, BitDepth::Int16).unwrap()).unwrap();
}

/// Broadband content, because a mark needs energy across 861-4307 Hz to carry; a pure tone
/// punctures nearly every slot.
fn write_noise(path: &Path, seconds: f64) {
    let sample_rate = 44_100;
    let frames = (f64::from(sample_rate) * seconds) as usize;
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let samples: Vec<f32> = (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 40) as f32 / 8_388_608.0 - 1.0) * 0.25
        })
        .collect();
    let buffer = AudioBuffer::from_channels(sample_rate, &[samples]).unwrap();
    std::fs::write(path, wav::encode(&buffer, BitDepth::Int24).unwrap()).unwrap();
}

#[test]
fn the_briefs_invocation_verifies_and_exits_zero() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write_tone(&root.join("filename.wav"));

    let (out, err, code) = audio_provenance(root, &["keygen", "--out", "studio.key"]);
    assert_eq!(code, 0, "keygen: {err}");
    let signer_id = out
        .lines()
        .find_map(|line| line.trim().strip_prefix("signer_id  "))
        .unwrap()
        .trim()
        .to_string();

    let (_, err, code) =
        audio_provenance(root, &["registry", "init", "registry", "--name", "public"]);
    assert_eq!(code, 0, "registry init: {err}");
    std::fs::write(
        root.join("audio-provenance.config.json"),
        serde_json::to_vec(&serde_json::json!({
            "config_format": "audio-provenance-config-v0",
            "registries": { "public": { "kind": "local", "root": root.join("registry") } },
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        root.join("trust-store.json"),
        serde_json::to_vec(&serde_json::json!({
            "format": "audio-provenance-trust-store-v0",
            "anchors": [{
                "signer_id": signer_id,
                "identity": IDENTITY,
                "authority": AUTHORITY,
            }],
        }))
        .unwrap(),
    )
    .unwrap();

    let (_, err, code) = audio_provenance(
        root,
        &[
            "sign",
            "filename.wav",
            "--key",
            "studio.key",
            "--signed-at",
            SIGNED_AT,
        ],
    );
    assert_eq!(code, 0, "sign: {err}");
    let (_, err, code) = audio_provenance(
        root,
        &[
            "--registry=public",
            "registry",
            "add",
            "filename.wav.audio-provenance.json",
        ],
    );
    assert_eq!(code, 0, "registry add: {err}");

    let (out, err, code) = audio_provenance(
        root,
        &[
            "--trust-store",
            "trust-store.json",
            "verify",
            "filename.wav",
            "--registry=public",
        ],
    );
    assert_eq!(code, 0, "verify: {err}\n{out}");
    assert!(out.starts_with("verified   filename.wav\n"), "{out}");
    assert!(
        out.contains(&format!(
            "identity   {IDENTITY}        (externally_verified via {AUTHORITY})"
        )),
        "{out}"
    );
    assert!(out.contains(&format!("signed     {SIGNED_AT}")), "{out}");
    assert!(out.contains("match      1.00"), "{out}");
    assert!(out.contains("registry   public (filesystem)"), "{out}");

    // The same file with no anchors resolves no identity. A cryptographically perfect signature
    // proving key possession must never render a name.
    let (out, _, code) = audio_provenance(root, &["verify", "filename.wav", "--registry=public"]);
    assert_eq!(code, 2, "{out}");
    assert!(out.starts_with("untrusted  "), "{out}");
    assert!(out.contains("identity   not_established"), "{out}");
    assert!(!out.contains(IDENTITY), "{out}");
}

/// `sign --mark` claims the enforced order allocate, mark, hash, sign. The claim is only true if
/// the mark reported at signing is the mark a blind detector finds in the file that was written,
/// if the hard binding still covers those exact bytes, and if the mark RESOLVES the record that was
/// signed over it once the audio has moved past every exact-digest rung.
#[test]
fn a_marked_signature_binds_the_marked_audio_and_the_mark_survives_into_the_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // A Watermark block is 9.66 s at 44.1 kHz and an arbitrary crop needs two of them, so 21 s is the
    // shortest input that puts recovery inside the guaranteed window rather than at its edge.
    write_noise(&root.join("master.wav"), 21.0);

    let (_, err, code) = audio_provenance(root, &["keygen", "--out", "studio.key"]);
    assert_eq!(code, 0, "keygen: {err}");
    // The Watermark rung reports `skipped` with no registry to resolve a mark against, so one has to
    // exist for the recovered mark id to reach the trace at all.
    let (_, err, code) =
        audio_provenance(root, &["registry", "init", "registry", "--name", "public"]);
    assert_eq!(code, 0, "registry init: {err}");
    std::fs::write(
        root.join("audio-provenance.config.json"),
        serde_json::to_vec(&serde_json::json!({
            "config_format": "audio-provenance-config-v0",
            "registries": { "public": { "kind": "local", "root": root.join("registry") } },
        }))
        .unwrap(),
    )
    .unwrap();

    let (out, err, code) = audio_provenance(
        root,
        &[
            "--registry=public",
            "sign",
            "master.wav",
            "--key",
            "studio.key",
            "--signed-at",
            SIGNED_AT,
            "--mark",
            "--out",
            "master.marked.wav",
        ],
    );
    assert_eq!(code, 0, "sign --mark: {err}");
    assert!(out.contains("resolves   record "), "{out}");
    assert!(out.contains("reference  apw-trace-landmark-v1"), "{out}");
    let signer_id = out
        .lines()
        .find_map(|line| line.trim().strip_prefix("signer_id  "))
        .unwrap()
        .trim()
        .to_string();
    let locator = out
        .lines()
        .find_map(|line| line.trim().strip_prefix("mark       "))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();

    // The hard binding covers the marked bytes, not the master's.
    let (out, err, code) = audio_provenance(root, &["verify", "master.marked.wav"]);
    assert_eq!(code, 2, "verify: {err}{out}");
    assert!(out.contains("match      1.00"), "{out}");
    assert!(out.contains("(hard binding, exact)"), "{out}");

    // THE FLAGSHIP PATH. Requantising to 16-bit moves both the file bytes and the decoded samples,
    // so every exact-digest rung misses and only the mark can recover the record. Nothing was
    // tampered with, so `changed` would be a false statement; the mark plus the signed reference
    // constellation carry the verdict instead, and how far they carry it is decided by whether the
    // verifier was given a measured false-positive rate and a trust anchor.
    let marked = wav::decode(
        &std::fs::read(root.join("master.marked.wav")).unwrap(),
        &DecodeLimits::default(),
    )
    .unwrap();
    std::fs::write(
        root.join("requantised.wav"),
        wav::encode(&marked, BitDepth::Int16).unwrap(),
    )
    .unwrap();

    // Unpriced: the soft binding affirmed, so the verdict is `untrusted` with the remedy named. It
    // is deliberately NOT `changed`, which would restate the false accusation this path removes.
    let (out, err, code) = audio_provenance(
        root,
        &["--registry=public", "verify", "requantised.wav", "-v"],
    );
    assert_eq!(code, 2, "{err}{out}");
    assert!(out.starts_with("untrusted  "), "{out}");
    assert!(out.contains("recovered via apw_watermark_recovery"), "{out}");
    assert!(
        out.contains("the signed reference constellation agree"),
        "{out}"
    );
    let apw_watermark = out
        .lines()
        .find(|line| line.contains("apw_watermark_recovery") && !line.contains("recovered via"))
        .unwrap();
    assert!(apw_watermark.contains("hit"), "{apw_watermark}");

    // Priced and anchored: all four predicates hold by route 3(c). The status is `verified`, and
    // every honesty guard fires with it — the qualifier on the header, a match strictly under 1.00,
    // and proof level `inferred` in the binding row.
    std::fs::write(
        root.join("trust-store.json"),
        serde_json::to_vec(&serde_json::json!({
            "format": "audio-provenance-trust-store-v0",
            "anchors": [{
                "signer_id": signer_id,
                "identity": IDENTITY,
                "authority": AUTHORITY,
            }],
        }))
        .unwrap(),
    )
    .unwrap();
    let null_test = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("bench-out/final2/apw-watermark-lepqim-v1.json");
    let (out, err, code) = audio_provenance(
        root,
        &[
            "--registry=public",
            "--trust-store",
            "trust-store.json",
            "--null-test",
            &null_test.display().to_string(),
            "verify",
            "requantised.wav",
        ],
    );
    assert_eq!(code, 0, "{err}{out}");
    assert!(out.starts_with("verified   "), "{out}");
    assert!(out.contains("(inferred)"), "{out}");
    assert!(out.contains(IDENTITY), "{out}");
    assert!(
        !out.contains("match      1.00"),
        "an inferred verification must never report an exact match: {out}"
    );
    assert!(out.contains("inferred)"), "{out}");

    // THE SPLICE, which is what stops route 3(c) being a hole. Ten marked seconds in front of
    // eleven unmarked ones is genuinely part of the signed work, so the reference constellation
    // aligns across the marked half and its coverage floor is cleared. The mark covering one whole
    // block in two is the fact that says the rest of the file is not this recording, and it is the
    // only term that catches this. `changed` here is the true statement.
    let head: Vec<f32> = marked
        .channel(0)
        .unwrap()
        .iter()
        .copied()
        .take(44_100 * 10)
        .collect();
    // A tone, not another noise burst. `write_noise` is seeded once, so a second burst would be
    // the master's own noise again and the reference would align at TWO offsets, which the
    // dominance gate rejects before the block guard is ever consulted. This fixture has to reach
    // the guard it is pinning.
    let foreign: Vec<f32> = (0..44_100 * 11)
        .map(|n| {
            let t = n as f32 / 44_100.0;
            (t * 523.0 * std::f32::consts::TAU).sin() * 0.25
        })
        .collect();
    let mut spliced = head;
    spliced.extend_from_slice(&foreign);
    let spliced = AudioBuffer::from_channels(44_100, &[spliced.clone(), spliced]).unwrap();
    std::fs::write(
        root.join("spliced.wav"),
        wav::encode(&spliced, BitDepth::Int16).unwrap(),
    )
    .unwrap();

    let (out, err, code) = audio_provenance(
        root,
        &[
            "--registry=public",
            "--trust-store",
            "trust-store.json",
            "--null-test",
            &null_test.display().to_string(),
            "verify",
            "spliced.wav",
            "-v",
        ],
    );
    assert_eq!(code, 1, "a splice must be changed: {err}{out}");
    assert!(out.starts_with("changed    "), "{out}");
    assert!(
        out.contains("reference_constellation_disagrees")
            || out.contains("recovered_mark_covers_too_little"),
        "either independent local-association gate may reject first: {out}"
    );

    // The locator the signer reported IS the key the registry stores the record under. A mark id is
    // the version and namespace nibbles followed by the six locator bytes.
    let (out, err, code) =
        audio_provenance(root, &["--registry=public", "--json", "registry", "list"]);
    assert_eq!(code, 0, "registry list: {err}");
    let listing: serde_json::Value = serde_json::from_str(&out).unwrap();
    let marks: Vec<&str> = listing["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["mark_id"].as_str())
        .collect();
    assert_eq!(marks, vec![format!("01{locator}").as_str()], "{out}");

    // The mark refuses to run over audio a manifest already covers.
    let (_, err, code) = audio_provenance(
        root,
        &[
            "sign",
            "master.marked.wav",
            "--key",
            "studio.key",
            "--mark",
            "--out",
            "again.wav",
        ],
    );
    assert_eq!(code, 64);
    assert!(err.contains("mark_after_sign"), "{err}");
}

/// clap exits 2 on a usage error by default, and 2 is `untrusted` here. A typo must never be
/// readable as a provenance verdict.
#[test]
fn a_usage_error_exits_64_and_never_2() {
    let temp = tempfile::tempdir().unwrap();
    let (_, _, code) = audio_provenance(temp.path(), &["verify", "--not-a-flag", "x.wav"]);
    assert_eq!(code, 64);
    let (_, _, code) = audio_provenance(temp.path(), &["--help"]);
    assert_eq!(code, 0);
}

/// `not_found` is a neutral result, and the exit code says so separately from a registry outage.
#[test]
fn an_unregistered_file_is_not_found_and_reads_neutrally() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write_tone(&root.join("orphan.wav"));
    let (out, err, code) = audio_provenance(root, &["verify", "orphan.wav"]);
    assert_eq!(code, 3, "{err}{out}");
    assert!(out.starts_with("not_found  orphan.wav\n"), "{out}");
    let flowed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flowed.contains("A missing mark is never proof of synthetic origin"),
        "{out}"
    );
    assert!(out.contains("identity   not_established"), "{out}");
}
