//! The trust store: anchors, signer records and revocation lists, loaded from one JSON file or a
//! directory, with every count and every byte bounded.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::document::{SignedAnchor, SignedRecord, SignedRevocationList};
use crate::error::TrustError;

pub const STORE_FORMAT: &str = "audio-provenance-trust-store-v1";

/// A store is untrusted input read before anything in it is trusted, so every limit here is a
/// refusal to allocate on a stranger's say-so rather than a capacity estimate.
pub const MAX_STORE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_ANCHORS: usize = 256;
pub const MAX_RECORDS: usize = 4096;
pub const MAX_REVOCATION_LISTS: usize = 256;
pub const MAX_DIRECTORY_ENTRIES: usize = 8192;

/// Records naming one subject key that a lookup will try before giving up.
///
/// PERF: each candidate costs up to `CHAIN_DEPTH_CEILING` Ed25519 verifications, so without this
/// bound a store holding thousands of records for one key would turn a single lookup into tens of
/// thousands of signature checks. A signer legitimately has a handful: a current record, a renewal,
/// and history.
pub const MAX_RECORDS_PER_SUBJECT: usize = 16;

const ANCHOR_DIR: &str = "anchors";
const RECORD_DIR: &str = "records";
const REVOCATION_DIR: &str = "revocations";

/// The store filenames `--trust-store=<dir>` probes for, in order.
pub const STORE_FILE_NAMES: [&str; 2] = ["trust-store.json", "audio-provenance-trust-store.json"];

#[derive(Debug, Clone, Default)]
pub struct TrustStore {
    anchors: BTreeMap<String, SignedAnchor>,
    anchor_id_by_key: BTreeMap<[u8; 32], String>,
    records_by_subject: BTreeMap<[u8; 32], Vec<SignedRecord>>,
    record_ids: BTreeMap<String, ()>,
    revocations: Vec<SignedRevocationList>,
}

impl TrustStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// One JSON document, or a directory laid out as `anchors/`, `records/`, `revocations/`, or a
    /// directory holding one of [`STORE_FILE_NAMES`].
    pub fn load(path: &Path) -> Result<Self, TrustError> {
        if path.is_dir() {
            if path.join(ANCHOR_DIR).is_dir() {
                return Self::load_directory(path);
            }
            let file = STORE_FILE_NAMES
                .iter()
                .map(|name| path.join(name))
                .find(|candidate| candidate.is_file())
                .ok_or_else(|| TrustError::Malformed {
                    what: "trust store directory",
                    reason: format!(
                        "{} holds neither an {ANCHOR_DIR}/ directory nor one of {}",
                        path.display(),
                        STORE_FILE_NAMES.join(", ")
                    ),
                })?;
            return Self::load_file(&file);
        }
        Self::load_file(path)
    }

    pub fn load_file(path: &Path) -> Result<Self, TrustError> {
        let mut store = Self::new();
        store.merge_file(path)?;
        Ok(store)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, TrustError> {
        let mut store = Self::new();
        store.merge_json(bytes, "trust store")?;
        Ok(store)
    }

    fn load_directory(root: &Path) -> Result<Self, TrustError> {
        let mut store = Self::new();
        for (directory, kind) in [
            (ANCHOR_DIR, DocumentKind::Anchor),
            (RECORD_DIR, DocumentKind::Record),
            (REVOCATION_DIR, DocumentKind::Revocations),
        ] {
            let path = root.join(directory);
            if !path.is_dir() {
                continue;
            }
            for entry in json_files_in(&path)? {
                let value = read_json(&entry)?;
                store.insert_typed(&value, kind, &entry)?;
            }
        }
        Ok(store)
    }

    /// Merges one document or one store file into this store. This is how issuance builds a store
    /// incrementally, and it applies the same parse and the same limits as loading.
    pub fn merge_file(&mut self, path: &Path) -> Result<(), TrustError> {
        let bytes = read_bounded(path)?;
        self.merge_json(&bytes, "trust store")
    }

    fn merge_json(&mut self, bytes: &[u8], what: &'static str) -> Result<(), TrustError> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|error| TrustError::Malformed {
                what,
                reason: error.to_string(),
            })?;
        self.merge_value(&value)
    }

    /// Accepts either a whole store document or one bare anchor / record / revocation list, so a
    /// freshly issued document can be added without first being wrapped.
    pub fn merge_value(&mut self, value: &Value) -> Result<(), TrustError> {
        let declared = value
            .get("format")
            .and_then(Value::as_str)
            .or_else(|| value.get("type").and_then(Value::as_str));
        match declared {
            Some(STORE_FORMAT) => self.merge_store_document(value),
            Some(crate::document::ANCHOR_TYPE) => self.insert_anchor(SignedAnchor::parse(value)?),
            Some(crate::document::RECORD_TYPE) => self.insert_record(SignedRecord::parse(value)?),
            Some(crate::document::REVOCATIONS_TYPE) => {
                self.insert_revocations(SignedRevocationList::parse(value)?)
            }
            other => Err(TrustError::UnsupportedFormat {
                expected: STORE_FORMAT,
                found: other.unwrap_or("<absent>").chars().take(64).collect(),
            }),
        }
    }

    fn merge_store_document(&mut self, value: &Value) -> Result<(), TrustError> {
        let reader = crate::reader::Reader::new(
            value,
            "trust store",
            &["format", "anchors", "records", "revocations"],
        )?;
        for anchor in reader.array("anchors", MAX_ANCHORS)? {
            self.insert_anchor(SignedAnchor::parse(anchor)?)?;
        }
        for record in reader.array("records", MAX_RECORDS)? {
            self.insert_record(SignedRecord::parse(record)?)?;
        }
        for list in reader.array("revocations", MAX_REVOCATION_LISTS)? {
            self.insert_revocations(SignedRevocationList::parse(list)?)?;
        }
        Ok(())
    }

    fn insert_typed(
        &mut self,
        value: &Value,
        kind: DocumentKind,
        path: &Path,
    ) -> Result<(), TrustError> {
        let outcome = match kind {
            DocumentKind::Anchor => SignedAnchor::parse(value).and_then(|a| self.insert_anchor(a)),
            DocumentKind::Record => SignedRecord::parse(value).and_then(|r| self.insert_record(r)),
            DocumentKind::Revocations => {
                SignedRevocationList::parse(value).and_then(|l| self.insert_revocations(l))
            }
        };
        outcome.map_err(|error| TrustError::Malformed {
            what: kind.as_str(),
            reason: format!("{}: {error}", path.display()),
        })
    }

    pub fn insert_anchor(&mut self, anchor: SignedAnchor) -> Result<(), TrustError> {
        if self.anchors.len() >= MAX_ANCHORS {
            return Err(TrustError::TooMany {
                what: "anchors",
                limit: MAX_ANCHORS,
                found: self.anchors.len() + 1,
            });
        }
        let id = anchor.anchor().anchor_id.clone();
        let key = anchor.anchor().public_key;
        // Two anchors on one key, or one id on two keys, makes "which anchor vouched" unanswerable.
        // The result names the anchor, so an ambiguous store is refused rather than resolved.
        if self.anchors.contains_key(&id) || self.anchor_id_by_key.contains_key(&key) {
            return Err(TrustError::Duplicate { id });
        }
        self.anchor_id_by_key.insert(key, id.clone());
        self.anchors.insert(id, anchor);
        Ok(())
    }

    pub fn insert_record(&mut self, record: SignedRecord) -> Result<(), TrustError> {
        if self.record_ids.len() >= MAX_RECORDS {
            return Err(TrustError::TooMany {
                what: "records",
                limit: MAX_RECORDS,
                found: self.record_ids.len() + 1,
            });
        }
        let id = record.record().record_id.clone();
        if self.record_ids.contains_key(&id) {
            return Err(TrustError::Duplicate { id });
        }
        // IMPORTANT: every check runs before any mutation. A rejected insert that had already
        // registered the record id would leave the store holding an id for a record it does not
        // have, and the next legitimate insert of that id would be refused as a duplicate.
        let held = self
            .records_by_subject
            .get(&record.record().subject_public_key)
            .map_or(0, Vec::len);
        if held >= MAX_RECORDS_PER_SUBJECT {
            return Err(TrustError::TooMany {
                what: "records for one subject key",
                limit: MAX_RECORDS_PER_SUBJECT,
                found: held + 1,
            });
        }
        self.record_ids.insert(id, ());
        self.records_by_subject
            .entry(record.record().subject_public_key)
            .or_default()
            .push(record);
        Ok(())
    }

    pub fn insert_revocations(&mut self, list: SignedRevocationList) -> Result<(), TrustError> {
        if self.revocations.len() >= MAX_REVOCATION_LISTS {
            return Err(TrustError::TooMany {
                what: "revocation lists",
                limit: MAX_REVOCATION_LISTS,
                found: self.revocations.len() + 1,
            });
        }
        self.revocations.push(list);
        Ok(())
    }

    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "format": STORE_FORMAT,
            "anchors": self.anchors.values().map(SignedAnchor::to_value).collect::<Vec<_>>(),
            "records": self
                .records_by_subject
                .values()
                .flatten()
                .map(SignedRecord::to_value)
                .collect::<Vec<_>>(),
            "revocations": self
                .revocations
                .iter()
                .map(SignedRevocationList::to_value)
                .collect::<Vec<_>>(),
        })
    }

    pub fn anchors(&self) -> impl Iterator<Item = &SignedAnchor> {
        self.anchors.values()
    }

    pub fn records(&self) -> impl Iterator<Item = &SignedRecord> {
        self.records_by_subject.values().flatten()
    }

    pub fn revocation_lists(&self) -> impl Iterator<Item = &SignedRevocationList> {
        self.revocations.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty() && self.record_ids.is_empty()
    }

    pub(crate) fn anchor_by_key(&self, key: &[u8; 32]) -> Option<&SignedAnchor> {
        self.anchor_id_by_key
            .get(key)
            .and_then(|id| self.anchors.get(id))
    }

    pub(crate) fn records_for(&self, subject: &[u8; 32]) -> &[SignedRecord] {
        self.records_by_subject
            .get(subject)
            .map_or(&[], Vec::as_slice)
    }
}

#[derive(Debug, Clone, Copy)]
enum DocumentKind {
    Anchor,
    Record,
    Revocations,
}

impl DocumentKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Anchor => "trust anchor",
            Self::Record => "signer record",
            Self::Revocations => "revocation list",
        }
    }
}

fn json_files_in(directory: &Path) -> Result<Vec<PathBuf>, TrustError> {
    let entries =
        std::fs::read_dir(directory).map_err(TrustError::io("read", directory.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(TrustError::io("read", directory.display()))?;
        if files.len() >= MAX_DIRECTORY_ENTRIES {
            return Err(TrustError::TooMany {
                what: "trust store files",
                limit: MAX_DIRECTORY_ENTRIES,
                found: files.len() + 1,
            });
        }
        let path = entry.path();
        // Only regular files, so a symlink to a device or a FIFO cannot block the load.
        if path.extension().is_some_and(|ext| ext == "json")
            && entry
                .metadata()
                .map_err(TrustError::io("read", path.display()))?
                .is_file()
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, TrustError> {
    let metadata = std::fs::metadata(path).map_err(TrustError::io("read", path.display()))?;
    if metadata.len() > MAX_STORE_BYTES {
        return Err(TrustError::StoreTooLarge {
            path: path.display().to_string(),
            limit: MAX_STORE_BYTES,
            found: metadata.len(),
        });
    }
    std::fs::read(path).map_err(TrustError::io("read", path.display()))
}

fn read_json(path: &Path) -> Result<Value, TrustError> {
    let bytes = read_bounded(path)?;
    serde_json::from_slice(&bytes).map_err(|error| TrustError::Malformed {
        what: "trust document",
        reason: format!("{}: {error}", path.display()),
    })
}
