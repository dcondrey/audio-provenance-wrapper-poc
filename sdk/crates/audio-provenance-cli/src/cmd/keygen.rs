use std::io::Write;
use std::path::PathBuf;

use audio_provenance_core::SigningKey;
use clap::Args;
use zeroize::Zeroize;

use crate::context::Context;
use crate::error::CliError;
use crate::exit;

const PRIVATE_KEY_BYTES: usize = 32;

#[derive(Debug, Args)]
pub struct KeygenArgs {
    /// Where to write the 32 raw private-key bytes. Created 0600 and never overwritten.
    #[arg(long)]
    pub out: PathBuf,
}

pub fn run(context: &Context, args: &KeygenArgs) -> Result<u8, CliError> {
    if args.out.exists() {
        return Err(CliError::WouldOverwrite {
            path: args.out.display().to_string(),
        });
    }

    let mut secret = [0u8; PRIVATE_KEY_BYTES];
    if getrandom::fill(&mut secret).is_err() {
        secret.zeroize();
        return Err(CliError::Entropy {
            bytes: PRIVATE_KEY_BYTES,
        });
    }
    let key = SigningKey::from_raw_bytes(&secret);
    let write = write_private_key(&args.out, &secret);
    secret.zeroize();
    write?;
    let key = key?;

    let public_key_path = args.out.with_extension("pub");
    std::fs::write(&public_key_path, format!("{}\n", key.public_key_hex()))
        .map_err(CliError::io("write", public_key_path.display()))?;

    if context.json {
        let rendered = serde_json::to_string_pretty(&serde_json::json!({
            "signer_id": key.signer_id(),
            "public_key_hex": key.public_key_hex(),
            "private_key_file": args.out.display().to_string(),
            "public_key_file": public_key_path.display().to_string(),
        }))
        .map_err(|source| CliError::Serialise {
            what: "keygen report",
            source,
        })?;
        println!("{rendered}");
    } else if !context.quiet {
        println!("keyed      {}", args.out.display());
        println!("  signer_id  {}", key.signer_id());
        println!("  public     {}", key.public_key_hex());
        println!("  public key {}", public_key_path.display());
        println!(
            "  note       a signature from this key proves key possession, not identity. Until a"
        );
        println!(
            "             trust store anchors signer {} to a name and an authority, every",
            key.signer_id()
        );
        println!("             verification of it reports identity=not_established.");
    }
    Ok(exit::VERIFIED)
}

#[cfg(unix)]
fn write_private_key(path: &std::path::Path, secret: &[u8]) -> Result<(), CliError> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        // IMPORTANT: create_new, not create. The existence check above races; this is what actually
        // refuses to write over a key another process created in between.
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(CliError::io("create", path.display()))?;
    file.write_all(secret)
        .map_err(CliError::io("write", path.display()))?;
    file.sync_all()
        .map_err(CliError::io("flush", path.display()))
}

#[cfg(not(unix))]
fn write_private_key(path: &std::path::Path, secret: &[u8]) -> Result<(), CliError> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(CliError::io("create", path.display()))?;
    file.write_all(secret)
        .map_err(CliError::io("write", path.display()))?;
    file.sync_all()
        .map_err(CliError::io("flush", path.display()))
}
