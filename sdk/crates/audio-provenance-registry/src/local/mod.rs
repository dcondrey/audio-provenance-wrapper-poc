mod atomic;
mod index;
mod paths;

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use crate::backend::{RegistryBackend, RegistryKind, RegistrySource, WritableRegistryBackend};
use crate::error::RegistryError;
use crate::ids::{ContentHash, Fingerprint, MarkId, RecordId};
use crate::lookup::{Lookup, Unavailable, UnavailableKind};
use crate::record::{
    AdvisoryScore, MAX_FINGERPRINT_CANDIDATES, MarkMatches, RegistryRecord, ScoredCandidate,
};

use index::{INDEX_FILE, Index, IndexEntry, RECORD_DIR};

/// A registry held in a directory: an index file plus one canonical record document each.
///
/// This is what makes offline verification real. Nothing here contacts a network.
#[derive(Debug)]
pub struct LocalRegistryBackend {
    source: RegistrySource,
    root: PathBuf,
    index_path: PathBuf,
    record_dir: PathBuf,
    cached: RwLock<Option<CachedIndex>>,
}

#[derive(Debug)]
struct CachedIndex {
    stamp: IndexStamp,
    index: Arc<Index>,
}

#[derive(Debug, PartialEq, Eq)]
struct IndexStamp {
    len: u64,
    modified: Option<SystemTime>,
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> RegistryError + '_ {
    move |source| RegistryError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// A filesystem fault is transient; anything else means the index disagrees with the records it
/// points at. Neither is a miss.
fn outage(error: RegistryError) -> Unavailable {
    let kind = match error {
        RegistryError::Io { .. } => UnavailableKind::Io,
        _ => UnavailableKind::IndexCorrupt,
    };
    Unavailable::new(kind, error.to_string())
}

impl LocalRegistryBackend {
    /// Open an existing registry. A root without an index is a configuration fault, reported
    /// here rather than surfacing later as an empty registry.
    pub fn open(name: impl Into<String>, root: &Path) -> Result<Self, RegistryError> {
        if !root.is_dir() {
            return Err(RegistryError::RootMissing {
                path: root.display().to_string(),
            });
        }
        let canonical = std::fs::canonicalize(root).map_err(io(root))?;
        let index_path = canonical.join(INDEX_FILE);
        if !index_path.is_file() {
            return Err(RegistryError::IndexMissing {
                path: index_path.display().to_string(),
            });
        }
        let record_dir = canonical.join(RECORD_DIR);
        let source =
            RegistrySource::new(name, RegistryKind::Local, canonical.display().to_string());
        Ok(Self {
            source,
            root: canonical,
            index_path,
            record_dir,
            cached: RwLock::new(None),
        })
    }

    /// Create the directory layout and an empty index if they are absent, then open it.
    pub fn init(name: impl Into<String>, root: &Path) -> Result<Self, RegistryError> {
        std::fs::create_dir_all(root).map_err(io(root))?;
        let record_dir = root.join(RECORD_DIR);
        std::fs::create_dir_all(&record_dir).map_err(io(&record_dir))?;
        let index_path = root.join(INDEX_FILE);
        if !index_path.exists() {
            atomic::write_atomic(&index_path, &Index::default().to_bytes()?)?;
        }
        Self::open(name, root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn stamp(&self) -> Result<IndexStamp, RegistryError> {
        let metadata = std::fs::metadata(&self.index_path).map_err(io(&self.index_path))?;
        Ok(IndexStamp {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }

    /// The parsed index, re-read whenever the file's size or mtime has moved.
    ///
    /// PERF: a verification ladder issues several lookups per file and a bench issues thousands,
    /// so the parse is not repeated for an unchanged index.
    fn index(&self) -> Result<Arc<Index>, RegistryError> {
        let stamp = self.stamp()?;
        if let Ok(guard) = self.cached.read()
            && let Some(cached) = guard.as_ref()
            && cached.stamp == stamp
        {
            return Ok(Arc::clone(&cached.index));
        }

        let bytes = std::fs::read(&self.index_path).map_err(io(&self.index_path))?;
        let index = Arc::new(Index::from_bytes(
            &bytes,
            &self.index_path.display().to_string(),
        )?);
        if let Ok(mut guard) = self.cached.write() {
            *guard = Some(CachedIndex {
                stamp,
                index: Arc::clone(&index),
            });
        }
        Ok(index)
    }

    fn read_entry(&self, entry: &IndexEntry) -> Result<RegistryRecord, RegistryError> {
        let path = paths::resolve_within(&self.root, &entry.path)?;
        let bytes = std::fs::read(&path).map_err(io(&path))?;
        let record = RegistryRecord::from_envelope_bytes(&bytes)?;
        if record.record_id() != entry.record_id {
            return Err(RegistryError::RecordIdMismatch {
                declared: entry.record_id.to_hex(),
                derived: record.record_id().to_hex(),
            });
        }
        if record.mark_id() != entry.mark_id {
            return Err(RegistryError::RecordIdMismatch {
                declared: entry.mark_id.to_hex(),
                derived: record.mark_id().to_hex(),
            });
        }
        Ok(record)
    }

    /// Store a record.
    ///
    /// IMPORTANT: the order is the crash-safety argument. The replacement index is built and
    /// validated in memory first, so a rejected record writes nothing at all; then the document
    /// lands, then the index that points at it. An interruption between the last two leaves an
    /// unreferenced document and the previous index fully intact.
    pub fn put(&self, record: &RegistryRecord) -> Result<(), RegistryError> {
        let relative = format!("{RECORD_DIR}/{}.json", record.record_id().to_hex());
        let entry = IndexEntry {
            record_id: record.record_id(),
            mark_id: record.mark_id(),
            content_sha256: record.content_hash(),
            fingerprint: record.fingerprint().cloned(),
            signed_at: record.signed_at().clone(),
            path: relative.clone(),
        };

        let current = self.index()?;
        // First writer wins a locator. A second record under an occupied one could only ever reach
        // `ambiguous_binding`, so admitting it would trade a working mark for two broken ones; an
        // idempotent re-put of the same record id is still accepted below.
        if let Some(existing) = current
            .by_mark(&entry.mark_id)
            .iter()
            .map(|held| held.record_id)
            .find(|held| *held != entry.record_id)
        {
            return Err(RegistryError::LocatorConflict {
                mark_id: entry.mark_id.to_hex(),
                existing: existing.to_hex(),
            });
        }

        let mut replacement = Index::default();
        for existing in current.entries() {
            if existing.record_id != entry.record_id {
                replacement.insert(existing.clone())?;
            }
        }
        replacement.insert(entry)?;
        let index_bytes = replacement.to_bytes()?;
        let document = record.to_envelope_bytes()?;

        std::fs::create_dir_all(&self.record_dir).map_err(io(&self.record_dir))?;
        let target = paths::resolve_within(&self.root, &relative)?;
        atomic::write_atomic(&target, &document)?;
        atomic::write_atomic(&self.index_path, &index_bytes)?;
        if let Ok(mut guard) = self.cached.write() {
            *guard = None;
        }
        Ok(())
    }

    /// Every record id in the index, for `audio-provenance registry list`.
    pub fn record_ids(&self) -> Result<Vec<RecordId>, RegistryError> {
        Ok(self
            .index()?
            .entries()
            .iter()
            .map(|entry| entry.record_id)
            .collect())
    }
}

impl RegistryBackend for LocalRegistryBackend {
    fn source(&self) -> &RegistrySource {
        &self.source
    }

    fn lookup_by_mark(&self, mark: &MarkId) -> Lookup<MarkMatches> {
        let index = match self.index() {
            Ok(index) => index,
            Err(error) => return Lookup::Unavailable(outage(error)),
        };
        let entries = index.by_mark(mark);
        if entries.is_empty() {
            return Lookup::NotFound;
        }
        let mut records = Vec::with_capacity(entries.len());
        for entry in entries {
            match self.read_entry(entry) {
                Ok(record) => records.push(record),
                Err(error) => return Lookup::Unavailable(outage(error)),
            }
        }
        match MarkMatches::new(records) {
            Ok(matches) => Lookup::Found(matches),
            Err(error) => Lookup::Unavailable(outage(error)),
        }
    }

    fn lookup_by_content_hash(&self, content_hash: &ContentHash) -> Lookup<RegistryRecord> {
        let index = match self.index() {
            Ok(index) => index,
            Err(error) => return Lookup::Unavailable(outage(error)),
        };
        let Some(entry) = index.by_content(content_hash) else {
            return Lookup::NotFound;
        };
        match self.read_entry(entry) {
            Ok(record) => Lookup::Found(record),
            Err(error) => Lookup::Unavailable(outage(error)),
        }
    }

    fn nearest_by_fingerprint(
        &self,
        query: &Fingerprint,
        limit: usize,
    ) -> Lookup<Vec<ScoredCandidate>> {
        let limit = limit.min(MAX_FINGERPRINT_CANDIDATES);
        if limit == 0 {
            return Lookup::NotFound;
        }
        let index = match self.index() {
            Ok(index) => index,
            Err(error) => return Lookup::Unavailable(outage(error)),
        };

        let mut scored: Vec<(f32, RecordId)> = index
            .entries()
            .iter()
            .filter_map(|entry| {
                let stored = entry.fingerprint.as_ref()?;
                let agreement = stored.bit_agreement(query)?;
                Some((agreement, entry.record_id))
            })
            .collect();
        scored.sort_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.1.cmp(&right.1))
        });
        scored.truncate(limit);

        let mut candidates = Vec::with_capacity(scored.len());
        for (agreement, record_id) in scored {
            match AdvisoryScore::new(agreement) {
                Ok(score) => candidates.push(ScoredCandidate::new(record_id, score)),
                Err(error) => return Lookup::Unavailable(outage(error)),
            }
        }
        if candidates.is_empty() {
            return Lookup::NotFound;
        }
        Lookup::Found(candidates)
    }

    fn fetch(&self, record_id: &RecordId) -> Lookup<RegistryRecord> {
        let index = match self.index() {
            Ok(index) => index,
            Err(error) => return Lookup::Unavailable(outage(error)),
        };
        let Some(entry) = index.by_record(record_id) else {
            return Lookup::NotFound;
        };
        match self.read_entry(entry) {
            Ok(record) => Lookup::Found(record),
            Err(error) => Lookup::Unavailable(outage(error)),
        }
    }
}

impl WritableRegistryBackend for LocalRegistryBackend {
    fn put(&self, record: &RegistryRecord) -> Result<(), RegistryError> {
        Self::put(self, record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_and_symlink_escapes_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(root.path()).unwrap();
        std::fs::create_dir_all(canonical.join(RECORD_DIR)).unwrap();

        for hostile in [
            "../../etc/passwd",
            "records/../../etc/passwd",
            "/etc/passwd",
            "records/..%2f..%2fetc",
            "records/./x.json",
            "records//x.json",
            "records/x.json.part",
            "records/A1B2.json",
        ] {
            let outcome = paths::resolve_within(&canonical, hostile);
            assert!(
                matches!(
                    outcome,
                    Err(RegistryError::PathComponent { .. })
                        | Err(RegistryError::PathEscapesRoot { .. })
                ),
                "{hostile} was not refused: {outcome:?}"
            );
        }

        let outside = tempfile::tempdir().unwrap();
        let secret = std::fs::canonicalize(outside.path())
            .unwrap()
            .join("secret");
        std::fs::write(&secret, b"{}").unwrap();
        std::os::unix::fs::symlink(&secret, canonical.join(RECORD_DIR).join("link.json")).unwrap();
        assert!(matches!(
            paths::resolve_within(&canonical, "records/link.json"),
            Err(RegistryError::PathEscapesRoot { .. })
        ));

        assert!(paths::resolve_within(&canonical, "records/ab12.json").is_ok());
    }
}
