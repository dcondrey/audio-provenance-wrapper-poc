use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apw_core::{
    append_jsonl, sha256_file_with_size, utc_timestamp, DEFAULT_EVIDENCE_BACKUPS,
    DEFAULT_EVIDENCE_MAX_BYTES,
};
use serde_json::{json, Map, Value};

use crate::error::{DaemonError, Result};
use crate::probe::AudioProbe;

pub const AUDIO_EXTENSIONS: [&str; 5] = ["wav", "aiff", "aif", "mp3", "m4a"];
pub const EXPORT_EXTENSIONS: [&str; 3] = ["wav", "aiff", "aif"];

pub const DEFAULT_SAMPLE_NOTES: [&str; 2] = [
    "Detected by filesystem watcher.",
    "No claim is made that this file was placed on a specific Ableton track.",
];

/// Size plus modification time. Equality is the whole stability test: a file
/// whose bytes are still arriving changes one or the other between polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSignature {
    pub size_bytes: u64,
    pub modified_nanos: i128,
}

impl FileSignature {
    pub fn of(path: &Path) -> Result<FileSignature> {
        let metadata = fs::metadata(path).map_err(|source| DaemonError::io("stat", path, source))?;
        Ok(FileSignature {
            size_bytes: metadata.len(),
            modified_nanos: metadata
                .modified()
                .ok()
                .map(system_time_nanos)
                .unwrap_or(0),
        })
    }
}

fn system_time_nanos(at: SystemTime) -> i128 {
    match at.duration_since(UNIX_EPOCH) {
        Ok(since) => since.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    }
}

fn has_extension(path: &Path, allowed: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .is_some_and(|extension| allowed.contains(&extension.as_str()))
}

fn lowercase_suffix(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
        .unwrap_or_default()
}

/// Directory listing in Python's `sorted(glob())` order. Growth is bounded by the
/// directory's own contents, not by any event rate, and every entry is re-checked
/// each scan so vanished files release their slot.
pub fn audio_files(watch_dir: &Path, recursive: bool, allowed: &[&str]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_files(watch_dir, recursive, allowed, &mut found);
    found.sort();
    found
}

fn collect_files(directory: &Path, recursive: bool, allowed: &[&str], found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if recursive {
                collect_files(&path, recursive, allowed, found);
            }
        } else if has_extension(&path, allowed) {
            found.push(path);
        }
    }
}

/// A `sample_file_observed` record for a file whose bytes did not move while it
/// was being read.
///
/// IMPORTANT: a `directly_observed` record must be internally consistent. Every
/// read of the file (hash, metadata, fingerprint) happens between two stats, and
/// a write landing anywhere in between voids the record rather than producing a
/// half-observed one.
pub fn build_sample_file_event(
    path: &Path,
    probe: &dyn AudioProbe,
    observed_at: Option<&str>,
    expected: Option<FileSignature>,
) -> Result<Value> {
    let resolved = fs::canonicalize(path).map_err(|source| DaemonError::io("resolve", path, source))?;
    let metadata =
        fs::metadata(&resolved).map_err(|source| DaemonError::io("stat", &resolved, source))?;
    let signature = FileSignature {
        size_bytes: metadata.len(),
        modified_nanos: metadata.modified().ok().map(system_time_nanos).unwrap_or(0),
    };
    if expected.is_some_and(|expected| expected != signature) {
        return Err(DaemonError::io(
            "record",
            &resolved,
            std::io::Error::other("file changed after its stability check; evidence not recorded"),
        ));
    }
    let created = metadata.created().or_else(|_| metadata.modified()).ok();

    let (sha256, hashed_bytes) = sha256_file_with_size(&resolved)?;
    let audio_metadata = probe.metadata(&resolved);
    let audio_fingerprint = probe.fingerprint(&resolved);

    let post = fs::metadata(&resolved).map_err(|source| DaemonError::io("stat", &resolved, source))?;
    let post_signature = FileSignature {
        size_bytes: post.len(),
        modified_nanos: post.modified().ok().map(system_time_nanos).unwrap_or(0),
    };
    if hashed_bytes != signature.size_bytes || post_signature != signature {
        return Err(DaemonError::io(
            "record",
            &resolved,
            std::io::Error::other("file changed while being read; evidence not recorded"),
        ));
    }

    let suffix = lowercase_suffix(&resolved);
    let mut event = Map::new();
    event.insert("event_type".to_owned(), json!("sample_file_observed"));
    event.insert(
        "proof_level".to_owned(),
        json!(apw_core::ProofLevel::DirectlyObserved.as_str()),
    );
    event.insert(
        "file_name".to_owned(),
        json!(resolved
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()),
    );
    event.insert("file_path".to_owned(), json!(resolved.to_string_lossy()));
    event.insert("sha256".to_owned(), json!(sha256));
    event.insert(
        "format".to_owned(),
        json!(suffix.trim_start_matches('.').to_owned()),
    );
    event.insert("file_extension".to_owned(), json!(suffix));
    event.insert("file_size_bytes".to_owned(), json!(hashed_bytes));
    event.insert("created_at".to_owned(), json!(utc_timestamp(created)));
    event.insert(
        "modified_at".to_owned(),
        json!(utc_timestamp(metadata.modified().ok())),
    );
    event.insert(
        "observed_at".to_owned(),
        json!(observed_at
            .map(str::to_owned)
            .unwrap_or_else(|| utc_timestamp(None))),
    );
    event.insert("audio_metadata".to_owned(), audio_metadata.to_json());
    event.insert("audio_fingerprint".to_owned(), audio_fingerprint.to_json());
    event.insert("notes".to_owned(), json!(DEFAULT_SAMPLE_NOTES));
    Ok(Value::Object(event))
}

/// Polling sample-import watcher. A file is hashed only after its signature has
/// repeated `stable_polls` times, and the hash is then taken under the
/// re-stat guard in [`build_sample_file_event`].
pub struct SampleWatcher {
    watch_dir: PathBuf,
    evidence_path: PathBuf,
    poll_interval: Duration,
    stable_polls: u32,
    recursive: bool,
    seen: HashMap<PathBuf, FileSignature>,
    pending: HashMap<PathBuf, (FileSignature, u32)>,
    read_failed: HashSet<PathBuf>,
    evidence_max_bytes: u64,
    evidence_backups: u32,
}

impl SampleWatcher {
    pub fn new(
        watch_dir: &Path,
        evidence_path: &Path,
        poll_interval: Duration,
        stable_polls: u32,
        recursive: bool,
    ) -> SampleWatcher {
        SampleWatcher {
            watch_dir: watch_dir.to_path_buf(),
            evidence_path: evidence_path.to_path_buf(),
            poll_interval,
            stable_polls: stable_polls.max(1),
            recursive,
            seen: HashMap::new(),
            pending: HashMap::new(),
            read_failed: HashSet::new(),
            evidence_max_bytes: DEFAULT_EVIDENCE_MAX_BYTES,
            evidence_backups: DEFAULT_EVIDENCE_BACKUPS,
        }
    }

    pub fn watch_dir(&self) -> &Path {
        &self.watch_dir
    }

    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    /// Records the files already present so a fresh session does not re-import
    /// the whole folder as newly observed evidence.
    pub fn mark_existing_seen(&mut self) -> Result<()> {
        self.ensure_dir()?;
        for path in audio_files(&self.watch_dir, self.recursive, &AUDIO_EXTENSIONS) {
            if let (Ok(resolved), Ok(signature)) = (fs::canonicalize(&path), FileSignature::of(&path))
            {
                self.seen.insert(resolved, signature);
            }
        }
        Ok(())
    }

    pub fn scan_once(&mut self, probe: &dyn AudioProbe) -> Result<Vec<Value>> {
        self.ensure_dir()?;
        let mut observed = Vec::new();

        let current: Vec<(PathBuf, PathBuf)> =
            audio_files(&self.watch_dir, self.recursive, &AUDIO_EXTENSIONS)
                .into_iter()
                .filter_map(|path| fs::canonicalize(&path).ok().map(|resolved| (path, resolved)))
                .collect();
        let live: HashSet<PathBuf> = current.iter().map(|(_, resolved)| resolved.clone()).collect();
        self.seen.retain(|key, _| live.contains(key));
        self.pending.retain(|key, _| live.contains(key));
        self.read_failed.retain(|key| live.contains(key));

        for (path, resolved) in current {
            let Ok(signature) = FileSignature::of(&path) else {
                continue;
            };
            if self.seen.get(&resolved) == Some(&signature) {
                continue;
            }

            let (previous, count) = self
                .pending
                .get(&resolved)
                .copied()
                .unwrap_or((signature, 0));
            let stable_count = if previous == signature { count + 1 } else { 1 };
            self.pending.insert(resolved.clone(), (signature, stable_count));
            if stable_count < self.stable_polls {
                continue;
            }

            match build_sample_file_event(&path, probe, None, Some(signature)) {
                Ok(event) => {
                    self.read_failed.remove(&resolved);
                    append_jsonl(
                        &self.evidence_path,
                        &event,
                        self.evidence_max_bytes,
                        self.evidence_backups,
                    )?;
                    observed.push(event);
                    self.seen.insert(resolved.clone(), signature);
                    self.pending.remove(&resolved);
                }
                Err(error) => {
                    // The unchanged signature re-satisfies the stable check every
                    // poll, so this retries forever; say so once per episode
                    // instead of dropping the file's evidence in silence.
                    if self.read_failed.insert(resolved.clone()) {
                        log::warn!(
                            "Sample file stat is stable but its content is unreadable; \
                             evidence not emitted, will keep retrying: {} ({error})",
                            path.display()
                        );
                    }
                }
            }
        }
        Ok(observed)
    }

    fn ensure_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.watch_dir)
            .map_err(|source| DaemonError::io("create", &self.watch_dir, source))
    }
}

/// How long an export must hold still before it is treated as finished.
#[derive(Debug, Clone, Copy)]
pub struct StabilityPolicy {
    pub checks: u32,
    pub interval: Duration,
}

impl Default for StabilityPolicy {
    fn default() -> Self {
        StabilityPolicy {
            checks: 3,
            interval: Duration::from_millis(500),
        }
    }
}

/// The wait between two stability samples. Injectable so the debounce can be
/// driven deterministically in a test instead of by wall-clock sleeping.
pub trait Delay: Send + Sync {
    fn wait(&self, duration: Duration);
}

pub struct ThreadSleep;

impl Delay for ThreadSleep {
    fn wait(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedExport {
    pub path: PathBuf,
    pub resolved: PathBuf,
    pub version: u32,
}

/// Polling export watcher.
///
/// IMPORTANT: a partially written export must never be hashed. A candidate is
/// returned only after its size and mtime are unchanged across every stability
/// sample AND its size is non-zero; the caller hashes and seals only what
/// [`ExportWatcher::detect`] hands back, and commits the result with
/// [`ExportWatcher::commit`] so a failed sealing attempt retries on the next poll.
pub struct ExportWatcher {
    export_dir: PathBuf,
    seen: HashMap<PathBuf, FileSignature>,
    versions: HashMap<PathBuf, u32>,
    stability: StabilityPolicy,
}

impl ExportWatcher {
    pub fn new(export_dir: &Path, stability: StabilityPolicy) -> ExportWatcher {
        ExportWatcher {
            export_dir: export_dir.to_path_buf(),
            seen: HashMap::new(),
            versions: HashMap::new(),
            stability,
        }
    }

    pub fn export_dir(&self) -> &Path {
        &self.export_dir
    }

    pub fn mark_existing_seen(&mut self) {
        for path in audio_files(&self.export_dir, false, &EXPORT_EXTENSIONS) {
            if let (Ok(resolved), Ok(signature)) = (fs::canonicalize(&path), FileSignature::of(&path))
            {
                self.seen.insert(resolved, signature);
            }
        }
    }

    pub fn detect(&mut self, delay: &dyn Delay) -> Vec<DetectedExport> {
        let mut detected = Vec::new();
        for path in audio_files(&self.export_dir, false, &EXPORT_EXTENSIONS) {
            let Ok(resolved) = fs::canonicalize(&path) else {
                continue;
            };
            let Ok(signature) = FileSignature::of(&path) else {
                continue;
            };
            if self.seen.get(&resolved) == Some(&signature) {
                continue;
            }
            if !self.is_stable(&path, delay) {
                continue;
            }
            let version = self.versions.get(&resolved).copied().unwrap_or(0) + 1;
            detected.push(DetectedExport {
                path,
                resolved,
                version,
            });
        }
        detected
    }

    /// Records a successfully sealed export. Called only after the manifest is
    /// written, so a failure leaves the export detectable on the next poll.
    pub fn commit(&mut self, export: &DetectedExport) {
        self.versions.insert(export.resolved.clone(), export.version);
        if let Ok(signature) = FileSignature::of(&export.path) {
            self.seen.insert(export.resolved.clone(), signature);
        }
    }

    fn is_stable(&self, path: &Path, delay: &dyn Delay) -> bool {
        let Ok(mut previous) = FileSignature::of(path) else {
            return false;
        };
        for _ in 0..self.stability.checks {
            delay.wait(self.stability.interval);
            let Ok(current) = FileSignature::of(path) else {
                return false;
            };
            if current != previous {
                return false;
            }
            previous = current;
        }
        previous.size_bytes > 0
    }
}
