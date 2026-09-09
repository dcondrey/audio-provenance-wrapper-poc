use std::path::PathBuf;

use clap::Args;
use apw_watermark::Watermark;
use apw_trace::{SidecarPolicy, VerifyOptions};

use crate::context::Context;
use crate::error::CliError;
use crate::{exit, render};

#[derive(Debug, Args)]
pub struct InspectArgs {
    pub file: PathBuf,
    #[arg(long, conflicts_with = "no_sidecar")]
    pub sidecar: Option<PathBuf>,
    #[arg(long)]
    pub no_sidecar: bool,
}

/// Exit 0 whenever the file was readable, regardless of what was found. `inspect` reaches no
/// verdict, so it has no verdict to map onto an exit code.
pub fn run(context: &Context, args: &InspectArgs) -> Result<u8, CliError> {
    let registry = context.registry()?;
    let apw_watermark = Watermark::public();
    let mut options = VerifyOptions::new()
        .with_apw_watermark(&apw_watermark)
        .offline(context.offline)
        .with_sidecar(match (&args.sidecar, args.no_sidecar) {
            (_, true) => SidecarPolicy::Disabled,
            (Some(path), false) => SidecarPolicy::Explicit(path.clone()),
            (None, false) => SidecarPolicy::Conventional,
        });
    if let Some(backend) = registry.as_deref() {
        options = options.with_registry(backend);
    }

    let report = apw_trace::inspect(&args.file, &options)?;
    if context.json {
        let rendered =
            serde_json::to_string_pretty(&report).map_err(|source| CliError::Serialise {
                what: "inspect report",
                source,
            })?;
        println!("{rendered}");
    } else if !context.quiet {
        print!(
            "{}",
            render::inspect_report(&report, &args.file.display().to_string(), context.style)
        );
    }
    Ok(exit::VERIFIED)
}
