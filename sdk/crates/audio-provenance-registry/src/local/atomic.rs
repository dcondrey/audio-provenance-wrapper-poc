use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::RegistryError;

static STAGE_NONCE: AtomicU64 = AtomicU64::new(0);

/// Bytes written and flushed to a sibling `.part` file, not yet visible at the target path.
///
/// Dropping without `commit` removes the temporary file, so an error path never leaves debris
/// and never leaves the target half-written.
#[derive(Debug)]
pub(crate) struct Staged {
    part: PathBuf,
    target: PathBuf,
    committed: bool,
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> RegistryError + '_ {
    move |source| RegistryError::Io {
        path: path.display().to_string(),
        source,
    }
}

pub(crate) fn stage(target: &Path, bytes: &[u8]) -> Result<Staged, RegistryError> {
    let parent = target
        .parent()
        .ok_or_else(|| RegistryError::PathEscapesRoot {
            relative: target.display().to_string(),
        })?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| RegistryError::PathComponent {
            component: target.display().to_string(),
        })?;

    // Same directory as the target, so the commit is a same-device rename.
    let nonce = STAGE_NONCE.fetch_add(1, Ordering::Relaxed);
    let part = parent.join(format!(
        "{name}.{}-{nonce}{}",
        std::process::id(),
        super::paths::PART_SUFFIX
    ));

    let mut file = File::create(&part).map_err(io(&part))?;
    let staged = Staged {
        part: part.clone(),
        target: target.to_path_buf(),
        committed: false,
    };
    file.write_all(bytes).map_err(io(&part))?;
    // REQUIRED: renaming an unsynced file can survive a crash as a zero-length target, which
    // would destroy the previous contents rather than preserve them.
    file.sync_all().map_err(io(&part))?;
    // IMPORTANT: closes the handle before the rename. `wasm32-unknown-unknown` compiles a
    // `File` with no `Drop` impl, which makes clippy read this as a no-op; on every target
    // that has a filesystem it is not one.
    #[allow(clippy::drop_non_drop)]
    drop(file);
    Ok(staged)
}

impl Staged {
    pub(crate) fn commit(mut self) -> Result<(), RegistryError> {
        fs::rename(&self.part, &self.target).map_err(io(&self.part))?;
        self.committed = true;
        if let Ok(dir) = File::open(self.target.parent().unwrap_or_else(|| Path::new("."))) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn part_path(&self) -> &Path {
        &self.part
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.part);
        }
    }
}

pub(crate) fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), RegistryError> {
    stage(target, bytes)?.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_bytes_are_invisible_until_commit_and_vanish_without_it() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("index.json");
        write_atomic(&target, b"v1").unwrap();

        let staged = stage(&target, b"v2").unwrap();
        let part = staged.part_path().to_path_buf();
        assert!(part.is_file());
        assert_eq!(fs::read(&target).unwrap(), b"v1");

        drop(staged);
        assert!(!part.exists(), "an abandoned stage left debris behind");
        assert_eq!(fs::read(&target).unwrap(), b"v1");

        stage(&target, b"v2").unwrap().commit().unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"v2");
    }
}
