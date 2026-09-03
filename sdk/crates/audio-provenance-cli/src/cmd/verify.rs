use std::path::PathBuf;

use clap::Args;
use genomark::GenoMark;
use genotrace::{
    FileFingerprintIndex, InferredAssociationPolicy, NoTrustAnchors, NullTestTable, SidecarPolicy,
    TrustStore, VerifyOptions,
};

use crate::context::Context;
use crate::error::CliError;
use crate::{exit, render};

#[derive(Debug, Args)]
pub struct VerifyArgs {
    pub file: PathBuf,
    /// Read the sidecar manifest from this exact path instead of the conventional one.
    #[arg(long, conflicts_with = "no_sidecar")]
    pub sidecar: Option<PathBuf>,
    #[arg(long)]
    pub no_sidecar: bool,
    /// Soft-binding acceptance threshold in [0, 1].
    #[arg(long)]
    pub threshold: Option<f64>,
    /// Let the fingerprint rung emit a candidate. It can never produce `verified`.
    #[arg(long)]
    pub accept_inferred: bool,
    /// Landmark index the fingerprint rung searches.
    #[arg(long)]
    pub fingerprint_index: Option<PathBuf>,
    /// Print every rung that ran, including the ones that missed.
    #[arg(short, long)]
    pub verbose: bool,
}

pub fn run(context: &Context, args: &VerifyArgs) -> Result<u8, CliError> {
    let registry = context.registry()?;
    let trust = context.trust_store()?;
    let null_test = context.null_test()?;
    let fingerprint_index = match &args.fingerprint_index {
        Some(path) => Some(FileFingerprintIndex::open(path)?),
        None => None,
    };
    let genomark = GenoMark::public();
    let no_anchors = NoTrustAnchors;

    let mut options = VerifyOptions::new()
        .with_genomark(&genomark)
        .offline(context.offline)
        .with_sidecar(match (&args.sidecar, args.no_sidecar) {
            (_, true) => SidecarPolicy::Disabled,
            (Some(path), false) => SidecarPolicy::Explicit(path.clone()),
            (None, false) => SidecarPolicy::Conventional,
        })
        .with_inferred_association(if args.accept_inferred {
            InferredAssociationPolicy::EmitCandidate
        } else {
            InferredAssociationPolicy::DiagnosticsOnly
        });
    if let Some(threshold) = args.threshold {
        options = options.with_soft_binding_threshold(threshold)?;
    }
    if let Some(backend) = registry.as_deref() {
        options = options.with_registry(backend);
    }
    options = match trust.as_ref() {
        Some((store, _)) => options.with_trust_store(store.as_ref()),
        // Explicit, not a default: a verifier with no configured anchors knows no identities.
        None => options.with_trust_store(&no_anchors as &dyn TrustStore),
    };
    let table: NullTestTable;
    if let Some(loaded) = null_test {
        table = loaded;
        options = options.with_null_test(&table);
    }
    if let Some(index) = fingerprint_index.as_ref() {
        options = options.with_fingerprint_index(index);
    }

    let result = genotrace::verify(&args.file, &options)?;
    let name = args.file.display().to_string();

    if context.json {
        let rendered =
            serde_json::to_string_pretty(&result).map_err(|source| CliError::Serialise {
                what: "verify result",
                source,
            })?;
        println!("{rendered}");
    } else if !context.quiet {
        print!(
            "{}",
            render::verify_result(&result, &name, context.style, args.verbose)
        );
    }
    Ok(exit::for_verdict(result.status, result.incomplete))
}
