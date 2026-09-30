use apw_core::{
    file_hash_op_name, ots_attestation_summary, parse_detached, sha256_hex, BlockHeader, CheckStatus,
    HeaderSource, LocalHeaderSource, OP_SHA256, DEFAULT_CALENDARS,
};
use apw_daemon::{upgrade, ExplorerHeaderSource, HttpCalendarTransport};
use serde_json::json;

use crate::cli::{OtsAction, OtsArgs};
use crate::error::{CliError, Result};

pub fn run(args: &OtsArgs) -> Result<i32> {
    match &args.action {
        OtsAction::Upgrade { proof, calendars, out } => {
            let bytes = std::fs::read(proof).map_err(|source| CliError::io("read", proof, source))?;
            let mut parsed = parse_detached(&bytes).map_err(|error| CliError::usage(format!("cannot read the proof: {error}")))?;
            let mut allowed: Vec<String> = DEFAULT_CALENDARS.iter().map(|url| (*url).to_owned()).collect();
            allowed.extend(calendars.iter().cloned());
            let outcomes = upgrade(&mut parsed, &allowed, &HttpCalendarTransport::new());
            let destination = out.clone().unwrap_or_else(|| {
                let mut name = proof.clone().into_os_string();
                name.push(".upgraded");
                name.into()
            });
            let serialized = parsed
                .serialize()
                .map_err(|error| CliError::usage(format!("cannot write the proof: {error}")))?;
            std::fs::write(&destination, serialized).map_err(|source| CliError::io("write", &destination, source))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "out": destination.display().to_string(),
                    "outcomes": outcomes,
                    "attestations": ots_attestation_summary(&parsed.timestamp),
                }))?
            );
            Ok(0)
        }
        OtsAction::Verify { proof, file, digest, explorer, header } => {
            let bytes = std::fs::read(proof).map_err(|source| CliError::io("read", proof, source))?;
            let parsed = parse_detached(&bytes).map_err(|error| CliError::usage(format!("cannot read the proof: {error}")))?;
            let wanted = match (file, digest) {
                (Some(path), _) => {
                    sha256_hex(&std::fs::read(path).map_err(|source| CliError::io("read", path, source))?)
                }
                (None, Some(digest)) => digest.clone(),
                (None, None) => return Err(CliError::usage("give --file or --digest")),
            };
            if parsed.file_hash_op != OP_SHA256 || apw_core::ots_hex(parsed.file_digest()) != wanted {
                println!("the proof is for a different file digest ({})", file_hash_op_name(parsed.file_hash_op));
                return Ok(1);
            }
            let source = header_source(explorer.as_deref(), header)?;
            let Some(source) = source else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "attestations": ots_attestation_summary(&parsed.timestamp),
                        "checked": false,
                    }))?
                );
                return Ok(0);
            };
            let checks = apw_core::check_bitcoin_attestations(&parsed.timestamp, source.as_ref());
            let rendered: Vec<_> = checks
                .iter()
                .map(|check| {
                    json!({
                        "height": check.height,
                        "status": match check.status {
                            CheckStatus::Verified => "verified",
                            CheckStatus::Mismatch => "mismatch",
                            CheckStatus::Unavailable => "unavailable",
                        },
                        "block_time": check.block_time,
                        "source": check.source,
                        "reason": check.reason,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&rendered)?);
            let all_verified = !checks.is_empty() && checks.iter().all(|check| check.status == CheckStatus::Verified);
            Ok(if all_verified { 0 } else { 1 })
        }
    }
}

/// The header source a `--header` or `--explorer` flag pair names; `--header` wins.
pub fn header_source(explorer: Option<&str>, headers: &[String]) -> Result<Option<Box<dyn HeaderSource>>> {
    if headers.is_empty() {
        return Ok(explorer.map(|url| Box::new(ExplorerHeaderSource::new(url)) as Box<dyn HeaderSource>));
    }
    let mut parsed = Vec::new();
    for item in headers {
        let (height, raw) = item
            .split_once(':')
            .ok_or_else(|| CliError::usage("--header expects HEIGHT:HEX"))?;
        let height = height.parse::<u64>().map_err(|_| CliError::usage("bad header height"))?;
        let raw = apw_core::python_from_hex(raw).ok_or_else(|| CliError::usage("bad header hex"))?;
        parsed.push((height, BlockHeader::new(&raw).map_err(|error| CliError::usage(error.to_string()))?));
    }
    Ok(Some(Box::new(LocalHeaderSource::new(parsed))))
}
