use std::path::{Path, PathBuf};

use audio_provenance_audio::{BitDepth, wav};
use audio_provenance_core::signing::fs::load_signing_key;
use audio_provenance_core::{LOCATOR_SALT_BYTES, LocatorSalt, SigningKey, derive_locator};
use audio_provenance_manifest::{HardBinding, ManifestDraft, MarkBinding};
use audio_provenance_registry::{
    ContentHash, LocalRegistryBackend, Lookup, MarkId, RegistryBackend, RegistryRecord,
};
use clap::Args;
use apw_watermark::{Watermark, Payload};
use apw_trace::{IngestLimits, Ingested};

/// 24-bit is the default the marked copy is written at. 16-bit requantisation is a channel the
/// mark survives, but re-encoding a master downward is a loss the signer did not ask for; anything
/// else belongs to `audio-provenance embed --bit-depth`.
use crate::context::Context;
use crate::error::CliError;
use crate::{date, exit, riff};

const MARKED_BIT_DEPTH: BitDepth = BitDepth::Int24;

/// A fresh 48-bit locator collides with a registered one at about 2^-48 per roll, so eight is
/// already far past the point where another draw is the wrong diagnosis.
const LOCATOR_ATTEMPTS: usize = 8;

#[derive(Debug, Args)]
pub struct SignArgs {
    pub file: PathBuf,
    /// The 32 raw private-key bytes `audio-provenance keygen` wrote.
    #[arg(long)]
    pub key: PathBuf,
    /// Write the manifest into the file's own RIFF `aprv` chunk.
    #[arg(long)]
    pub embed: bool,
    #[arg(long, conflicts_with = "no_sidecar")]
    pub sidecar: Option<PathBuf>,
    #[arg(long)]
    pub no_sidecar: bool,
    /// Write the signed audio here instead of in place.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Signing date, YYYY-MM-DD. Defaults to today in UTC.
    #[arg(long)]
    pub signed_at: Option<String>,
    /// Embed a Watermark before hashing. Needs --out, and refuses an already-signed input.
    #[arg(long)]
    pub mark: bool,
}

pub fn run(context: &Context, args: &SignArgs) -> Result<u8, CliError> {
    let source = apw_trace::ingest_path(&args.file, apw_trace::IngestLimits::default())?;
    let key = load_signing_key(&args.key)?;

    // IMPORTANT: resolved BEFORE the audio is rewritten. The locator is checked against this
    // registry, and a registry that cannot answer has to stop the run while the master is still
    // untouched rather than after a marked copy is on disk.
    let registry = match &context.registry {
        Some(name) => Some((name.clone(), context.writable_registry(name)?)),
        None => None,
    };
    let plan = plan_mark(args, &source, &key, registry.as_ref())?;

    let signed_at = match &args.signed_at {
        Some(date) => date.clone(),
        None => date::today_utc()?,
    };

    let audio_out = args.out.clone().unwrap_or_else(|| args.file.clone());
    if audio_out != args.file && audio_out.exists() {
        return Err(CliError::WouldOverwrite {
            path: audio_out.display().to_string(),
        });
    }

    // IMPORTANT: mark, then hash, then sign. The digests are taken by re-ingesting the exact bytes
    // that will be written, never from the in-memory buffer, because the WAV encoder quantises and
    // a verifier decodes what is on disk.
    let ingested = match &plan.marked {
        Some(marked) => apw_trace::ingest_bytes(marked.bytes.clone(), IngestLimits::default())?,
        None => source,
    };

    // With --embed the signed bytes are the pre-chunk bytes, because a manifest cannot contain a
    // digest of a file that contains that manifest. The chunk leaves the decoded audio untouched,
    // so the decoded-audio digest still covers the delivered file and is what a verifier recomputes.
    let binding = HardBinding::new(
        ingested.content_sha256(),
        Some(ingested.content_bytes()),
        Some(&ingested.decoded_audio_sha256()),
    )?;
    let mut draft = ManifestDraft::new(&signed_at, binding)?.with_locator_salt(plan.salt);
    if let Some(marked) = &plan.marked {
        draft = draft.with_mark(MarkBinding {
            version: marked.payload.version(),
            namespace: marked.payload.namespace(),
        });
    }
    // IMPORTANT: from `ingested`, the marked buffer the digests above were taken from, never from
    // `source`. A reference extracted before marking describes audio that is on disk nowhere.
    let reference = apw_trace::reference_fingerprint(ingested.audio())?;
    if let Some(fingerprint) = &reference {
        draft = draft.with_fingerprint(fingerprint.clone());
    }
    let unsigned = draft.to_unsigned_value()?;
    let public_key_file = args
        .key
        .with_extension("pub")
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let signature = key.sign_manifest(&unsigned, &public_key_file)?;
    let manifest_bytes = draft.seal(&signature)?;

    let mut embedded_in = None;
    if args.embed {
        let rewritten = riff::set_manifest_chunk(ingested.bytes(), &manifest_bytes)?;
        std::fs::write(&audio_out, &rewritten)
            .map_err(CliError::io("write", audio_out.display()))?;
        embedded_in = Some(audio_out.clone());
    } else if audio_out != args.file || plan.marked.is_some() {
        // A marked run ALWAYS writes: the manifest binds the marked bytes, so leaving the source
        // in place would sign audio that is not on disk anywhere.
        std::fs::write(&audio_out, ingested.bytes())
            .map_err(CliError::io("write", audio_out.display()))?;
    }

    let sidecar_path = sidecar_target(args, &audio_out);
    if let Some(path) = &sidecar_path {
        std::fs::write(path, &manifest_bytes).map_err(CliError::io("write", path.display()))?;
    }

    let record_id = match &registry {
        Some((_, backend)) => {
            let record = RegistryRecord::from_signed_manifest(
                serde_json::from_slice(&manifest_bytes).map_err(|source| CliError::Serialise {
                    what: "sealed manifest",
                    source,
                })?,
                ContentHash::parse_hex(ingested.content_sha256())?,
                None,
                date::registry_signed_at(&signed_at)?,
            )?;
            backend.put(&record)?;
            Some(record.record_id().to_hex())
        }
        None => None,
    };

    report(
        context,
        &Signed {
            audio: &audio_out,
            signer_id: key.signer_id(),
            signed_at: &signed_at,
            content_sha256: ingested.content_sha256(),
            content_bytes: ingested.content_bytes(),
            decoded_audio_sha256: ingested.decoded_audio_sha256(),
            sidecar: sidecar_path.as_deref(),
            embedded_in: embedded_in.as_deref(),
            record_id: record_id.as_deref(),
            registry: registry.as_ref().map(|(name, _)| name.as_str()),
            locator_salt: plan.salt,
            mark: plan.marked.as_ref().map(|marked| marked.payload),
            reference_hex_len: reference
                .as_ref()
                .map(|fingerprint| fingerprint.digest_hex().len()),
        },
    )?;
    Ok(exit::VERIFIED)
}

struct Marked {
    bytes: Vec<u8>,
    payload: Payload,
}

/// The locator allocation, and the marked audio when one was asked for.
///
/// A salt is rolled either way: `locator_salt` is a required field of every Audio Provenance record, and an
/// unmarked one still names the locator a later mark for this work would have to carry.
struct MarkPlan {
    salt: LocatorSalt,
    marked: Option<Marked>,
}

fn fresh_salt() -> Result<LocatorSalt, CliError> {
    let mut raw = [0u8; LOCATOR_SALT_BYTES];
    if getrandom::fill(&mut raw).is_err() {
        return Err(CliError::Entropy {
            bytes: LOCATOR_SALT_BYTES,
        });
    }
    Ok(LocatorSalt::from_bytes(raw))
}

/// Stage one of the enforced order: allocate the locator, mark, then hash, then sign.
fn plan_mark(
    args: &SignArgs,
    source: &Ingested,
    key: &SigningKey,
    registry: Option<&(String, LocalRegistryBackend)>,
) -> Result<MarkPlan, CliError> {
    if !args.mark {
        return Ok(MarkPlan {
            salt: fresh_salt()?,
            marked: None,
        });
    }
    // The spec's own guard. Marking rewrites the audio a hard binding covers, so a file that
    // already carries a manifest cannot be marked without invalidating it.
    if apw_trace::container::find_embedded(source.bytes(), source.container())
        != apw_trace::container::EmbeddedOutcome::Absent
    {
        return Err(CliError::usage(
            "mark_after_sign: this file already carries an embedded manifest, and marking changes the audio that manifest's hard binding covers",
        ));
    }
    if apw_trace::sidecar_paths(&args.file)
        .iter()
        .any(|path| path.is_file())
    {
        return Err(CliError::usage(
            "mark_after_sign: this file already has a sidecar manifest, and marking changes the audio that manifest's hard binding covers",
        ));
    }
    // Marking replaces the audio, so it never happens in place: a master is not silently
    // overwritten with a re-encoded, marked copy.
    if args.out.is_none() {
        return Err(CliError::usage(
            "--mark rewrites the audio, so it needs --out=<path> rather than signing in place",
        ));
    }
    if args.embed {
        return Err(CliError::usage(
            "--mark and --embed together are not supported: mark with --out, then sign the marked file with --embed",
        ));
    }

    for _ in 0..LOCATOR_ATTEMPTS {
        let salt = fresh_salt()?;
        let payload = Payload::with_locator_bytes(
            apw_watermark::payload::VERSION,
            0,
            &derive_locator(&key.public_key_bytes(), &salt),
        )?;
        if let Some((name, backend)) = registry {
            let mark = MarkId::new(
                payload.version(),
                payload.namespace(),
                payload.locator_bytes(),
            )?;
            match backend.lookup_by_mark(&mark) {
                Lookup::Found(_) => continue,
                // An unanswered registry means an unknown locator, and marking against one would
                // write a payload that may already belong to somebody else's record.
                Lookup::Unavailable(reason) => {
                    return Err(CliError::LocatorCheckUnavailable {
                        name: name.clone(),
                        locator: hex::encode(payload.locator_bytes()),
                        detail: format!("{}: {}", reason.kind().as_str(), reason.detail()),
                    });
                }
                Lookup::NotFound => {}
            }
        }
        let marked = Watermark::public().embed(source.audio(), payload)?;
        return Ok(MarkPlan {
            salt,
            marked: Some(Marked {
                bytes: wav::encode(&marked, MARKED_BIT_DEPTH)?,
                payload,
            }),
        });
    }
    Err(CliError::LocatorAllocation {
        attempts: LOCATOR_ATTEMPTS,
    })
}

fn sidecar_target(args: &SignArgs, audio_out: &Path) -> Option<PathBuf> {
    if args.no_sidecar {
        return None;
    }
    if let Some(path) = &args.sidecar {
        return Some(path.clone());
    }
    if args.embed {
        return None;
    }
    apw_trace::sidecar_paths(audio_out).into_iter().next()
}

struct Signed<'a> {
    audio: &'a Path,
    signer_id: String,
    signed_at: &'a str,
    content_sha256: &'a str,
    content_bytes: u64,
    decoded_audio_sha256: String,
    sidecar: Option<&'a Path>,
    embedded_in: Option<&'a Path>,
    record_id: Option<&'a str>,
    registry: Option<&'a str>,
    locator_salt: LocatorSalt,
    mark: Option<Payload>,
    /// `None` when the work exceeded the reference-constellation budget, which is the one case
    /// where a signed record carries no soft binding and cannot survive a lossy path.
    reference_hex_len: Option<usize>,
}

fn report(context: &Context, signed: &Signed<'_>) -> Result<(), CliError> {
    if context.json {
        let rendered = serde_json::to_string_pretty(&serde_json::json!({
            "audio": signed.audio.display().to_string(),
            "signer_id": signed.signer_id,
            "signed_at": signed.signed_at,
            "content_sha256": signed.content_sha256,
            "content_bytes": signed.content_bytes,
            "decoded_audio_sha256": signed.decoded_audio_sha256,
            "sidecar": signed.sidecar.map(|path| path.display().to_string()),
            "embedded_in": signed.embedded_in.map(|path| path.display().to_string()),
            "record_id": signed.record_id,
            "registry": signed.registry,
            "locator_salt": signed.locator_salt.to_hex(),
            "reference_fingerprint": signed.reference_hex_len.map(|length| serde_json::json!({
                "algorithm": apw_trace::fingerprint::ALGORITHM_ID,
                "digest_hex_len": length,
            })),
            "mark": signed.mark.map(|payload| serde_json::json!({
                "algorithm": apw_watermark::ALGORITHM_ID,
                "version": payload.version(),
                "namespace": payload.namespace(),
                "locator": hex::encode(&payload.to_bytes()[1..]),
                "resolves_a_registry_record": signed.record_id.is_some(),
            })),
            "identity_proof_level": audio_provenance_core::ProofLevel::UnknownUnobserved.as_str(),
        }))
        .map_err(|source| CliError::Serialise {
            what: "sign report",
            source,
        })?;
        println!("{rendered}");
        return Ok(());
    }
    if context.quiet {
        return Ok(());
    }

    println!("signed     {}", signed.audio.display());
    println!("  signer_id  {}", signed.signer_id);
    println!("  signed     {}", signed.signed_at);
    println!(
        "  binding    {}  content, {} bytes",
        signed.content_sha256, signed.content_bytes
    );
    println!(
        "  pcm        {}  decoded audio",
        signed.decoded_audio_sha256
    );
    match signed.reference_hex_len {
        Some(length) => println!(
            "  reference  {}  {} hex, corroborates a lossy path",
            apw_trace::fingerprint::ALGORITHM_ID,
            length
        ),
        None => {
            println!(
                "  reference  none  this work is longer than the reference-constellation budget,"
            );
            println!(
                "             so a transcode of it can never reach verified; only the exact bytes can."
            );
        }
    }
    if let Some(path) = signed.sidecar {
        println!("  sidecar    {}", path.display());
    }
    if let Some(path) = signed.embedded_in {
        println!("  embedded   {} (RIFF aprv chunk)", path.display());
        println!(
            "  note       the chunk changed the file's bytes after they were hashed, so a verifier"
        );
        println!(
            "             recomputes the decoded-audio digest and reports hard_binding_decoded_audio_only."
        );
    }
    if let Some(payload) = signed.mark {
        println!(
            "  mark       {}  {}, namespace {}",
            hex::encode(&payload.to_bytes()[1..]),
            apw_watermark::ALGORITHM_ID,
            payload.namespace()
        );
        match (signed.record_id, signed.registry) {
            (Some(record), Some(name)) => {
                println!("  resolves   record {record} in registry {name}");
            }
            _ => {
                println!(
                    "  note       no registry was named, so this locator is published nowhere yet."
                );
                println!(
                    "             Re-run with --registry=<name>, or `audio-provenance registry add` the"
                );
                println!("             sidecar, before the mark can resolve anything.");
            }
        }
    }
    if let Some(record) = signed.record_id {
        println!("  record     {record}");
    }
    println!(
        "  note       this signature proves key possession, not identity. Verification reports"
    );
    println!(
        "             identity=not_established until a trust store anchors signer {} to a",
        signed.signer_id
    );
    println!("             name and the authority that vouches for it.");
    Ok(())
}
