//! `audio-provenance`: the command line over the Audio Provenance SDK.
//!
//! Every verdict printed here was decided by `apw_trace`. This binary chooses a renderer and maps a
//! status onto an exit code; it holds no provenance logic of its own, and there is no path by which
//! a rendering decision can change an answer.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod cmd;
mod context;
mod date;
mod error;
mod exit;
mod render;
mod riff;
mod style;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ColorChoice, Parser, Subcommand};

use crate::context::Context;
use crate::error::{CliError, describe};
use crate::style::Style;

#[derive(Debug, Parser)]
#[command(
    name = "audio-provenance",
    version,
    about = "Audio provenance: verify, sign, mark and register.",
    long_about = "Audio provenance.\n\nEXIT CODES\n  0 verified   1 changed   2 untrusted   3 not_found\n  4 incomplete (a recovery step could not run; not a verdict)\n  64 usage   65 unreadable input   70 internal\n\n`not_found` is neutral. A missing mark is never proof of synthetic origin: plenty of\nlegitimate recordings carry no mark, simply because they were never registered.",
    color = ColorChoice::Never
)]
struct Cli {
    /// Emit the SDK result verbatim as one JSON object.
    #[arg(long, global = true)]
    json: bool,
    /// Configuration file that names registries. Discovered when absent.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Registry to consult, by configured name.
    #[arg(long, global = true, value_name = "NAME")]
    registry: Option<String>,
    /// Trust anchors: a JSON store, or a directory containing one.
    #[arg(long, global = true, value_name = "PATH")]
    trust_store: Option<PathBuf>,
    /// Bench null-test report. Without one, no soft binding can reach `verified`.
    #[arg(long, global = true, value_name = "PATH")]
    null_test: Option<PathBuf>,
    /// Skip network rungs. They report `skipped`, never `unavailable`, so this alone never yields 4.
    #[arg(long, global = true)]
    offline: bool,
    /// Print nothing; communicate through the exit code alone.
    #[arg(long, short, global = true)]
    quiet: bool,
    #[arg(long, global = true)]
    no_color: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Decide the provenance of a file.
    Verify(cmd::verify::VerifyArgs),
    /// Hash and sign a file, optionally publishing the record.
    Sign(cmd::sign::SignArgs),
    /// Write a Watermark-marked copy.
    Embed(cmd::embed::EmbedArgs),
    /// Convert one admitted capture handoff into a development SDK record and verify it.
    CaptureAdapt(cmd::capture_adapt::CaptureAdaptArgs),
    /// Report what was found, with no verdict and no trust evaluation.
    Inspect(cmd::inspect::InspectArgs),
    /// Generate an Ed25519 signing key.
    Keygen(cmd::keygen::KeygenArgs),
    /// Manage a filesystem registry.
    Registry(cmd::registry::RegistryArgs),
    /// Create authorities, issue signer records, revoke, and inspect a trust store.
    Trust(cmd::trust::TrustArgs),
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // IMPORTANT: clap exits 2 by default, which is `untrusted` in this contract. A misspelled
        // flag must never be readable as a provenance verdict.
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(match error.kind() {
                clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayVersion
                | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    exit::VERIFIED
                }
                _ => exit::USAGE,
            });
        }
    };

    let context = Context {
        json: cli.json,
        quiet: cli.quiet,
        style: if cli.json {
            Style::plain()
        } else {
            Style::resolve(cli.no_color)
        },
        config: cli.config,
        registry: cli.registry,
        trust_store: cli.trust_store,
        null_test: cli.null_test,
        offline: cli.offline,
    };

    let outcome = match &cli.command {
        Command::Verify(args) => cmd::verify::run(&context, args),
        Command::Sign(args) => cmd::sign::run(&context, args),
        Command::Embed(args) => cmd::embed::run(&context, args),
        Command::CaptureAdapt(args) => cmd::capture_adapt::run(&context, args),
        Command::Inspect(args) => cmd::inspect::run(&context, args),
        Command::Keygen(args) => cmd::keygen::run(&context, args),
        Command::Registry(args) => cmd::registry::run(&context, args),
        Command::Trust(args) => cmd::trust::run(&context, args),
    };

    match outcome {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            if !context.quiet {
                eprintln!("{}", describe(&error));
            }
            ExitCode::from(CliError::exit(&error))
        }
    }
}
