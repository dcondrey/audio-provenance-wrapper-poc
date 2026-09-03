use std::collections::BTreeMap;

use audio_provenance_core::canonical_json;
use serde::{Deserialize, Serialize};

use crate::error::RegistryError;
use crate::ids::{ContentHash, Fingerprint, MarkId, RecordId, SignedAt};
use crate::record::MAX_MARK_MATCHES;

pub(crate) const INDEX_FORMAT: &str = "audio-provenance-registry-index-v0";
pub(crate) const INDEX_FILE: &str = "index.json";
pub(crate) const RECORD_DIR: &str = "records";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct IndexEntry {
    pub(crate) record_id: RecordId,
    pub(crate) mark_id: MarkId,
    pub(crate) content_sha256: ContentHash,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) fingerprint: Option<Fingerprint>,
    pub(crate) signed_at: SignedAt,
    pub(crate) path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct IndexFile {
    index_format: String,
    entries: Vec<IndexEntry>,
}

/// What to do when a locator bucket is already at [`MAX_MARK_MATCHES`].
///
/// IMPORTANT: the write path refuses and the LOAD path caps. `from_bytes` inserts every entry, so a
/// refusal there would turn one over-full bucket into `Unavailable(IndexCorrupt)` for every lookup
/// in the registry, which is a whole-registry outage reachable from a bucket size an adversary can
/// influence. A duplicate record id or a content-hash conflict still refuses on both paths: those
/// are corruption, not crowding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BucketFull {
    Refuse,
    Cap,
}

#[derive(Debug, Default)]
pub(crate) struct Index {
    entries: Vec<IndexEntry>,
    by_record: BTreeMap<RecordId, usize>,
    by_mark: BTreeMap<MarkId, Vec<usize>>,
    by_content: BTreeMap<ContentHash, usize>,
}

impl Index {
    pub(crate) fn from_bytes(bytes: &[u8], path: &str) -> Result<Self, RegistryError> {
        let file: IndexFile =
            serde_json::from_slice(bytes).map_err(|source| RegistryError::Json {
                path: path.into(),
                source,
            })?;
        if file.index_format != INDEX_FORMAT {
            return Err(RegistryError::IndexFormat {
                found: file.index_format,
                expected: INDEX_FORMAT,
            });
        }
        let mut index = Self::default();
        for entry in file.entries {
            index.insert_bounded(entry, BucketFull::Cap)?;
        }
        Ok(index)
    }

    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>, RegistryError> {
        let mut entries = self.entries.clone();
        entries.sort_by_key(|entry| entry.record_id);
        let file = IndexFile {
            index_format: INDEX_FORMAT.into(),
            entries,
        };
        let value =
            serde_json::to_value(&file).map_err(|error| RegistryError::MalformedRecord {
                reason: error.to_string(),
            })?;
        Ok(canonical_json(&value)?)
    }

    pub(crate) fn insert(&mut self, entry: IndexEntry) -> Result<(), RegistryError> {
        self.insert_bounded(entry, BucketFull::Refuse)
    }

    fn insert_bounded(
        &mut self,
        entry: IndexEntry,
        bucket_full: BucketFull,
    ) -> Result<(), RegistryError> {
        if self.by_record.contains_key(&entry.record_id) {
            return Err(RegistryError::IndexDuplicateRecord {
                record_id: entry.record_id.to_hex(),
            });
        }
        if let Some(&existing) = self.by_content.get(&entry.content_sha256) {
            let existing_id = self
                .entries
                .get(existing)
                .map(|other| other.record_id.to_hex())
                .unwrap_or_default();
            return Err(RegistryError::ContentHashConflict {
                content_sha256: entry.content_sha256.to_hex(),
                existing: existing_id,
            });
        }
        let position = self.entries.len();
        let bucket = self.by_mark.entry(entry.mark_id).or_default();
        if bucket.len() >= MAX_MARK_MATCHES {
            if bucket_full == BucketFull::Refuse {
                return Err(RegistryError::MarkMatchCount {
                    limit: MAX_MARK_MATCHES,
                    found: bucket.len() + 1,
                });
            }
        } else {
            bucket.push(position);
        }
        self.by_record.insert(entry.record_id, position);
        self.by_content.insert(entry.content_sha256, position);
        self.entries.push(entry);
        Ok(())
    }

    pub(crate) fn by_record(&self, record_id: &RecordId) -> Option<&IndexEntry> {
        self.by_record
            .get(record_id)
            .and_then(|&position| self.entries.get(position))
    }

    pub(crate) fn by_content(&self, content_hash: &ContentHash) -> Option<&IndexEntry> {
        self.by_content
            .get(content_hash)
            .and_then(|&position| self.entries.get(position))
    }

    pub(crate) fn by_mark(&self, mark_id: &MarkId) -> Vec<&IndexEntry> {
        self.by_mark
            .get(mark_id)
            .map(|bucket| {
                bucket
                    .iter()
                    .filter_map(|&position| self.entries.get(position))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn entries(&self) -> &[IndexEntry] {
        &self.entries
    }
}
