use std::path::{Path, PathBuf};

use audio_provenance_registry::{
    ContentHash, LOCAL_REGISTRY, LocalRegistryBackend, Lookup, RecordId, RegistryBackend,
    RegistryRecord,
};
use clap::{Args, Subcommand};
use apw_trace::admission::Admission;

use crate::context::Context;
use crate::error::CliError;
use crate::exit;

#[derive(Debug, Args)]
pub struct RegistryArgs {
    #[command(subcommand)]
    pub command: RegistryCommand,
}

#[derive(Debug, Subcommand)]
pub enum RegistryCommand {
    /// Create the directory layout and an empty index.
    Init {
        root: PathBuf,
        /// The name this registry answers to in reports. Also the name to configure it under.
        #[arg(long, default_value = LOCAL_REGISTRY)]
        name: String,
    },
    /// Store a signed manifest. Refuses anything whose canonical bytes do not round-trip.
    Add { manifest: PathBuf },
    /// Every record id, with its content hash and signing date.
    List,
    /// Print one record's manifest.
    Get { record_id: String },
}

pub fn run(context: &Context, args: &RegistryArgs) -> Result<u8, CliError> {
    match &args.command {
        RegistryCommand::Init { root, name } => init(context, root, name),
        RegistryCommand::Add { manifest } => add(context, manifest),
        RegistryCommand::List => list(context),
        RegistryCommand::Get { record_id } => get(context, record_id),
    }
}

fn init(context: &Context, root: &Path, name: &str) -> Result<u8, CliError> {
    let backend = LocalRegistryBackend::init(name, root)?;
    if context.json {
        emit(
            context,
            &serde_json::json!({
                "name": backend.source().name(),
                "kind": backend.source().kind().as_str(),
                "root": backend.root().display().to_string(),
            }),
            "registry init report",
        )?;
    } else if !context.quiet {
        println!("initialised {}", backend.root().display());
        println!("  name       {}", backend.source().name());
        println!(
            "  configure  AUDIO_PROVENANCE_REGISTRY_ROOT={} audio-provenance verify <file> --registry={}",
            backend.root().display(),
            backend.source().name()
        );
    }
    Ok(exit::VERIFIED)
}

fn add(context: &Context, manifest_path: &Path) -> Result<u8, CliError> {
    let name = registry_name(context)?;
    let backend = context.writable_registry(&name)?;
    let bytes =
        std::fs::read(manifest_path).map_err(CliError::io("read", manifest_path.display()))?;

    // The signature is checked by admitting the manifest before it is stored. A registry of records
    // that never verified is a registry of assertions.
    let manifest = match apw_trace::admission::admit(&bytes) {
        Admission::Admitted { manifest } => manifest,
        Admission::Rejected { code, detail, .. } | Admission::Unparseable { code, detail } => {
            return Err(CliError::usage(format!("{code}: {detail}")));
        }
    };
    let hard_binding = manifest.hard_binding().ok_or_else(|| {
        CliError::usage("manifest declares no hard binding, so it indexes no content hash")
    })?;
    let signed_at = manifest
        .signed_at()
        .ok_or_else(|| CliError::usage("manifest declares no signed_at"))?;

    // The mark version, namespace and locator all come from the manifest itself, so a stored
    // record can never name a mark its own document contradicts.
    let record = RegistryRecord::from_signed_manifest(
        manifest.value().clone(),
        ContentHash::parse_hex(hard_binding.content_sha256())?,
        None,
        crate::date::registry_signed_at(signed_at)?,
    )?;
    backend.put(&record)?;

    if context.json {
        emit(
            context,
            &serde_json::json!({
                "registry": backend.source().name(),
                "record_id": record.record_id().to_hex(),
                "mark_id": record.mark_id().to_hex(),
                "content_sha256": record.content_hash().to_hex(),
                "signed_at": record.signed_at().as_str(),
            }),
            "registry add report",
        )?;
    } else if !context.quiet {
        println!("added      {}", record.record_id().to_hex());
        println!("  registry   {}", backend.source().name());
        println!("  mark       {}", record.mark_id().to_hex());
        println!("  content    {}", record.content_hash().to_hex());
        println!("  signed     {}", record.signed_at().as_str());
    }
    Ok(exit::VERIFIED)
}

fn list(context: &Context) -> Result<u8, CliError> {
    let name = registry_name(context)?;
    let backend = context.writable_registry(&name)?;
    let ids = backend.record_ids()?;
    let mut rows = Vec::with_capacity(ids.len());
    let mut unavailable = Vec::new();
    for id in &ids {
        match backend.fetch(id) {
            Lookup::Found(record) => rows.push(serde_json::json!({
                "record_id": record.record_id().to_hex(),
                "mark_id": record.mark_id().to_hex(),
                "content_sha256": record.content_hash().to_hex(),
                "signed_at": record.signed_at().as_str(),
                "signer_id": record.signature().signer_id.clone(),
            })),
            // An index entry whose document will not load is reported as itself. Dropping the row
            // would make a damaged registry look merely smaller.
            Lookup::NotFound => unavailable.push((id.to_hex(), "document missing".to_string())),
            Lookup::Unavailable(reason) => unavailable.push((
                id.to_hex(),
                format!("{}: {}", reason.kind().as_str(), reason.detail()),
            )),
        }
    }

    if context.json {
        emit(
            context,
            &serde_json::json!({
                "registry": backend.source().name(),
                "root": backend.root().display().to_string(),
                "records": rows,
                "unreadable": unavailable
                    .iter()
                    .map(|(id, reason)| serde_json::json!({ "record_id": id, "reason": reason }))
                    .collect::<Vec<_>>(),
            }),
            "registry list report",
        )?;
    } else if !context.quiet {
        println!(
            "registry   {} ({})",
            backend.source().name(),
            backend.root().display()
        );
        for row in &rows {
            println!(
                "  {}  {}  {}",
                row["record_id"].as_str().unwrap_or("-"),
                row["signed_at"].as_str().unwrap_or("-"),
                row["content_sha256"].as_str().unwrap_or("-")
            );
        }
        for (id, reason) in &unavailable {
            println!("  {id}  unreadable: {reason}");
        }
        println!(
            "  {} record(s), {} unreadable",
            rows.len(),
            unavailable.len()
        );
    }
    Ok(exit::VERIFIED)
}

fn get(context: &Context, record_id: &str) -> Result<u8, CliError> {
    let name = registry_name(context)?;
    let backend = context.writable_registry(&name)?;
    let id = RecordId::parse_hex(record_id)?;
    match backend.fetch(&id) {
        Lookup::Found(record) => {
            let text = String::from_utf8_lossy(record.manifest_bytes()).into_owned();
            if !context.quiet {
                println!("{text}");
            }
            Ok(exit::VERIFIED)
        }
        Lookup::NotFound => {
            if !context.quiet {
                eprintln!("not_found: no record {record_id} in registry {name}");
            }
            Ok(exit::NOT_FOUND)
        }
        // An outage is not "unregistered", so it gets the operational code and never exit 3.
        Lookup::Unavailable(reason) => {
            if !context.quiet {
                eprintln!(
                    "{}: registry {name} could not answer: {}",
                    reason.kind().as_str(),
                    reason.detail()
                );
            }
            Ok(exit::INCOMPLETE)
        }
    }
}

fn registry_name(context: &Context) -> Result<String, CliError> {
    context
        .registry
        .clone()
        .ok_or_else(|| CliError::usage("name the registry to act on with --registry=<name>"))
}

fn emit(context: &Context, value: &serde_json::Value, what: &'static str) -> Result<(), CliError> {
    let rendered = serde_json::to_string_pretty(value)
        .map_err(|source| CliError::Serialise { what, source })?;
    if !context.quiet {
        println!("{rendered}");
    }
    Ok(())
}
