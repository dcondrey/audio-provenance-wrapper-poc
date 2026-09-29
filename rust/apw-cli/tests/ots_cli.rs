//! `apw ots` and the daemon's OpenTimestamps flags, driven through the public
//! entry point. The proof and header are real (see tests/fixtures/parity/ots/SOURCES.json).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;

use clap::Parser;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/parity")
}

const HEADER_358391: &str = "02000000b96394585a281b7e5f438fd1c9ed492645a1fd61cb3802040000000000000000007ee445d23ad061af4a36b809501fab1ac4f2d7e7a739817dd0cbb7ec661b8a1e376755f58616186272def6";

fn run(args: &[String]) -> i32 {
    let mut argv = vec!["apw".to_owned()];
    argv.extend(args.iter().cloned());
    apw_cli::run(argv)
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[test]
fn a_real_bitcoin_attestation_verifies_against_the_callers_own_header() {
    let proof = fixtures().join("ots/hello-world.txt.ots").display().to_string();
    let file = fixtures().join("ots/hello-world.txt").display().to_string();
    let header = format!("358391:{HEADER_358391}");
    assert_eq!(run(&strings(&["ots", "verify", &proof, "--file", &file, "--header", &header])), 0);

    // A header for another height carries no such block: unavailable, not verified.
    let wrong_height = format!("1:{HEADER_358391}");
    assert_eq!(run(&strings(&["ots", "verify", &proof, "--file", &file, "--header", &wrong_height])), 1);

    // A file the proof does not cover is refused.
    let other = fixtures().join("ots/incomplete.txt").display().to_string();
    assert_eq!(run(&strings(&["ots", "verify", &proof, "--file", &other, "--header", &header])), 1);
}

#[test]
fn a_flipped_header_bit_is_not_verified() {
    let proof = fixtures().join("ots/hello-world.txt.ots").display().to_string();
    let digest = "03ba204e50d126e4674c005e04d82e84c21366780af1f43bd54a37816b6ab340";
    let mut tampered = HEADER_358391.to_owned();
    tampered.replace_range(80..82, "00");
    let header = format!("358391:{tampered}");
    assert_eq!(run(&strings(&["ots", "verify", &proof, "--digest", digest, "--header", &header])), 1);
}

#[test]
fn the_daemon_ots_flags_parse() {
    let base = ["apw", "daemon", "--port", "9876"];
    let cli = apw_cli::Cli::try_parse_from(base.iter().chain(&["--ots"])).unwrap();
    let apw_cli::Command::Daemon(args) = cli.command else { panic!("expected the daemon subcommand") };
    assert!(args.ots && args.ots_calendar.is_empty());
    let cli = apw_cli::Cli::try_parse_from(
        base.iter().chain(&["--ots-calendar", "https://a.example", "--ots-calendar", "https://b.example"]),
    )
    .unwrap();
    let apw_cli::Command::Daemon(args) = cli.command else { panic!("expected the daemon subcommand") };
    assert_eq!(args.ots_calendar, ["https://a.example", "https://b.example"]);
    // --explorer and --header are alternatives.
    assert!(apw_cli::Cli::try_parse_from(["apw", "ots", "verify", "p.ots", "--digest", "aa", "--explorer", "https://x", "--header", "1:aa"]).is_err());
}
