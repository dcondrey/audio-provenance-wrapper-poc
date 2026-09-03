use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::canonical::canonical_json_utf8;
use crate::error::{CoreError, Result};

pub const DEFAULT_EVIDENCE_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_EVIDENCE_BACKUPS: u32 = 3;

/// Appends one `canonical_json_utf8` record plus `\n`, rotating at the cap.
/// An oversize record is dropped and logged, never truncated into the file.
pub fn append_jsonl(
    path: &Path,
    record: &Value,
    max_bytes: u64,
    backup_count: u32,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|source| CoreError::io(parent, source))?;
        }
    }
    let mut encoded = canonical_json_utf8(record)?;
    encoded.push(b'\n');
    let encoded_len = encoded.len() as u64;
    if encoded_len > max_bytes {
        log::error!(
            "Dropped oversize evidence event ({encoded_len} bytes) for {}",
            path.display()
        );
        return Ok(());
    }
    let current_size = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if current_size > 0 && current_size.saturating_add(encoded_len) > max_bytes {
        rotate_jsonl(path, backup_count.max(1), max_bytes)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| CoreError::io(path, source))?;
    file.write_all(&encoded)
        .map_err(|source| CoreError::io(path, source))
}

/// Existing rotations oldest-first, then the active file.
pub fn rotated_evidence_paths(path: &Path) -> Result<Vec<PathBuf>> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let (stem, suffix) = stem_and_suffix(path);
    let prefix = format!("{stem}.");
    let mut rotations: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) && name.ends_with(&suffix) && name.len() > prefix.len() + suffix.len() - 1 {
                rotations.push(entry.path());
            }
        }
    }
    rotations.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    if path.is_file() {
        rotations.push(path.to_path_buf());
    }
    Ok(rotations)
}

fn rotate_jsonl(path: &Path, backup_count: u32, max_bytes: u64) -> Result<()> {
    let oldest = numbered(path, backup_count);
    if oldest.exists() {
        fs::remove_file(&oldest).map_err(|source| CoreError::io(&oldest, source))?;
    }
    for index in (1..backup_count).rev() {
        let source = numbered(path, index);
        if source.exists() {
            let target = numbered(path, index + 1);
            fs::rename(&source, &target).map_err(|err| CoreError::io(&source, err))?;
        }
    }
    if path.exists() {
        let target = numbered(path, 1);
        fs::rename(path, &target).map_err(|source| CoreError::io(path, source))?;
    }
    log::warn!(
        "Evidence limit reached; rotated {} (max={max_bytes} bytes, backups={backup_count})",
        path.display()
    );
    Ok(())
}

fn numbered(path: &Path, index: u32) -> PathBuf {
    let (stem, suffix) = stem_and_suffix(path);
    let parent = path.parent().unwrap_or(Path::new("."));
    parent.join(format!("{stem}.{index}{suffix}"))
}

/// `pathlib.Path.stem` / `.suffix`: the suffix is the LAST dot-segment, and a
/// name with no dot has an empty suffix.
fn stem_and_suffix(path: &Path) -> (String, String) {
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.rfind('.') {
        Some(index) if index > 0 => {
            let stem = name.get(..index).unwrap_or(&name).to_owned();
            let suffix = name.get(index..).unwrap_or("").to_owned();
            (stem, suffix)
        }
        _ => (name, String::new()),
    }
}
