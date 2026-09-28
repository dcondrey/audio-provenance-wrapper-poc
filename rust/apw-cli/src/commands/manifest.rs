use apw_core::pretty_json_bytes;
use apw_daemon::{Daemon, DaemonConfig, DetectedExport, SourceCategory};
use apw_provenance::expand_user;

use crate::adapters::{EngineConfig, Engines};
use crate::cli::ManifestArgs;
use crate::error::{CliError, Result};

/// The one-shot sealer never listens for plug-in evidence, so it takes an
/// ephemeral port rather than competing with a running daemon for 9876.
const EPHEMERAL_PORT: u16 = 0;

pub fn run(args: &ManifestArgs) -> Result<i32> {
    let export = expand_user(&args.export);
    if !export.is_file() {
        return Err(CliError::io(
            "seal",
            &export,
            std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
        ));
    }
    let source_category = SourceCategory::parse(&args.source_category).ok_or_else(|| {
        CliError::usage(format!(
            "unsupported source category {:?}",
            args.source_category
        ))
    })?;

    let engines = Engines::load(&EngineConfig {
        provenance_store: &args.provenance_store,
        provenance_provider: args.provenance_provider.as_deref(),
        device_key_path: &args.signing_key,
        portable_private_key: &args.portable_private_key,
        portable_public_key: &args.portable_public_key,
    })?;

    let config = DaemonConfig {
        udp_port: EPHEMERAL_PORT,
        evidence_dir: expand_user(&args.evidence_dir),
        manifest_dir: expand_user(&args.manifest_dir),
        session_id: args.session_id.clone(),
        stem_id: args.stem_id.clone(),
        source_category,
        generate_html_report: !args.no_html_report,
        ..DaemonConfig::default()
    };
    let daemon = Daemon::new(config, engines.daemon_services(None))?;

    // IMPORTANT: no routed audio was observed in this process, so the manifest
    // records an empty hash chain and an unavailable export association. That is
    // the honest result of sealing a file after the fact, not a degraded one.
    log::warn!(
        "Sealing {} outside a capture session: no routed windows were observed, so the \
         manifest records no hash chain and no export association",
        export.display()
    );

    let generated = daemon.seal_export(&DetectedExport {
        resolved: std::fs::canonicalize(&export).unwrap_or_else(|_| export.clone()),
        path: export,
        version: 1,
    })?;

    println!("MANIFEST: {}", generated.manifest_path.display());
    println!("EXPORT:   {}", generated.export_hash);
    if args.stdout {
        let bytes = pretty_json_bytes(generated.manifest.as_value())?;
        println!("{}", String::from_utf8_lossy(&bytes));
    }
    Ok(0)
}
