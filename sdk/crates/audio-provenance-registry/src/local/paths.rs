use std::path::{Path, PathBuf};

use crate::error::RegistryError;

pub(crate) const MAX_COMPONENT_LEN: usize = 128;
pub(crate) const MAX_RELATIVE_LEN: usize = 512;
pub(crate) const PART_SUFFIX: &str = ".part";

fn component_is_safe(component: &str) -> bool {
    !component.is_empty()
        && component.len() <= MAX_COMPONENT_LEN
        && !component.starts_with('.')
        && !component.ends_with(PART_SUFFIX)
        && component.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-' || b == b'_'
        })
}

/// Resolve an index-supplied relative path against a registry root.
///
/// The index is untrusted input: anyone who can write it can point an entry anywhere. Two
/// independent gates apply. The charset whitelist admits only lowercase alphanumerics, `.`, `-`
/// and `_`, which excludes `..`, absolute paths and encoded separators by construction rather
/// than by blacklist. The confinement check then resolves symlinks and requires the result to
/// still sit under the canonical root.
pub(crate) fn resolve_within(
    canonical_root: &Path,
    relative: &str,
) -> Result<PathBuf, RegistryError> {
    if relative.is_empty() || relative.len() > MAX_RELATIVE_LEN {
        return Err(RegistryError::PathComponent {
            component: relative.chars().take(MAX_COMPONENT_LEN).collect(),
        });
    }

    let mut resolved = canonical_root.to_path_buf();
    for component in relative.split('/') {
        if !component_is_safe(component) {
            return Err(RegistryError::PathComponent {
                component: component.chars().take(MAX_COMPONENT_LEN).collect(),
            });
        }
        resolved.push(component);
    }

    if let Ok(metadata) = resolved.symlink_metadata()
        && metadata.file_type().is_symlink()
    {
        return Err(RegistryError::PathEscapesRoot {
            relative: relative.into(),
        });
    }

    let parent = resolved
        .parent()
        .ok_or_else(|| RegistryError::PathEscapesRoot {
            relative: relative.into(),
        })?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|source| RegistryError::Io {
        path: parent.display().to_string(),
        source,
    })?;
    if !canonical_parent.starts_with(canonical_root) {
        return Err(RegistryError::PathEscapesRoot {
            relative: relative.into(),
        });
    }

    Ok(resolved)
}
