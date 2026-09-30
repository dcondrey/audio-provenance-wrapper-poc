//! The shipped engine: the daemon, the C2PA signer, the four-state verifier and
//! the manifest sealer, plus the adapters that connect the observation crates to
//! the seams `apw-daemon` declares.
//!
//! IMPORTANT: nothing here establishes identity. A locally issued chain proves
//! possession of a key on this machine; `verified` means a claim chains to an
//! anchor the caller supplied, never that a creator, owner or rights holder was
//! confirmed. `nothing_found` records the absence of provenance data and is
//! never evidence of synthetic origin.

mod adapters;
mod cli;
mod commands;
mod error;
mod manifest_verify;

use clap::Parser;

pub use adapters::{
    C2paClaimIssuer, EngineConfig, Engines, FeatureAssociator, PcmAudioProbe, ProviderSealer,
};
pub use cli::{Cli, Command};
pub use error::{one_line, CliError, Result};
pub use manifest_verify::{verify_manifest, ManifestVerifyOptions};
pub use verify_state::{state_of_file, VerifiedFile};

mod verify_state;

/// Parse `argv` and run one command, returning the process exit code.
pub fn run<I, T>(argv: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        // clap renders help and version through the same error channel; its own
        // exit code is the correct one for both.
        Err(error) => error.exit(),
    };
    init_logging(&cli.log_level);

    let outcome = match &cli.command {
        Command::Daemon(args) => commands::daemon::run(args),
        Command::Sign(args) => commands::sign::run(args),
        Command::Verify(args) => commands::verify::run(args),
        Command::Manifest(args) => commands::manifest::run(args),
        Command::Ots(args) => commands::ots::run(args),
    };

    match outcome {
        Ok(code) => code,
        Err(error) => {
            log::error!("{}", one_line(&error));
            1
        }
    }
}

fn init_logging(level: &str) {
    let mut builder = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(level),
    );
    builder.format_timestamp_secs();
    let _ = builder.try_init();
}
