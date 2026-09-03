//! The posting index behind rung 5, and its on-disk form.
//!
//! # The client never trusts a supplied score
//!
//! Nothing here returns a similarity. An index returns postings; [`crate::fingerprint::score`]
//! re-derives the offset histogram locally. A registry that can fabricate a score can fabricate a
//! match, so `audio_provenance_registry::AdvisoryScore` is deliberately not a path into a verdict.
//!
//! # An index file is untrusted input
//!
//! `fp.idx` may come from anywhere. Every offset is bounds-checked against the file's own posting
//! count, buckets are required to be non-decreasing, and a bucket over
//! [`MAX_POSTINGS_PER_HASH`] is dropped rather than read: a build that honoured the cap cannot
//! produce one, and a hash appearing in that many recordings carries no discriminative information
//! anyway.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use audio_provenance_registry::RecordId;

use crate::error::GenoTraceError;

pub const INDEX_MAGIC: [u8; 8] = *b"GTOFPX01";

/// The offset table spans the 24 discriminative bits of a landmark hash, plus a sentinel.
pub const PREFIX_SPACE: usize = 1 << 24;
const OFFSET_TABLE_ENTRIES: usize = PREFIX_SPACE + 1;
const OFFSET_TABLE_BYTES: u64 = (OFFSET_TABLE_ENTRIES as u64) * 4;
const HEADER_BYTES: u64 = 40;
const POSTING_BYTES: u64 = 16;
const TRACK_ENTRY_BYTES: u64 = 8 + 32;

/// A hash in more than this many recordings is not discriminative and is dropped at build time.
pub const MAX_POSTINGS_PER_HASH: usize = 1000;

/// Total postings a single query may scan before the rung aborts.
pub const DEFAULT_POSTING_BUDGET: usize = 2_000_000;

const MAX_TRACKS: usize = 1 << 21;

/// One index entry: which recording, and where in it the anchor sat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Posting {
    pub track: u64,
    pub t1: u32,
}

/// A source of postings for a landmark hash.
pub trait FingerprintIndex: std::fmt::Debug + Send + Sync {
    /// Postings for exactly this hash. An empty slice is a miss, never an error.
    fn postings_for(&self, hash: u32) -> Result<Vec<Posting>, GenoTraceError>;

    /// The registry record a track key names. `None` means the index knows the track but not what
    /// it points at, which is a corrupt index, not a miss.
    fn record_id(&self, track: u64) -> Option<RecordId>;

    fn track_count(&self) -> usize;
}

/// The in-memory index. Also the builder: [`Self::write_to`] emits the on-disk form.
#[derive(Debug, Clone, Default)]
pub struct MemoryFingerprintIndex {
    postings: BTreeMap<u32, Vec<Posting>>,
    tracks: BTreeMap<u64, RecordId>,
}

impl MemoryFingerprintIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_track(&mut self, track: u64, record: RecordId) {
        self.tracks.insert(track, record);
    }

    /// Adds one recording's landmarks. Over-full buckets are trimmed here, so the on-disk cap and
    /// the in-memory cap are the same rule rather than two rules that can drift.
    pub fn insert_landmarks(
        &mut self,
        track: u64,
        record: RecordId,
        landmarks: &[crate::fingerprint::landmark::Landmark],
    ) {
        self.tracks.insert(track, record);
        for landmark in landmarks {
            let bucket = self.postings.entry(landmark.hash).or_default();
            if bucket.len() >= MAX_POSTINGS_PER_HASH {
                continue;
            }
            bucket.push(Posting {
                track,
                t1: landmark.t1,
            });
        }
    }

    pub fn posting_count(&self) -> usize {
        self.postings.values().map(Vec::len).sum()
    }

    /// Writes `fp.idx`: header, then the prefix offset table, then postings sorted by
    /// `(hash, track, t1)`, then the track table.
    pub fn write_to(&self, path: &Path) -> Result<(), GenoTraceError> {
        let mut file = File::create(path).map_err(|source| GenoTraceError::Unreadable {
            path: path.display().to_string(),
            source,
        })?;

        let mut flat: Vec<(u32, Posting)> = Vec::with_capacity(self.posting_count());
        for (hash, bucket) in &self.postings {
            let mut sorted = bucket.clone();
            sorted.sort_unstable();
            flat.extend(sorted.into_iter().map(|posting| (*hash, posting)));
        }
        flat.sort_by_key(|(hash, posting)| (*hash, posting.track, posting.t1));

        if flat.len() > u32::MAX as usize {
            return Err(GenoTraceError::FingerprintIndexMalformed {
                reason: format!("{} postings exceed the u32 offset table", flat.len()),
            });
        }
        let posting_count = flat.len() as u64;
        let track_table_offset =
            HEADER_BYTES + OFFSET_TABLE_BYTES + posting_count.saturating_mul(POSTING_BYTES);
        let track_table_len = (self.tracks.len() as u64).saturating_mul(TRACK_ENTRY_BYTES);

        let mut header = Vec::with_capacity(HEADER_BYTES as usize);
        header.extend_from_slice(&INDEX_MAGIC);
        header.extend_from_slice(
            &u32::from(crate::fingerprint::landmark::SCHEME_VERSION).to_le_bytes(),
        );
        header.extend_from_slice(&posting_count.to_le_bytes());
        header.extend_from_slice(&track_table_offset.to_le_bytes());
        header.extend_from_slice(&track_table_len.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        write_all(&mut file, &header, path)?;

        let mut table = vec![0u32; OFFSET_TABLE_ENTRIES];
        for (hash, _) in &flat {
            let prefix = crate::fingerprint::landmark::hash_prefix(*hash) as usize;
            if let Some(slot) = table.get_mut(prefix) {
                *slot += 1;
            }
        }
        let mut running = 0u32;
        for slot in &mut table {
            let count = *slot;
            *slot = running;
            running += count;
        }
        let mut table_bytes = Vec::with_capacity(OFFSET_TABLE_BYTES as usize);
        for value in &table {
            table_bytes.extend_from_slice(&value.to_le_bytes());
        }
        write_all(&mut file, &table_bytes, path)?;

        let mut posting_bytes = Vec::with_capacity(flat.len() * POSTING_BYTES as usize);
        for (hash, posting) in &flat {
            posting_bytes.extend_from_slice(&hash.to_le_bytes());
            posting_bytes.extend_from_slice(&posting.track.to_le_bytes());
            posting_bytes.extend_from_slice(&posting.t1.to_le_bytes());
        }
        write_all(&mut file, &posting_bytes, path)?;

        let mut track_bytes = Vec::with_capacity(self.tracks.len() * TRACK_ENTRY_BYTES as usize);
        for (track, record) in &self.tracks {
            track_bytes.extend_from_slice(&track.to_le_bytes());
            track_bytes.extend_from_slice(record.as_bytes());
        }
        write_all(&mut file, &track_bytes, path)?;
        file.flush().map_err(|source| GenoTraceError::Unreadable {
            path: path.display().to_string(),
            source,
        })
    }
}

fn write_all(file: &mut File, bytes: &[u8], path: &Path) -> Result<(), GenoTraceError> {
    file.write_all(bytes)
        .map_err(|source| GenoTraceError::Unreadable {
            path: path.display().to_string(),
            source,
        })
}

impl FingerprintIndex for MemoryFingerprintIndex {
    fn postings_for(&self, hash: u32) -> Result<Vec<Posting>, GenoTraceError> {
        Ok(self
            .postings
            .get(&hash)
            .filter(|bucket| bucket.len() <= MAX_POSTINGS_PER_HASH)
            .cloned()
            .unwrap_or_default())
    }

    fn record_id(&self, track: u64) -> Option<RecordId> {
        self.tracks.get(&track).copied()
    }

    fn track_count(&self) -> usize {
        self.tracks.len()
    }
}

/// The filesystem index: two four-byte reads into the offset table give a bucket, and only that
/// bucket is read.
///
/// Not memory-mapped. `unsafe_code` is denied workspace-wide and every mapping API is unsafe;
/// reading the 64 MiB offset table into memory to avoid the syscalls would cost far more than the
/// handful of bucket reads a query actually makes.
#[derive(Debug)]
pub struct FileFingerprintIndex {
    file: std::sync::Mutex<File>,
    posting_count: u64,
    tracks: BTreeMap<u64, RecordId>,
}

impl FileFingerprintIndex {
    pub fn open(path: &Path) -> Result<Self, GenoTraceError> {
        let mut file = File::open(path).map_err(|source| GenoTraceError::Unreadable {
            path: path.display().to_string(),
            source,
        })?;
        let length = file
            .metadata()
            .map_err(|source| GenoTraceError::Unreadable {
                path: path.display().to_string(),
                source,
            })?
            .len();

        let mut header = [0u8; HEADER_BYTES as usize];
        read_exact_at(&mut file, 0, &mut header)?;
        if header.get(..8) != Some(&INDEX_MAGIC) {
            return Err(GenoTraceError::FingerprintIndexMalformed {
                reason: "magic does not identify a genotrace fingerprint index".to_string(),
            });
        }
        let scheme = u32::from_le_bytes(take4(&header, 8)?);
        if scheme != u32::from(crate::fingerprint::landmark::SCHEME_VERSION) {
            return Err(GenoTraceError::FingerprintIndexMalformed {
                reason: format!("index scheme version {scheme} is not readable by this build"),
            });
        }
        let posting_count = u64::from_le_bytes(take8(&header, 12)?);
        let track_table_offset = u64::from_le_bytes(take8(&header, 20)?);
        let track_table_len = u64::from_le_bytes(take8(&header, 28)?);

        let postings_end = HEADER_BYTES
            .checked_add(OFFSET_TABLE_BYTES)
            .and_then(|base| posting_count.checked_mul(POSTING_BYTES).map(|n| base + n))
            .ok_or_else(|| GenoTraceError::FingerprintIndexMalformed {
                reason: "posting count overflows the addressable file".to_string(),
            })?;
        if track_table_offset < postings_end {
            return Err(GenoTraceError::FingerprintIndexMalformed {
                reason: "track table overlaps the posting region".to_string(),
            });
        }
        let track_table_end = track_table_offset
            .checked_add(track_table_len)
            .ok_or_else(|| GenoTraceError::FingerprintIndexMalformed {
                reason: "track table length overflows".to_string(),
            })?;
        if track_table_end > length {
            return Err(GenoTraceError::FingerprintIndexMalformed {
                reason: "track table runs past the end of the file".to_string(),
            });
        }
        if track_table_len % TRACK_ENTRY_BYTES != 0 {
            return Err(GenoTraceError::FingerprintIndexMalformed {
                reason: "track table length is not a whole number of entries".to_string(),
            });
        }
        let track_count = (track_table_len / TRACK_ENTRY_BYTES) as usize;
        if track_count > MAX_TRACKS {
            return Err(GenoTraceError::FingerprintIndexMalformed {
                reason: format!(
                    "track table declares {track_count} tracks, over the {MAX_TRACKS} cap"
                ),
            });
        }

        let mut track_bytes = vec![0u8; track_table_len as usize];
        read_exact_at(&mut file, track_table_offset, &mut track_bytes)?;
        let mut tracks = BTreeMap::new();
        for entry in track_bytes.chunks_exact(TRACK_ENTRY_BYTES as usize) {
            let key = u64::from_le_bytes(take8(entry, 0)?);
            let mut digest = [0u8; 32];
            digest.copy_from_slice(entry.get(8..40).ok_or_else(|| {
                GenoTraceError::FingerprintIndexMalformed {
                    reason: "truncated track entry".to_string(),
                }
            })?);
            tracks.insert(
                key,
                RecordId::parse_hex(&hex::encode(digest)).map_err(|error| {
                    GenoTraceError::FingerprintIndexMalformed {
                        reason: error.to_string(),
                    }
                })?,
            );
        }

        Ok(Self {
            file: std::sync::Mutex::new(file),
            posting_count,
            tracks,
        })
    }
}

fn take4(bytes: &[u8], offset: usize) -> Result<[u8; 4], GenoTraceError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| GenoTraceError::FingerprintIndexMalformed {
            reason: "header is truncated".to_string(),
        })
}

fn take8(bytes: &[u8], offset: usize) -> Result<[u8; 8], GenoTraceError> {
    bytes
        .get(offset..offset + 8)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| GenoTraceError::FingerprintIndexMalformed {
            reason: "record is truncated".to_string(),
        })
}

fn read_exact_at(file: &mut File, offset: u64, buffer: &mut [u8]) -> Result<(), GenoTraceError> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|source| GenoTraceError::Unreadable {
            path: "<fingerprint index>".to_string(),
            source,
        })?;
    file.read_exact(buffer)
        .map_err(|source| GenoTraceError::Unreadable {
            path: "<fingerprint index>".to_string(),
            source,
        })
}

impl FingerprintIndex for FileFingerprintIndex {
    fn postings_for(&self, hash: u32) -> Result<Vec<Posting>, GenoTraceError> {
        let prefix = u64::from(crate::fingerprint::landmark::hash_prefix(hash));
        let Ok(mut file) = self.file.lock() else {
            return Err(GenoTraceError::FingerprintIndexMalformed {
                reason: "index handle was poisoned by a failed read".to_string(),
            });
        };

        let mut bounds = [0u8; 8];
        read_exact_at(&mut file, HEADER_BYTES + prefix * 4, &mut bounds)?;
        let start = u64::from(u32::from_le_bytes(take4(&bounds, 0)?));
        let end = u64::from(u32::from_le_bytes(take4(&bounds, 4)?));
        if end < start || end > self.posting_count {
            return Err(GenoTraceError::FingerprintIndexMalformed {
                reason: format!(
                    "bucket [{start}, {end}) is not inside {} postings",
                    self.posting_count
                ),
            });
        }
        let count = (end - start) as usize;
        if count == 0 {
            return Ok(Vec::new());
        }
        // A bucket over the build cap is not read at all. It is either a corrupt file or a hash so
        // common it carries no information; both answers are "no postings".
        if count > MAX_POSTINGS_PER_HASH {
            return Ok(Vec::new());
        }

        let mut bytes = vec![0u8; count * POSTING_BYTES as usize];
        read_exact_at(
            &mut file,
            HEADER_BYTES + OFFSET_TABLE_BYTES + start * POSTING_BYTES,
            &mut bytes,
        )?;
        let mut out = Vec::with_capacity(count);
        for entry in bytes.chunks_exact(POSTING_BYTES as usize) {
            let stored = u32::from_le_bytes(take4(entry, 0)?);
            if stored != hash {
                continue;
            }
            let track = u64::from_le_bytes(take8(entry, 4)?);
            if !self.tracks.contains_key(&track) {
                return Err(GenoTraceError::FingerprintIndexMalformed {
                    reason: format!("posting names track {track}, which the track table omits"),
                });
            }
            out.push(Posting {
                track,
                t1: u32::from_le_bytes(take4(entry, 12)?),
            });
        }
        Ok(out)
    }

    fn record_id(&self, track: u64) -> Option<RecordId> {
        self.tracks.get(&track).copied()
    }

    fn track_count(&self) -> usize {
        self.tracks.len()
    }
}
