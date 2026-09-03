use std::path::PathBuf;

use audio_provenance_core::signing::fs::load_signing_key;
use audio_provenance_sdk::{
    CaptureAdapterOptions, SidecarOutput, adapt_capture_export, write_capture_adapter_receipt,
};
use clap::Args;

use crate::context::Context;
use crate::error::CliError;
use crate::exit;

#[derive(Debug, Args)]
pub struct CaptureAdaptArgs {
    /// Completed audio export whose hard hash the capture handoff declares.
    pub export: PathBuf,
    /// Signed audio-provenance-manifest-v0 capture manifest.
    #[arg(long)]
    pub capture_manifest: PathBuf,
    /// Downstream handoff copied from that signed capture manifest.
    #[arg(long)]
    pub handoff: PathBuf,
    /// Evidence bundle to hash into the development SDK record.
    #[arg(long)]
    pub evidence_bundle: PathBuf,
    /// 32 raw Ed25519 development-key bytes.
    #[arg(long)]
    pub key: PathBuf,
    /// Require this precomputed evidence-bundle SHA-256 in addition to recomputing it.
    #[arg(long)]
    pub evidence_bundle_sha256: Option<String>,
    /// Write an export copy with one `aprv` record slot. Conflicts with --sidecar.
    #[arg(long, conflicts_with = "sidecar")]
    pub out: Option<PathBuf>,
    /// Write the record to this sidecar. The conventional sidecar is used when neither output flag
    /// is present.
    #[arg(long)]
    pub sidecar: Option<PathBuf>,
    /// Atomic JSON receipt path. Defaults beside the handoff.
    #[arg(long)]
    pub receipt: Option<PathBuf>,
}

pub fn run(context: &Context, args: &CaptureAdaptArgs) -> Result<u8, CliError> {
    let key = load_signing_key(&args.key)?;
    let public_key_file = args
        .key
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ephemeral-development.key".to_string());
    let mut options = CaptureAdapterOptions::new(key).public_key_file(public_key_file);
    options = match (&args.out, &args.sidecar) {
        (Some(output), None) => options.embedded_output(output.clone()),
        (None, Some(sidecar)) => options.sidecar(SidecarOutput::Explicit(sidecar.clone())),
        (None, None) => options.sidecar(SidecarOutput::Conventional),
        (Some(_), Some(_)) => {
            return Err(CliError::usage(
                "--out and --sidecar are mutually exclusive",
            ));
        }
    };
    if let Some(expected) = &args.evidence_bundle_sha256 {
        options = options.expected_evidence_bundle_sha256(expected.clone())?;
    }
    if let Some(name) = &context.registry {
        options = options.registry(context.writable_registry(name)?);
    }

    let result = adapt_capture_export(
        &args.export,
        &args.capture_manifest,
        &args.handoff,
        &args.evidence_bundle,
        options,
    )?;
    let receipt = args.receipt.clone().unwrap_or_else(|| {
        let mut name = args
            .handoff
            .file_stem()
            .map(|stem| stem.to_os_string())
            .unwrap_or_else(|| "capture-handoff".into());
        name.push(".sdk-receipt.json");
        args.handoff.with_file_name(name)
    });
    write_capture_adapter_receipt(&receipt, &result)?;

    if context.json {
        let rendered =
            serde_json::to_string_pretty(&result).map_err(|source| CliError::Serialise {
                what: "capture adapter result",
                source,
            })?;
        println!("{rendered}");
    } else if !context.quiet {
        println!("adapted    {}", args.export.display());
        println!("  record     {}", result.sign.record_id);
        println!(
            "  verification {} ({})",
            result.verification.status.as_str(),
            result.verification.reason
        );
        println!("  identity   not_established");
        println!("  mode       development_only");
        println!("  evidence   {}", result.evidence_bundle_sha256);
        println!("  receipt    {}", receipt.display());
    }
    Ok(exit::VERIFIED)
}
