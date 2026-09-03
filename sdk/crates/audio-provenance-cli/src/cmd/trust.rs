//! Issuance and inspection for the identity layer.
//!
//! Without these, a trust store is a hook with nothing on the other end of it. Every document
//! written here is Ed25519-signed by a key the operator holds; nothing in this module can raise a
//! proof level, and nothing it writes is trusted by a verifier that has not been given the anchor.

use std::path::{Path, PathBuf};

use audio_provenance_core::signer_id_for_public_key;
use audio_provenance_core::signing::fs::load_signing_key;
use audio_provenance_trust::{
    Anchor, Capability, Instant, RevocationEntry, RevocationList, RevocationReason, SignedAnchor,
    SignerRecord, TrustEvaluation, TrustStore, Window,
};
use clap::{Args, Subcommand};
use serde_json::Value;

use crate::context::{Context, now};
use crate::error::CliError;
use crate::exit;

const DEFAULT_VALID_DAYS: i64 = 3650;
const MAX_PUBLIC_KEY_FILE_BYTES: u64 = 4096;

#[derive(Debug, Args)]
pub struct TrustArgs {
    #[command(subcommand)]
    pub command: TrustCommand,
}

#[derive(Debug, Subcommand)]
pub enum TrustCommand {
    /// Create a self-signed authority: the root a verifier chooses to trust.
    InitAuthority(InitAuthorityArgs),
    /// Bind a public key to a display name under an authority.
    Issue(IssueArgs),
    /// Publish a signed revocation for a key.
    Revoke(RevokeArgs),
    /// Write a distributable store holding one anchor and nothing else.
    ExportAnchor(ExportAnchorArgs),
    /// Merge an anchor, record or revocation list into a store.
    Add(AddArgs),
    /// List what a store holds.
    Show(ShowArgs),
    /// Ask a store whether a key resolves to a name. Exits 0 when it does, 2 when it does not.
    /// This resolves an identity; it is not a verdict about any file.
    VerifySigner(VerifySignerArgs),
}

#[derive(Debug, Args)]
pub struct InitAuthorityArgs {
    /// The 32 raw private-key bytes the authority signs with.
    #[arg(long)]
    pub key: PathBuf,
    /// Lowercase `[a-z0-9._-]` identifier, stable for the life of the authority.
    #[arg(long)]
    pub anchor_id: String,
    /// The authority's display name, printed beside every identity it vouches for.
    #[arg(long)]
    pub name: String,
    #[arg(long, default_value_t = DEFAULT_VALID_DAYS)]
    pub valid_days: i64,
    /// Links permitted between a leaf record and this anchor.
    #[arg(long, default_value_t = 2)]
    pub max_chain_depth: u64,
    #[arg(long)]
    pub out: PathBuf,
}

#[derive(Debug, Args)]
pub struct IssueArgs {
    /// The anchor document this record chains to.
    #[arg(long)]
    pub anchor: PathBuf,
    /// The private key that signs the record: the anchor's own, or an issuing record's subject key.
    #[arg(long)]
    pub issuer_key: PathBuf,
    /// The subject's public key: 64 hex characters, or a path to the `.pub` file `keygen` wrote.
    #[arg(long)]
    pub subject_key: String,
    /// The name a verifier will print.
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub record_id: String,
    /// `leaf` may be named and may not issue; `issuer` may issue and is never named.
    #[arg(long, default_value = "leaf")]
    pub capability: String,
    #[arg(long, default_value_t = DEFAULT_VALID_DAYS)]
    pub valid_days: i64,
    #[arg(long)]
    pub out: PathBuf,
}

#[derive(Debug, Args)]
pub struct RevokeArgs {
    #[arg(long)]
    pub anchor: PathBuf,
    /// The anchor's own private key. Only an anchor revokes.
    #[arg(long)]
    pub key: PathBuf,
    #[arg(long)]
    pub subject_key: String,
    /// One of key_compromise, superseded, ceased_operation, unspecified.
    #[arg(long, default_value = "unspecified")]
    pub reason: String,
    /// The revocation list to write, extended in place when it already exists.
    #[arg(long)]
    pub out: PathBuf,
}

#[derive(Debug, Args)]
pub struct ExportAnchorArgs {
    #[arg(long)]
    pub anchor: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// The store to extend. Created when absent.
    #[arg(long)]
    pub store: PathBuf,
    /// An anchor, signer record, revocation list, or another store.
    #[arg(long)]
    pub document: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    #[arg(long)]
    pub store: PathBuf,
}

#[derive(Debug, Args)]
pub struct VerifySignerArgs {
    #[arg(long)]
    pub store: PathBuf,
    #[arg(long)]
    pub subject_key: String,
    /// Evaluate at this instant instead of now, for auditing a store rather than using one.
    #[arg(long)]
    pub at: Option<String>,
}

pub fn run(context: &Context, args: &TrustArgs) -> Result<u8, CliError> {
    match &args.command {
        TrustCommand::InitAuthority(args) => init_authority(context, args),
        TrustCommand::Issue(args) => issue(context, args),
        TrustCommand::Revoke(args) => revoke(context, args),
        TrustCommand::ExportAnchor(args) => export_anchor(context, args),
        TrustCommand::Add(args) => add(context, args),
        TrustCommand::Show(args) => show(context, args),
        TrustCommand::VerifySigner(args) => verify_signer(context, args),
    }
}

fn init_authority(context: &Context, args: &InitAuthorityArgs) -> Result<u8, CliError> {
    let key = load_signing_key(&args.key)?;
    let anchor = Anchor {
        anchor_id: args.anchor_id.clone(),
        name: args.name.clone(),
        public_key: key.public_key_bytes(),
        window: validity(args.valid_days)?,
        max_chain_depth: args.max_chain_depth,
    }
    .self_sign(&key)?;

    write_new(&args.out, &anchor.to_value())?;
    report(
        context,
        &args.out,
        &[
            ("anchor", anchor.anchor().anchor_id.clone()),
            ("name", anchor.anchor().name.clone()),
            ("key", anchor.anchor().key_id()),
            (
                "valid",
                format!(
                    "{} .. {}",
                    anchor.anchor().window.not_before,
                    anchor.anchor().window.not_after
                ),
            ),
            (
                "note",
                "a verifier trusts this anchor only once it is given it; distribute it with \
                 `audio-provenance trust export-anchor`"
                    .to_string(),
            ),
        ],
    )
}

fn issue(context: &Context, args: &IssueArgs) -> Result<u8, CliError> {
    let anchor = read_anchor(&args.anchor)?;
    let issuer = load_signing_key(&args.issuer_key)?;
    let subject = read_public_key(&args.subject_key)?;
    let record = SignerRecord {
        record_id: args.record_id.clone(),
        subject_public_key: subject,
        display_name: args.name.clone(),
        capability: Capability::parse(&args.capability)?,
        issuer_anchor_id: anchor.anchor().anchor_id.clone(),
        issuer_public_key: issuer.public_key_bytes(),
        window: validity(args.valid_days)?,
    }
    .sign(&issuer)?;

    write_new(&args.out, &record.to_value())?;
    report(
        context,
        &args.out,
        &[
            ("record", record.record().record_id.clone()),
            ("identity", record.record().display_name.clone()),
            ("subject", record.record().subject_signer_id()),
            (
                "capability",
                record.record().capability.as_str().to_string(),
            ),
            (
                "issuer",
                signer_id_for_public_key(&record.record().issuer_public_key),
            ),
            ("anchor", record.record().issuer_anchor_id.clone()),
            (
                "valid",
                format!(
                    "{} .. {}",
                    record.record().window.not_before,
                    record.record().window.not_after
                ),
            ),
        ],
    )
}

fn revoke(context: &Context, args: &RevokeArgs) -> Result<u8, CliError> {
    let anchor = read_anchor(&args.anchor)?;
    let key = load_signing_key(&args.key)?;
    let subject = read_public_key(&args.subject_key)?;
    let revoked_at = now()?;

    // Extending an existing list rather than replacing it: a revocation that silently dropped its
    // predecessors would un-revoke every key already on the list.
    let mut entries = match read_json_if_present(&args.out)? {
        Some(value) => audio_provenance_trust::SignedRevocationList::parse(&value)?
            .list()
            .entries
            .clone(),
        None => Vec::new(),
    };
    entries.retain(|entry| entry.subject_public_key != subject);
    entries.push(RevocationEntry {
        subject_public_key: subject,
        revoked_at: revoked_at.clone(),
        reason: RevocationReason::parse(&args.reason)?,
    });

    let list = RevocationList {
        anchor_id: anchor.anchor().anchor_id.clone(),
        issuer_public_key: key.public_key_bytes(),
        issued_at: revoked_at.clone(),
        entries,
    }
    .sign(&key)?;
    write_replacing(&args.out, &list.to_value())?;

    report(
        context,
        &args.out,
        &[
            ("revoked", signer_id_for_public_key(&subject)),
            ("anchor", list.list().anchor_id.clone()),
            ("at", revoked_at.to_string()),
            ("reason", args.reason.clone()),
            ("entries", list.list().entries.len().to_string()),
            (
                "note",
                "revocation is retroactive: this key is refused an identity regardless of when a \
                 signature claims to have been made. The record stays in the store and stays \
                 inspectable."
                    .to_string(),
            ),
        ],
    )
}

fn export_anchor(context: &Context, args: &ExportAnchorArgs) -> Result<u8, CliError> {
    let anchor = read_anchor(&args.anchor)?;
    let mut store = TrustStore::new();
    let key_id = anchor.anchor().key_id();
    let anchor_id = anchor.anchor().anchor_id.clone();
    store.insert_anchor(anchor)?;
    write_new(&args.out, &store.to_value())?;
    report(
        context,
        &args.out,
        &[
            ("anchor", anchor_id),
            ("key", key_id),
            (
                "note",
                "hand this to a verifier and pass it as --trust-store. It holds no private key."
                    .to_string(),
            ),
        ],
    )
}

fn add(context: &Context, args: &AddArgs) -> Result<u8, CliError> {
    if args.document.is_empty() {
        return Err(CliError::usage("--document is required at least once"));
    }
    let mut store = match read_json_if_present(&args.store)? {
        Some(value) => {
            let mut store = TrustStore::new();
            store.merge_value(&value)?;
            store
        }
        None => TrustStore::new(),
    };
    for path in &args.document {
        store.merge_value(&read_json(path)?)?;
    }
    write_replacing(&args.store, &store.to_value())?;
    report(
        context,
        &args.store,
        &[
            ("anchors", store.anchors().count().to_string()),
            ("records", store.records().count().to_string()),
            (
                "revocation lists",
                store.revocation_lists().count().to_string(),
            ),
        ],
    )
}

fn show(context: &Context, args: &ShowArgs) -> Result<u8, CliError> {
    let store = TrustStore::load(&args.store)?;
    if context.json {
        print_json(&store.to_value())?;
        return Ok(exit::VERIFIED);
    }
    if context.quiet {
        return Ok(exit::VERIFIED);
    }
    println!("store      {}", args.store.display());
    for anchor in store.anchors() {
        let anchor = anchor.anchor();
        println!(
            "  anchor   {} \"{}\" key {} depth<={} valid {} .. {}",
            anchor.anchor_id,
            anchor.name,
            anchor.key_id(),
            anchor.max_chain_depth,
            anchor.window.not_before,
            anchor.window.not_after
        );
    }
    for record in store.records() {
        let record = record.record();
        println!(
            "  record   {} \"{}\" {} subject {} issuer {} valid {} .. {}",
            record.record_id,
            record.display_name,
            record.capability.as_str(),
            record.subject_signer_id(),
            signer_id_for_public_key(&record.issuer_public_key),
            record.window.not_before,
            record.window.not_after
        );
    }
    for list in store.revocation_lists() {
        let list = list.list();
        for entry in &list.entries {
            println!(
                "  revoked  {} by anchor {} at {} ({})",
                signer_id_for_public_key(&entry.subject_public_key),
                list.anchor_id,
                entry.revoked_at,
                entry.reason.as_str()
            );
        }
    }
    Ok(exit::VERIFIED)
}

fn verify_signer(context: &Context, args: &VerifySignerArgs) -> Result<u8, CliError> {
    let store = TrustStore::load(&args.store)?;
    let at = match &args.at {
        Some(text) => Instant::parse_date_or_instant(text)?,
        None => now()?,
    };
    let subject = read_public_key(&args.subject_key)?;
    let evaluation = store.evaluate(&subject, &at);

    let (status, rows) = match &evaluation {
        TrustEvaluation::Vouched(identity) => (
            exit::VERIFIED,
            vec![
                ("identity", identity.display_name.clone()),
                ("proof level", "externally_verified".to_string()),
                ("anchor", identity.authority()),
                ("record", identity.record_id.clone()),
                ("evaluated at", at.to_string()),
            ],
        ),
        TrustEvaluation::NotCovered => (
            exit::UNTRUSTED,
            vec![
                ("identity", "not_established".to_string()),
                ("proof level", "unknown_unobserved".to_string()),
                (
                    "reason",
                    format!(
                        "no record in this store covers signer {}",
                        signer_id_for_public_key(&subject)
                    ),
                ),
                ("evaluated at", at.to_string()),
            ],
        ),
        TrustEvaluation::Refused(refusal) => (
            exit::UNTRUSTED,
            vec![
                ("identity", "not_established".to_string()),
                ("proof level", "unknown_unobserved".to_string()),
                ("refused", refusal.code().to_string()),
                ("reason", refusal.describe()),
                ("evaluated at", at.to_string()),
            ],
        ),
    };

    if context.json {
        let mut object = serde_json::Map::new();
        for (key, value) in &rows {
            object.insert((*key).replace(' ', "_"), Value::String(value.clone()));
        }
        print_json(&Value::Object(object))?;
    } else if !context.quiet {
        for (key, value) in &rows {
            println!("{key:<14} {value}");
        }
    }
    Ok(status)
}

fn validity(days: i64) -> Result<Window, CliError> {
    if days < 1 {
        return Err(CliError::usage("--valid-days must be at least 1"));
    }
    let start = now()?;
    let end = start.plus_days(days)?;
    Ok(Window::new(start, end)?)
}

fn read_anchor(path: &Path) -> Result<SignedAnchor, CliError> {
    Ok(SignedAnchor::parse(&read_json(path)?)?)
}

/// 64 hex characters, or the `.pub` file `audio-provenance keygen` wrote.
fn read_public_key(value: &str) -> Result<[u8; 32], CliError> {
    let text = if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        value.to_string()
    } else {
        let path = Path::new(value);
        let length = std::fs::metadata(path)
            .map_err(CliError::io("read", path.display()))?
            .len();
        if length > MAX_PUBLIC_KEY_FILE_BYTES {
            return Err(CliError::usage(format!(
                "{} is {length} bytes; a public key file holds 64 hex characters",
                path.display()
            )));
        }
        std::fs::read_to_string(path)
            .map_err(CliError::io("read", path.display()))?
            .trim()
            .to_string()
    };
    let raw = hex::decode(&text)
        .map_err(|_| CliError::usage("a public key is 64 lowercase hex characters"))?;
    raw.try_into()
        .map_err(|_| CliError::usage("a public key is 32 bytes"))
}

fn read_json(path: &Path) -> Result<Value, CliError> {
    let bytes = std::fs::read(path).map_err(CliError::io("read", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|source| CliError::Serialise {
        what: "trust document",
        source,
    })
}

fn read_json_if_present(path: &Path) -> Result<Option<Value>, CliError> {
    if path.exists() {
        return read_json(path).map(Some);
    }
    Ok(None)
}

fn write_new(path: &Path, value: &Value) -> Result<(), CliError> {
    if path.exists() {
        return Err(CliError::WouldOverwrite {
            path: path.display().to_string(),
        });
    }
    write_replacing(path, value)
}

fn write_replacing(path: &Path, value: &Value) -> Result<(), CliError> {
    let rendered = serde_json::to_string_pretty(value).map_err(|source| CliError::Serialise {
        what: "trust document",
        source,
    })?;
    std::fs::write(path, format!("{rendered}\n")).map_err(CliError::io("write", path.display()))
}

fn print_json(value: &Value) -> Result<(), CliError> {
    let rendered = serde_json::to_string_pretty(value).map_err(|source| CliError::Serialise {
        what: "trust report",
        source,
    })?;
    println!("{rendered}");
    Ok(())
}

fn report(context: &Context, path: &Path, rows: &[(&str, String)]) -> Result<u8, CliError> {
    if context.json {
        let mut object = serde_json::Map::new();
        object.insert(
            "path".to_string(),
            Value::String(path.display().to_string()),
        );
        for (key, value) in rows {
            object.insert((*key).replace(' ', "_"), Value::String(value.clone()));
        }
        print_json(&Value::Object(object))?;
    } else if !context.quiet {
        println!("wrote      {}", path.display());
        for (key, value) in rows {
            println!("  {key:<16} {value}");
        }
    }
    Ok(exit::VERIFIED)
}
