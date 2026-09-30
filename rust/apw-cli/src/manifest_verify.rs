//! The local manifest verifier: a port of `daemon/verify.py::verify_manifest`.
//!
//! Finding codes, severities and messages are the Python verifier's. Unlike the
//! asset verifier in `verify_state`, this grades one manifest document and the
//! files it binds; `verified` needs every check in [`apw_core::REQUIRED_CHECKS`]
//! to have run, so a manifest whose export or evidence is absent grades
//! `incomplete` instead of borrowing the strongest word.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};

use apw_core::{
    canonical_json, check_time_anchor, evaluate_ots_record, is_truthy,
    path_name, python_eq, python_from_hex, python_int, python_repr, python_str, rotated_evidence_paths, sha256_file,
    sha256_prefix, validate_manifest_invariants, verify_pinned_signature, without_top_level_keys,
    Canonicalization, HeaderSource, VerificationReport, VerificationState,
    LOCAL_SIGNATURE_EXCLUDED_KEYS, PORTABLE_SIGNATURE_EXCLUDED_KEYS,
};
use apw_daemon::project::safe::{parse_json, Limits};
use apw_daemon::HashChainAnalyzer;
use apw_provenance::{expand_user, HardwareProvider, SoftwareProvider};
use serde_json::{json, Map, Value};

const LOCAL_SEAL_ALGORITHM: &str = "hmac-sha256-local";

/// A crafted evidence file with no newline would otherwise be read to EOF in one
/// allocation.
const MAX_EVIDENCE_LINE_BYTES: u64 = 1 << 20;

/// Keys where Python's `is None` also matches a JSON null.
fn some<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(key).filter(|value| !value.is_null())
}

fn field<'a>(map: &'a Map<String, Value>, key: &str) -> &'a Value {
    map.get(key).unwrap_or(&Value::Null)
}

fn truthy(map: &Map<String, Value>, key: &str) -> bool {
    map.get(key).is_some_and(is_truthy)
}

fn text_of(value: Option<&Value>) -> String {
    value.map(python_str).unwrap_or_default()
}

fn object_of(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

/// Python's `isinstance(value, int)`, which is true for booleans.
fn int_like(value: &Value) -> Option<i128> {
    match value {
        Value::Bool(flag) => Some(i128::from(*flag)),
        other => python_int(other),
    }
}

/// `int(value)` over decoded JSON.
fn coerce_int(value: &Value) -> Option<i128> {
    match value {
        Value::String(text) => text.trim().parse().ok(),
        Value::Number(number) if python_int(value).is_none() => {
            let float = number.as_f64().filter(|float| float.is_finite())?;
            Some(float.trunc() as i128)
        }
        other => int_like(other),
    }
}

/// `Path(str(value)).expanduser()`. A `~user` form names a home this process
/// cannot resolve, which Python also treats as unresolvable.
fn safe_path(value: &str) -> Option<PathBuf> {
    match value.strip_prefix('~') {
        Some(rest) if !rest.is_empty() && !rest.starts_with('/') => None,
        _ => Some(expand_user(Path::new(value))),
    }
}

/// Resolve one bound evidence file inside its directory, or `None`.
///
/// IMPORTANT: the names come from the untrusted manifest and are hashed before any
/// signature on that manifest is checked, so an unconstrained join is an
/// attacker-directed filesystem read plus a per-path hash oracle.
fn evidence_member(evidence_dir: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return None;
    }
    let candidate = evidence_dir.join(name);
    if path_name(&candidate) != name {
        return None;
    }
    if candidate.exists() {
        let root = evidence_dir.canonicalize().ok()?;
        if candidate.canonicalize().ok()?.parent() != Some(root.as_path()) {
            return None;
        }
    }
    Some(candidate)
}

/// Resolve a manifest-relative artifact without leaving the session tree.
///
/// IMPORTANT: sidecar mode legitimately records `../exports/take.aiff`, so the
/// manifest directory alone is too tight a fence; its parent is the session root
/// and is the widest an untrusted manifest may steer a read.
fn contained_artifact(manifest_path: &Path, relative: &Value) -> Option<PathBuf> {
    let value = python_str(relative);
    let relative_path = Path::new(&value);
    if value.is_empty() || relative_path.is_absolute() {
        return None;
    }
    let parts: Vec<Component> = relative_path.components().filter(|part| *part != Component::CurDir).collect();
    let parents = parts.iter().filter(|part| **part == Component::ParentDir).count();
    if parents > 1 || parts.iter().skip(1).any(|part| *part == Component::ParentDir) {
        return None;
    }
    let candidate = manifest_path.parent().unwrap_or(Path::new("")).join(relative_path);
    let resolved = candidate.canonicalize().ok()?;
    let root = manifest_path.canonicalize().ok()?.parent()?.parent()?.to_path_buf();
    resolved.starts_with(root).then_some(candidate)
}

/// Base64 bodies of the certificates in a PEM blob, order- and wrap-insensitive.
fn certificate_bodies(pem: &str) -> Vec<String> {
    let mut bodies = Vec::new();
    let mut current: Option<String> = None;
    for line in pem.lines() {
        match line.trim() {
            "-----BEGIN CERTIFICATE-----" => current = Some(String::new()),
            "-----END CERTIFICATE-----" => {
                if let Some(body) = current.take().filter(|body| !body.is_empty()) {
                    bodies.push(body);
                }
            }
            other => {
                if let Some(body) = current.as_mut() {
                    body.push_str(other);
                }
            }
        }
    }
    bodies.sort();
    bodies
}

fn load_trust_anchors(path: Option<&Path>) -> Option<String> {
    let bytes = std::fs::read(expand_user(path?)).ok()?;
    let pem = String::from_utf8(bytes).ok().filter(|pem| pem.is_ascii())?;
    pem.contains("BEGIN CERTIFICATE").then_some(pem)
}

/// Whole JSONL records from the first `byte_length` bytes, line-bounded.
fn bounded_jsonl_lines(path: &Path, byte_length: u64, mut visit: impl FnMut(&[u8])) -> std::io::Result<()> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut consumed = 0_u64;
    let mut line = Vec::new();
    while consumed < byte_length {
        line.clear();
        let read = reader.by_ref().take(MAX_EVIDENCE_LINE_BYTES).read_until(b'\n', &mut line)? as u64;
        if read == 0 {
            return Ok(());
        }
        consumed += read;
        if consumed > byte_length {
            return Ok(());
        }
        if line.ends_with(b"\n") || read < MAX_EVIDENCE_LINE_BYTES {
            visit(&line);
            continue;
        }
        // An overlong record: discard the rest of it rather than buffering it.
        loop {
            if consumed >= byte_length {
                return Ok(());
            }
            line.clear();
            let extra = reader.by_ref().take(MAX_EVIDENCE_LINE_BYTES).read_until(b'\n', &mut line)? as u64;
            if extra == 0 {
                return Ok(());
            }
            consumed += extra;
            if line.ends_with(b"\n") {
                break;
            }
        }
    }
    Ok(())
}

/// Everything `verify_manifest` takes besides the manifest path.
#[derive(Default)]
pub struct ManifestVerifyOptions {
    /// Local HMAC key; `None` is `--public-only`.
    pub signing_key: Option<PathBuf>,
    pub public_key: PathBuf,
    pub export: Option<PathBuf>,
    pub trust_anchor: Option<PathBuf>,
    pub c2pa_asset: Option<PathBuf>,
    pub ots_header_source: Option<Box<dyn HeaderSource>>,
    pub ots_proof: Option<Vec<u8>>,
}

struct Verifier<'a> {
    manifest_path: &'a Path,
    options: &'a ManifestVerifyOptions,
    report: VerificationReport,
}

pub fn verify_manifest(manifest_path: &Path, options: &ManifestVerifyOptions) -> VerificationReport {
    let mut verifier = Verifier { manifest_path, options, report: VerificationReport::new() };
    verifier.run();
    verifier.report
}

impl Verifier<'_> {
    fn run(&mut self) {
        if !self.manifest_path.is_file() {
            self.report.error("not_found", format!("Manifest not found: {}", self.manifest_path.display()));
            return;
        }
        let parsed = std::fs::read(self.manifest_path)
            .map_err(|error| error.to_string())
            // Python decodes with utf-8, not utf-8-sig, so a byte order mark is a parse error.
            .and_then(|bytes| if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) { Err("Unexpected UTF-8 BOM".to_owned()) } else { Ok(bytes) })
            .and_then(|bytes| parse_json(&bytes, &Limits::default()));
        let data = match parsed {
            Ok(data) => data,
            Err(error) => {
                self.report.error("read_failed", format!("Cannot read manifest: {error}"));
                return;
            }
        };
        for error in validate_manifest_invariants(&data) {
            self.report.error("schema_invalid", error);
        }
        let Some(manifest) = data.as_object() else {
            return;
        };

        for (key, code, message) in [
            ("apw_version", "missing_version", "Missing apw_version field"),
            ("c2pa_mapping", "missing_c2pa", "Missing c2pa_mapping field"),
            ("apw:unobserved", "missing_unobserved", "Missing apw:unobserved field (honesty model violation)"),
        ] {
            if !manifest.contains_key(key) {
                self.report.error(code, message);
            }
        }

        self.check_export(manifest);
        let stems: &[Value] = manifest.get("observed_stems").and_then(Value::as_array).map_or(&[], Vec::as_slice);
        self.check_stems(stems);
        self.check_assertions(manifest);
        self.check_evidence_binding(manifest, stems);
        self.check_seal(manifest);
        self.check_portable_signature(manifest);
        self.check_c2pa_claim(manifest);

        check_time_anchor(&data, &mut self.report);
        let source = self.options.ots_header_source.as_deref();
        for (severity, code, message) in evaluate_ots_record(&data, source, self.options.ots_proof.as_deref()) {
            match severity.as_str() {
                "error" => self.report.error(&code, message),
                "warning" => self.report.warn(&code, message),
                _ => self.report.info(&code, message),
            }
        }

        self.check_session_facts(manifest);
        self.check_presentation(manifest);
    }

    fn check_export(&mut self, manifest: &Map<String, Value>) {
        let Some(export) = some(manifest, "export") else {
            self.report.error("no_export", "No export evidence in manifest");
            return;
        };
        let Some(export) = export.as_object() else {
            self.report.error("schema_invalid", "export must be an object");
            return;
        };
        if !truthy(export, "sha256") {
            self.report.error("export_no_hash", "Export missing sha256 hash");
        }
        if !truthy(export, "file_name") {
            self.report.error("export_no_name", "Export missing file_name");
        }
        let path_value = match &self.options.export {
            Some(path) => path.to_string_lossy().into_owned(),
            None if truthy(export, "file_path") => python_str(field(export, "file_path")),
            None => return,
        };
        match safe_path(&path_value).filter(|path| path.is_file()) {
            Some(path) => match sha256_file(&path) {
                Err(_) => self.report.warn("export_file_unavailable", "Export file could not be read for hash verification"),
                Ok(actual) => {
                    self.report.complete("export_binding");
                    if python_eq(&Value::String(actual), field(export, "sha256")) {
                        self.report.info("export_hash_valid", "Export file SHA-256 matches manifest");
                    } else {
                        self.report.error("export_hash_mismatch", "Export file SHA-256 does not match manifest");
                        self.note_supersession();
                    }
                }
            },
            None => self.report.warn(
                "export_file_unavailable",
                format!(
                    "Export file is unavailable for hash verification: {path_value}. \
                     Pass --export <file> to verify the manifest against a copy you hold."
                ),
            ),
        }
    }

    /// Explain an export-hash mismatch caused by a re-export, not a tamper: the daemon
    /// versions the manifest but the DAW overwrites the audio in place.
    fn note_supersession(&mut self) {
        let stem = self.manifest_path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let marker = self.manifest_path.with_file_name(format!("{stem}.superseded.json"));
        let Some(record) = std::fs::read_to_string(marker).ok().and_then(|text| parse_json(text.as_bytes(), &Limits::default()).ok()) else {
            return;
        };
        if let Some(successor) = record.get("superseded_by").filter(|value| is_truthy(value)) {
            self.report.info(
                "manifest_superseded",
                format!(
                    "The export was re-rendered under the same file name after this manifest was \
                     sealed; the current bytes are described by {}",
                    python_str(successor)
                ),
            );
        }
    }

    fn check_stems(&mut self, stems: &[Value]) {
        if stems.is_empty() {
            self.report.info(
                "no_routed_evidence",
                "No routed-audio evidence was received; export integrity can still be checked, but association is unobserved",
            );
        }
        for (index, stem) in stems.iter().enumerate() {
            let Some(stem) = stem.as_object() else { continue };
            if !truthy(stem, "hash_chain_root") {
                self.report.error("stem_no_root", format!("Stem {index} missing hash_chain_root"));
            }
            if python_eq(stem.get("hash_chain_length").unwrap_or(&json!(0)), &json!(0)) {
                self.report.error("stem_empty_chain", format!("Stem {index} has zero-length hash chain"));
            }
            if !truthy(stem, "source_category_proof_level") {
                self.report.warn("source_proof_missing", format!("Stem {index} source category has no proof level"));
            }
        }
    }

    fn check_assertions(&mut self, manifest: &Map<String, Value>) {
        let has_unobserved = object_of(manifest.get("c2pa_mapping"))
            .and_then(|mapping| mapping.get("assertions"))
            .and_then(Value::as_array)
            .is_some_and(|assertions| {
                assertions
                    .iter()
                    .filter_map(Value::as_object)
                    .any(|assertion| python_eq(field(assertion, "label"), &json!("apw.unobserved")))
            });
        if !has_unobserved {
            self.report.error("c2pa_no_unobserved", "C2PA assertions missing apw.unobserved declaration");
        }
    }

    fn check_evidence_binding(&mut self, manifest: &Map<String, Value>, stems: &[Value]) {
        let Some(binding) = manifest.get("evidence_binding") else {
            self.report.warn("no_evidence_binding", "No evidence_binding (cannot trace manifest to evidence files)");
            return;
        };
        let Some(binding) = binding.as_object() else {
            self.report.error("schema_invalid", "evidence_binding must be an object");
            return;
        };
        if !truthy(binding, "evidence_file_hashes") {
            self.report.warn("empty_evidence_hashes", "Evidence binding has no file hashes");
        }
        if python_eq(binding.get("chain_length").unwrap_or(&json!(0)), &json!(0)) {
            self.report.warn("binding_no_chain", "Evidence binding reports zero chain length");
        }
        let last_window_hash = field(binding, "last_window_hash");
        if is_truthy(last_window_hash) {
            for (index, stem) in stems.iter().enumerate() {
                let Some(stem) = stem.as_object() else { continue };
                if !python_eq(field(stem, "hash_chain_root"), last_window_hash) {
                    self.report.error(
                        "stem_commitment_mismatch",
                        format!("Stem {index} chain commitment does not match the last bound window hash"),
                    );
                }
            }
        }

        let directory_value = field(binding, "evidence_directory");
        let directory = is_truthy(directory_value).then(|| safe_path(&python_str(directory_value))).flatten();
        if is_truthy(directory_value) && directory.is_none() {
            self.report.warn("path_unresolvable", "Evidence directory path cannot be resolved");
        }
        let Some(directory) = directory else { return };
        let evidence_files = object_of(binding.get("evidence_files")).filter(|files| !files.is_empty());
        if let Some(files) = evidence_files {
            let mut verified = Vec::new();
            for (name, file_binding) in files {
                self.check_evidence_file(&directory, name, file_binding, &mut verified);
            }
            self.check_coverage(manifest, binding, &verified);
        } else if let Some(hashes) = object_of(binding.get("evidence_file_hashes")) {
            for (name, expected) in hashes {
                self.check_evidence_hash(&directory, name, expected);
            }
        }
    }

    fn check_evidence_file(&mut self, directory: &Path, name: &str, file_binding: &Value, verified: &mut Vec<(PathBuf, u64)>) {
        let Some(evidence_path) = evidence_member(directory, name) else {
            self.report.error(
                "evidence_binding_invalid",
                format!("Evidence binding names a file outside its bound directory: {name}"),
            );
            return;
        };
        let Some(file_binding) = file_binding.as_object() else {
            self.report.error("evidence_binding_invalid", format!("Invalid evidence binding: {name}"));
            return;
        };
        let Some(byte_length) = file_binding.get("byte_length").map_or(Some(0), coerce_int) else {
            self.report.error(
                "evidence_binding_invalid",
                format!("Invalid evidence binding: {name} (byte_length is not an integer)"),
            );
            return;
        };
        if byte_length < 0 {
            self.report.error(
                "evidence_binding_invalid",
                format!("Invalid evidence binding: {name} (byte_length is negative)"),
            );
            return;
        }
        let byte_length = u64::try_from(byte_length).unwrap_or(u64::MAX);
        let expected = field(file_binding, "sha256");

        let evidence_stem = evidence_path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let suffix = evidence_path.extension().map(|ext| format!(".{}", ext.to_string_lossy())).unwrap_or_default();
        let rotation_stem = match evidence_stem.rsplit_once('.') {
            Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_digit()) => head.to_owned(),
            _ => evidence_stem,
        };
        let active = directory.join(format!("{rotation_stem}{suffix}"));
        let mut candidates = vec![evidence_path.clone()];
        let mut rotations: Vec<PathBuf> = rotated_evidence_paths(&active).unwrap_or_default();
        rotations.retain(|path| *path != active);
        candidates.extend(rotations.into_iter().rev());

        let mut matched: Option<PathBuf> = None;
        let mut sizes = Vec::new();
        for candidate in &candidates {
            if !candidate.is_file() {
                continue;
            }
            let Ok(size) = std::fs::metadata(candidate).map(|meta| meta.len()) else { continue };
            sizes.push(size);
            if matched.is_some() || size < byte_length {
                continue;
            }
            if sha256_prefix(candidate, byte_length).is_ok_and(|actual| python_eq(&Value::String(actual), expected)) {
                matched = Some(candidate.clone());
            }
        }
        if let Some(path) = matched {
            let suffix = if path == evidence_path { String::new() } else { format!(" (now in rotation {})", path_name(&path)) };
            self.report.info("evidence_hash_valid", format!("Evidence prefix hash matches: {name}{suffix}"));
            verified.push((path, byte_length));
            self.report.complete("evidence_binding");
        } else if sizes.is_empty() {
            self.report.warn("evidence_file_unavailable", format!("Evidence file unavailable: {name}"));
        } else if sizes.iter().all(|size| *size < byte_length) {
            self.report.error("evidence_truncated", format!("Evidence prefix is unavailable or truncated: {name}"));
        } else {
            self.report.error("evidence_hash_mismatch", format!("Evidence file changed: {name}"));
        }
    }

    fn check_evidence_hash(&mut self, directory: &Path, name: &str, expected: &Value) {
        let Some(path) = evidence_member(directory, name) else {
            self.report.error(
                "evidence_binding_invalid",
                format!("Evidence binding names a file outside its bound directory: {name}"),
            );
            return;
        };
        if !path.is_file() {
            self.report.warn("evidence_file_unavailable", format!("Evidence file unavailable: {name}"));
            return;
        }
        let Ok(actual) = sha256_file(&path) else {
            self.report.warn("evidence_file_unavailable", format!("Evidence file unreadable: {name}"));
            return;
        };
        if python_eq(&Value::String(actual), expected) {
            self.report.info("evidence_hash_valid", format!("Evidence file hash matches: {name}"));
            self.report.complete("evidence_binding");
        } else {
            self.report.error("evidence_hash_mismatch", format!("Evidence file changed: {name}"));
        }
    }

    /// Re-derive the buffer_hash counters from the verified bound prefixes and compare
    /// them with the manifest's own numbers. Without this, a genuinely signed manifest
    /// whose counters diverged from its bound evidence would still certify.
    fn check_coverage(&mut self, manifest: &Map<String, Value>, binding: &Map<String, Value>, verified: &[(PathBuf, u64)]) {
        // Nothing bound to check against: the hash checks already ran. An empty list is
        // not zero events in a bound file, which must still catch a chain claimed over
        // evidence with no windows.
        if verified.is_empty() {
            return;
        }
        let session_id = field(manifest, "session_id");
        let mut streams: HashMap<(String, String), usize> = HashMap::new();
        let mut analyzers: Vec<HashChainAnalyzer> = Vec::new();
        let mut recomputed = 0_i128;
        let mut window_hashes: HashSet<String> = HashSet::new();
        for (path, byte_length) in verified {
            let _ = bounded_jsonl_lines(path, *byte_length, |raw| {
                let trimmed = raw.trim_ascii();
                let Ok(event) = serde_json::from_slice::<Value>(trimmed) else { return };
                let Some(fields) = event.as_object() else { return };
                if !python_eq(field(fields, "event_type"), &json!("buffer_hash")) {
                    return;
                }
                let event_session = field(fields, "capture_session_id");
                if is_truthy(session_id) && is_truthy(event_session) && !python_eq(event_session, session_id) {
                    return;
                }
                let stream = (
                    fields.get("plugin_instance_id").map(python_str).unwrap_or_default(),
                    fields.get("plugin_capture_session_id").map(python_str).unwrap_or_default(),
                );
                let slot = *streams.entry(stream).or_insert_with(|| {
                    analyzers.push(HashChainAnalyzer::new());
                    analyzers.len() - 1
                });
                if let Some(analyzer) = analyzers.get_mut(slot) {
                    analyzer.ingest_buffer_hash(&event);
                }
                recomputed += 1;
                if let Some(hash) = fields.get("window_hash").and_then(Value::as_str) {
                    window_hashes.insert(hash.to_owned());
                }
            });
        }

        for analyzer in &analyzers {
            for flag in analyzer.analyze().flags {
                let message = format!("{} ({})", flag.description, flag.evidence);
                let code = format!("evidence_{}", flag.name);
                if flag.severity >= 0.8 {
                    self.report.error(&code, message);
                } else {
                    self.report.warn(&code, message);
                }
            }
        }

        let claimed = object_of(manifest.get("observation_coverage"))
            .and_then(|coverage| object_of(coverage.get("counters")))
            .and_then(|counters| counters.get("buffer_hash_events_received"));
        let claimed_int = claimed.and_then(int_like);
        let bound_length = binding.get("chain_length").and_then(int_like);
        let mut mismatched = false;
        // Over-claiming is the fraud: asserting more coverage than the bound evidence
        // supports. Under-claiming (events landed after the counter was snapshotted) is
        // conservative and honest, so it is noted, not failed.
        if let (Some(claimed), Some(value)) = (claimed_int, claimed) {
            if claimed > recomputed {
                mismatched = true;
                self.report.error(
                    "coverage_counters_mismatch",
                    format!("Manifest reports {} buffer_hash events; bound evidence contains only {recomputed}", python_str(value)),
                );
            }
        }
        if let (Some(length), Some(value)) = (bound_length, binding.get("chain_length")) {
            if length > recomputed {
                mismatched = true;
                self.report.error(
                    "coverage_counters_mismatch",
                    format!("Evidence binding reports chain_length {}; bound evidence contains only {recomputed}", python_str(value)),
                );
            }
        }
        // The committed chain root must be one of the windows in the bound evidence, not
        // necessarily the last: later windows may have been appended.
        if let Some(hash) = binding.get("last_window_hash").and_then(Value::as_str).filter(|hash| !hash.is_empty()) {
            if !window_hashes.contains(hash) {
                mismatched = true;
                self.report.error("evidence_commitment_mismatch", "The bound last_window_hash does not appear in the bound evidence");
            }
        }
        if !mismatched {
            let detail = match (claimed_int, claimed) {
                (Some(count), Some(value)) if count != recomputed => {
                    format!("Manifest conservatively reports {} of {recomputed} bound buffer_hash events", python_str(value))
                }
                _ => format!("Recomputed {recomputed} buffer_hash events from bound evidence; counters agree"),
            };
            self.report.info("coverage_counters_rederived", detail);
        }
    }

    fn check_seal(&mut self, manifest: &Map<String, Value>) {
        let Some(seal) = some(manifest, "manifest_signature") else {
            self.report.warn("unsigned", "Manifest is unsigned (no manifest_signature)");
            return;
        };
        let Some(seal) = seal.as_object() else {
            self.report.error("schema_invalid", "manifest_signature must be an object");
            return;
        };
        if !truthy(seal, "signed_content_hash") {
            // A present-but-hollow seal used to be quieter than a deleted one, so a
            // forger's best move was to keep the key and empty it.
            self.report.error(
                "signature_incomplete",
                "manifest_signature carries no signed_content_hash, so nothing binds it to this manifest",
            );
            return;
        }
        let sealed = without_top_level_keys(&Value::Object(manifest.clone()), &LOCAL_SIGNATURE_EXCLUDED_KEYS);
        let signed_bytes = canonical_json(&sealed, Canonicalization::AsciiEscaped).unwrap_or_default();
        let recomputed = apw_core::sha256_hex(&signed_bytes);
        if python_eq(&Value::String(recomputed), field(seal, "signed_content_hash")) {
            self.report.info("signed_content_hash_valid", "Manifest content matches its signed-content hash");
        } else {
            self.report.error("tampered", "Manifest content hash mismatch (manifest may have been tampered with)");
        }
        // IMPORTANT: keyed off the signature algorithm, never off trust_scope.
        // trust_scope is an unconstrained string inside the sealed document, so a
        // forger who edits it picks whether their own seal gets checked.
        if text_of(seal.get("algorithm")) != LOCAL_SEAL_ALGORITHM {
            self.report.error(
                "signature_algorithm_unrecognised",
                format!(
                    "manifest_signature declares algorithm {}, which this verifier cannot check; \
                     an unverifiable seal is not an accepted one",
                    seal.get("algorithm").map_or_else(|| "None".to_owned(), python_repr)
                ),
            );
        } else if let Some(key) = &self.options.signing_key {
            self.check_local_seal(seal, &signed_bytes, &expand_user(key));
        } else {
            self.report.warn(
                "local_signing_key_not_supplied",
                "The local HMAC key was deliberately not supplied (--public-only), so the seal \
                 bytes were not verified; the portable Ed25519 signature still covers this manifest",
            );
        }
        self.report.warn("local_integrity_only", "Local HMAC integrity is not hardware attestation or external identity");
    }

    fn check_local_seal(&mut self, seal: &Map<String, Value>, signed_bytes: &[u8], key: &Path) {
        if !key.is_file() {
            self.report.warn(
                "local_signing_key_unavailable",
                format!("Local HMAC key is unavailable at {}; signature bytes were not verified", key.display()),
            );
            return;
        }
        if !truthy(seal, "signature_hex") {
            self.report.error("signature_missing", "Local integrity seal has no signature bytes");
            return;
        }
        let Ok(provider) = SoftwareProvider::new(key) else {
            self.report.warn(
                "local_signing_key_unavailable",
                format!("Local HMAC key is unavailable at {}; signature bytes were not verified", key.display()),
            );
            return;
        };
        let Some(signature) = python_from_hex(&python_str(field(seal, "signature_hex"))) else {
            self.report.error("signature_malformed", "Local integrity signature is not valid hex");
            return;
        };
        let device_id = provider.device_identity().map(|identity| identity.device_id).unwrap_or_default();
        if !python_eq(&Value::String(device_id), field(seal, "device_id")) {
            self.report.error("signer_mismatch", "Local signing key does not match manifest signer");
        } else if !provider.verify(signed_bytes, &signature).unwrap_or(false) {
            self.report.error("signature_invalid", "Local HMAC integrity seal is invalid");
        } else {
            self.report.info("local_signature_valid", "Local HMAC integrity seal verified");
        }
    }

    fn check_portable_signature(&mut self, manifest: &Map<String, Value>) {
        let pinned_path = expand_user(&self.options.public_key);
        let Some(portable) = manifest.get("portable_signature").filter(|value| value.is_object()) else {
            self.report.warn(
                "portable_signature_missing",
                "No portable Ed25519 signature is present; public-key integrity is untrusted",
            );
            return;
        };
        let Some(pinned) = std::fs::read(&pinned_path).ok().and_then(|raw| <[u8; 32]>::try_from(raw).ok()) else {
            // IMPORTANT: unverifiable is not tampered. portable_signature_invalid is a
            // "changed" code, so treating a missing pin as a failure accused a
            // recipient's untouched manifest of having moved.
            self.report.warn(
                "portable_key_unavailable",
                format!(
                    "No pinned Ed25519 public key at {}, so the portable signature was not verified. \
                     Pass --public-key <the producer's public key file> to check it.",
                    pinned_path.display()
                ),
            );
            return;
        };
        let unsigned = without_top_level_keys(&Value::Object(manifest.clone()), &PORTABLE_SIGNATURE_EXCLUDED_KEYS);
        match verify_pinned_signature(&unsigned, portable, &pinned, &pinned_path) {
            Ok(message) => {
                self.report.complete("portable_signature");
                self.report.info("portable_signature_valid", message);
                self.report.warn(
                    "signer_identity_unverified",
                    "Signature validity does not establish the signer’s externally verified identity",
                );
            }
            Err(message) => self.report.error("portable_signature_invalid", message),
        }
    }

    /// Re-verify the recorded C2PA claim against the signed asset on disk.
    ///
    /// IMPORTANT: trust is evaluated against an anchor this machine holds, never
    /// against `signer.trust_anchor_pem` inside the manifest being verified. A forger
    /// generates their own root, signs a fabricated asset with it, writes that root
    /// into the manifest, and every layer downstream reports "verified".
    fn check_c2pa_claim(&mut self, manifest: &Map<String, Value>) {
        let Some(claim) = some(manifest, "c2pa_claim") else {
            self.report.warn("c2pa_claim_missing", "Manifest records no C2PA claim section");
            return;
        };
        let Some(claim) = claim.as_object() else {
            self.report.error("schema_invalid", "c2pa_claim must be an object");
            return;
        };
        if python_eq(field(claim, "status"), &json!("unavailable")) {
            self.report.warn(
                "c2pa_claim_unavailable",
                format!(
                    "No C2PA claim was produced: {}",
                    claim.get("reason").map_or_else(|| "no reason recorded".to_owned(), python_str)
                ),
            );
            return;
        }
        let empty = Map::new();
        let signer = object_of(claim.get("signer")).unwrap_or(&empty);
        let asset = object_of(claim.get("signed_asset")).unwrap_or(&empty);
        if !python_eq(field(signer, "signer_identity"), &json!("not_established"))
            || python_eq(field(signer, "apw:proof_level"), &json!("externally_verified"))
        {
            self.report.error(
                "c2pa_signer_identity_overclaimed",
                "The C2PA claim presents a self-issued signer as an established identity",
            );
        }

        let asset_path = self.options.c2pa_asset.clone().or_else(|| {
            let relative = asset.get("relative_path").filter(|value| is_truthy(value))?;
            contained_artifact(self.manifest_path, relative).filter(|candidate| candidate.is_file())
        });
        let Some(asset_path) = asset_path.filter(|path| path.is_file()) else {
            self.report.warn("c2pa_asset_unavailable", "The C2PA-signed asset is unavailable, so its claim could not be re-read");
            return;
        };
        let Ok(actual) = sha256_file(&asset_path) else {
            self.report.warn("c2pa_asset_unavailable", "The C2PA-signed asset could not be read");
            return;
        };
        if !python_eq(&Value::String(actual), field(asset, "sha256")) {
            // Not a return: the recorded digest and the real hard binding are
            // independent checks, and a tampered asset must exercise both.
            self.report.error(
                "c2pa_asset_hash_mismatch",
                format!(
                    "The C2PA-signed asset {} no longer matches the digest recorded in the manifest",
                    path_name(&asset_path)
                ),
            );
        }

        let Some(anchors) = load_trust_anchors(self.options.trust_anchor.as_deref()) else {
            self.report.warn(
                "c2pa_trust_anchor_unavailable",
                "This machine holds no C2PA trust anchor, so the claim signer was not evaluated; \
                 an anchor inside the manifest cannot vouch for the manifest",
            );
            return;
        };
        if let Some(recorded) = signer.get("trust_anchor_pem").and_then(Value::as_str) {
            if recorded.contains("BEGIN CERTIFICATE") && certificate_bodies(recorded) != certificate_bodies(&anchors) {
                self.report.error(
                    "c2pa_trust_anchor_untrusted",
                    "The manifest names a trust root this machine does not hold; the claim was \
                     signed under a certificate authority outside the configured anchor",
                );
                return;
            }
        }

        let sidecar = claim
            .get("sidecar_manifest")
            .filter(|value| is_truthy(value))
            .and_then(|value| contained_artifact(self.manifest_path, value))
            .and_then(|path| std::fs::read(path).ok());
        let mime = claim.get("mime").filter(|value| is_truthy(value)).map_or_else(|| "audio/wav".to_owned(), python_str);
        let live = match apw_c2pa::verify_asset(&asset_path, &mime, Some(&anchors), sidecar.as_deref()) {
            Ok(live) => live,
            Err(error) => {
                self.report.error("c2pa_claim_unreadable", format!("The recorded C2PA claim could not be re-read: {error}"));
                return;
            }
        };

        let recorded = object_of(claim.get("validation")).and_then(|validation| validation.get("state"));
        if !recorded.is_some_and(|state| python_eq(state, &json!(live.state.as_str()))) {
            self.report.warn(
                "c2pa_claim_state_changed",
                format!(
                    "The C2PA claim now validates as {}, not {} as recorded",
                    live.state.as_str(),
                    recorded.map_or_else(|| "None".to_owned(), python_str)
                ),
            );
        }
        match live.state {
            VerificationState::Verified => {
                self.report.complete("c2pa_claim");
                self.report.info(
                    "c2pa_claim_verified",
                    "The C2PA claim chains to the trust anchor held by this machine and its \
                     hard binding is intact; this is not an externally verified identity",
                );
            }
            VerificationState::RegisteredButChanged => self.report.error(
                "c2pa_hard_binding_broken",
                format!("The C2PA hard binding no longer covers the signed asset: {}", live.detail),
            ),
            VerificationState::NothingFound => self.report.error(
                "c2pa_claim_absent",
                "The manifest records a C2PA claim, but the signed asset carries none",
            ),
            VerificationState::MarkFoundClaimNotTrusted => {
                self.report.error("c2pa_claim_untrusted", format!("The C2PA claim is not trusted: {}", live.detail));
            }
        }
    }

    fn check_session_facts(&mut self, manifest: &Map<String, Value>) {
        match some(manifest, "session_facts") {
            Some(Value::Object(facts)) => {
                let tracks = facts.get("tracks").and_then(Value::as_array).map_or(0, Vec::len);
                self.report.info(
                    "session_facts",
                    format!(
                        "Session facts present: {tracks} tracks, BPM {}",
                        facts.get("bpm").map_or_else(|| "None".to_owned(), python_str)
                    ),
                );
            }
            Some(_) => self.report.error("schema_invalid", "session_facts must be an object"),
            None => self.report.warn("no_session_facts", "No session_facts (no .als project data)"),
        }
    }

    fn check_presentation(&mut self, manifest: &Map<String, Value>) {
        let Some(report_name) = object_of(manifest.get("presentation"))
            .and_then(|presentation| presentation.get("html_report"))
            .filter(|value| is_truthy(value))
            .map(python_str)
        else {
            return;
        };
        let report_path = self.manifest_path.parent().unwrap_or(Path::new("")).join(&report_name);
        if report_path.is_file() {
            self.report.info("html_report_present", format!("HTML fight card present: {}", path_name(&report_path)));
        } else {
            self.report.warn("html_report_missing", format!("HTML fight card is missing: {}", path_name(&report_path)));
        }
    }
}
