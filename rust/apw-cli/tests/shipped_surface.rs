//! The assembled binary, driven the way the demo drives it.
//!
//! These pin the wiring the CLI owns and nothing else: that the real engines
//! reach the seams `apw-daemon` declares, and that the honest defaults are still
//! honest once real engines are plugged in beside them.

use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Parser;
use serde_json::Value;

fn write_wav(path: &Path, frames: usize, channels: u16, bits: u16) -> Result<(), Box<dyn Error>> {
    let sample_rate: u32 = 44_100;
    let block_align = channels * bits / 8;
    let byte_rate = sample_rate * u32::from(block_align);
    let data_len = frames * usize::from(block_align);

    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(b"WAVE");
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&16u32.to_le_bytes());
    body.extend_from_slice(&if bits == 32 { 3u16 } else { 1u16 }.to_le_bytes());
    body.extend_from_slice(&channels.to_le_bytes());
    body.extend_from_slice(&sample_rate.to_le_bytes());
    body.extend_from_slice(&byte_rate.to_le_bytes());
    body.extend_from_slice(&block_align.to_le_bytes());
    body.extend_from_slice(&bits.to_le_bytes());
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(data_len as u32).to_le_bytes());
    for index in 0..frames {
        for _ in 0..channels {
            let phase = (index as f64) / 37.0;
            match bits {
                32 => body.extend_from_slice(&(phase.sin() as f32).to_le_bytes()),
                _ => body.extend_from_slice(&(((phase.sin()) * 12_000.0) as i16).to_le_bytes()),
            }
        }
    }

    let mut file = fs::File::create(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&(body.len() as u32).to_le_bytes())?;
    file.write_all(&body)?;
    Ok(())
}

struct Workspace {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Workspace {
    fn new() -> Result<Workspace, Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().to_path_buf();
        Ok(Workspace { _dir: dir, root })
    }

    fn manifest_argv(&self, export: &Path) -> Vec<String> {
        [
            "apw",
            "--log-level",
            "error",
            "manifest",
            &export.to_string_lossy(),
            "--manifest-dir",
            &self.root.join("manifests").to_string_lossy(),
            "--evidence-dir",
            &self.root.join("evidence").to_string_lossy(),
            "--provenance-store",
            &self.root.join("store").to_string_lossy(),
            "--signing-key",
            &self.root.join("keys/device.bin").to_string_lossy(),
            "--portable-private-key",
            &self.root.join("keys/ed25519.key").to_string_lossy(),
            "--portable-public-key",
            &self.root.join("keys/ed25519.pub").to_string_lossy(),
        ]
        .iter()
        .map(|value| value.to_string())
        .collect()
    }
}

/// The whole assembled path: the container probe, the C2PA claim issuer over the
/// local provenance store, and the device sealer, all reached through the shipped
/// `manifest` command.
///
/// The assertions that earn their place are the honesty ones. A signed claim that
/// reported `hardware_attested: true` from a filesystem key, or a `signer_identity`
/// that drifted off `not_established`, would still leave every other field looking
/// correct.
#[test]
fn manifest_seals_a_wav_with_a_signed_claim_and_an_unattested_local_seal(
) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new()?;
    let export = workspace.root.join("take.wav");
    write_wav(&export, 44_100, 2, 16)?;

    let code = apw_cli::run(workspace.manifest_argv(&export));
    assert_eq!(code, 0, "manifest command failed");

    let path = workspace.root.join("manifests/take_manifest.json");
    let manifest: Value = serde_json::from_slice(&fs::read(&path)?)?;

    let claim = manifest
        .get("c2pa_claim")
        .ok_or("the manifest carries no c2pa_claim")?;
    assert_eq!(claim.get("status").and_then(Value::as_str), Some("embedded"));
    assert_eq!(
        claim
            .pointer("/validation/state")
            .and_then(Value::as_str),
        Some("verified"),
        "claim did not verify against its own issuing root: {claim}"
    );
    assert_eq!(
        claim.pointer("/signer/signer_identity").and_then(Value::as_str),
        Some("not_established"),
        "a self-issued chain must never be reported as an established identity"
    );
    assert_eq!(
        claim.pointer("/hard_binding/source_sha256").and_then(Value::as_str),
        manifest.pointer("/export/sha256").and_then(Value::as_str),
        "the hard binding must commit to the export the manifest describes"
    );

    let signature = manifest
        .get("manifest_signature")
        .ok_or("the manifest carries no manifest_signature")?;
    assert_eq!(
        signature.get("hardware_attested").and_then(Value::as_bool),
        Some(false),
        "a filesystem key is not hardware attestation"
    );
    assert_eq!(
        signature.get("apw:proof_level").and_then(Value::as_str),
        Some("unknown_unobserved")
    );
    assert!(signature.pointer("/hardware_cosignature/entangled_hash").is_some());

    // REQUIRED: `sample_rate_hz` stays a JSON integer. 44100 and 44100.0 are
    // different canonical bytes, so a float here breaks signature parity with the
    // Python daemon.
    assert!(
        manifest
            .pointer("/export/sample_rate_hz")
            .and_then(Value::as_u64)
            .is_some(),
        "sample_rate_hz must serialise as an integer"
    );

    // No forgery screen is compiled in. An empty flag list from a screen that
    // never ran must not read as a clean result.
    assert_eq!(
        manifest.pointer("/forgery_analysis/status").and_then(Value::as_str),
        Some("not_analyzed")
    );
    Ok(())
}

/// 32-bit float breaks the routed/export feature association, so it must be
/// refused before anything is signed rather than producing a claim nobody can act
/// on. The refusal has to survive the claim issuer's own error handling, which
/// turns an engine failure into an `unavailable` record.
#[test]
fn a_float_wav_is_refused_and_names_the_supported_format() -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new()?;
    let export = workspace.root.join("float.wav");
    write_wav(&export, 4_410, 2, 32)?;

    let code = apw_cli::run(workspace.manifest_argv(&export));
    assert_eq!(code, 0, "the manifest itself must still be written");

    let path = workspace.root.join("manifests/float_manifest.json");
    let manifest: Value = serde_json::from_slice(&fs::read(&path)?)?;
    let claim = manifest
        .get("c2pa_claim")
        .ok_or("the manifest carries no c2pa_claim")?;

    assert_eq!(
        claim.get("status").and_then(Value::as_str),
        Some("unavailable")
    );
    let reason = claim
        .get("reason")
        .and_then(Value::as_str)
        .ok_or("the unavailable claim carries no reason")?;
    assert!(
        reason.contains("Re-export as 16-bit PCM WAV"),
        "the refusal must name the format that works: {reason}"
    );
    Ok(())
}

/// `scripts/run_demo.sh` builds this argument vector against
/// `daemon/__main__.py::parse_args`. Nothing else in the workspace can catch a
/// spelling drift, and `--time-anchor` is appended bare in one branch: clap would
/// silently read that as "no anchor" if `default_missing_value` were dropped.
#[test]
fn the_demo_script_daemon_argv_still_parses() -> Result<(), Box<dyn Error>> {
    let base: Vec<&str> = vec![
        "apw", "daemon",
        "--port", "9876",
        "--session-id", "capture-20260831T000000Z-1",
        "--stem-id", "stem-1",
        "--evidence-dir", "/tmp/e",
        "--sample-dir", "/tmp/s",
        "--export-dir", "/tmp/x",
        "--manifest-dir", "/tmp/m",
        "--source-category", "unknown",
        "--open-artifacts",
        "--project", "/tmp/set.als",
    ];

    let mut bare = base.clone();
    bare.push("--time-anchor");
    let apw_cli::Command::Daemon(args) = apw_cli::Cli::try_parse_from(&bare)?.command else {
        return Err("expected the daemon subcommand".into());
    };
    assert_eq!(
        args.time_anchor.as_deref(),
        Some("http://timestamp.digicert.com"),
        "a bare --time-anchor must select the default TSA, not disable anchoring"
    );
    assert_eq!(args.port, 9876);
    assert_eq!(args.project.as_deref(), Some(std::path::Path::new("/tmp/set.als")));
    assert!(args.open_artifacts);

    let mut explicit = base;
    explicit.extend(["--time-anchor", "https://tsa.example/ts"]);
    let apw_cli::Command::Daemon(args) = apw_cli::Cli::try_parse_from(&explicit)?.command else {
        return Err("expected the daemon subcommand".into());
    };
    assert_eq!(args.time_anchor.as_deref(), Some("https://tsa.example/ts"));
    Ok(())
}

/// A container that cannot carry an embedded manifest, verified without a
/// sidecar, must grade `nothing_found`. c2pa-rs answers "type is unsupported"
/// there, and surfacing that as an error leaves the four-state verifier with no
/// answer for a format this project supports by sidecar.
#[test]
fn an_aiff_without_a_sidecar_grades_nothing_found() -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new()?;
    let asset = workspace.root.join("take.aiff");
    write_aiff(&asset, 4_410, 2)?;

    let verified = apw_cli::state_of_file(&asset, None, "no_trust_anchors_supplied", None)?;
    assert_eq!(
        verified.record.get("state").and_then(Value::as_str),
        Some("nothing_found")
    );
    assert!(
        verified
            .record
            .get("detail")
            .and_then(Value::as_str)
            .is_some_and(|detail| detail.contains("--sidecar")),
        "the grade must say how to supply the detached manifest: {}",
        verified.record
    );
    Ok(())
}

fn write_aiff(path: &Path, frames: usize, channels: u16) -> Result<(), Box<dyn Error>> {
    // 44100 Hz as an IEEE 754 80-bit extended float, the only encoding COMM accepts.
    const SAMPLE_RATE_EXTENDED: [u8; 10] = [0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0];

    let mut comm: Vec<u8> = Vec::new();
    comm.extend_from_slice(&channels.to_be_bytes());
    comm.extend_from_slice(&(frames as u32).to_be_bytes());
    comm.extend_from_slice(&16u16.to_be_bytes());
    comm.extend_from_slice(&SAMPLE_RATE_EXTENDED);

    let mut ssnd: Vec<u8> = vec![0; 8];
    for index in 0..frames {
        let sample = ((index as f64 / 29.0).sin() * 9_000.0) as i16;
        for _ in 0..channels {
            ssnd.extend_from_slice(&sample.to_be_bytes());
        }
    }

    let mut body: Vec<u8> = Vec::from(*b"AIFF");
    body.extend_from_slice(b"COMM");
    body.extend_from_slice(&(comm.len() as u32).to_be_bytes());
    body.extend_from_slice(&comm);
    body.extend_from_slice(b"SSND");
    body.extend_from_slice(&(ssnd.len() as u32).to_be_bytes());
    body.extend_from_slice(&ssnd);

    let mut file = fs::File::create(path)?;
    file.write_all(b"FORM")?;
    file.write_all(&(body.len() as u32).to_be_bytes())?;
    file.write_all(&body)?;
    Ok(())
}
