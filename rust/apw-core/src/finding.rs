use std::collections::HashSet;

use serde_json::{json, Value};

use crate::state::LocalOutcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

impl core::fmt::Display for Severity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct VerificationReport {
    pub findings: Vec<Finding>,
    completed_checks: HashSet<String>,
}

/// Load-bearing checks, in report order, each with what a run that skipped it
/// could not claim. `verified` is earned by running them, never by finding
/// nothing wrong.
pub const REQUIRED_CHECKS: [(&str, &str); 4] = [
    (
        "export_binding",
        "the export file was not available, so its hard binding was never checked",
    ),
    (
        "evidence_binding",
        "no bound evidence prefix could be hashed, so the coverage counters were never re-derived",
    ),
    (
        "portable_signature",
        "the portable Ed25519 signature was not verified against a pinned public key",
    ),
    (
        "c2pa_claim",
        "the C2PA claim was not re-verified against a locally held trust anchor",
    ),
];

/// Codes that force [`LocalOutcome::Changed`] regardless of severity.
pub const CHANGED_CODES: [&str; 9] = [
    "export_hash_mismatch",
    "evidence_hash_mismatch",
    "evidence_truncated",
    "tampered",
    "signature_invalid",
    "portable_signature_invalid",
    "stem_commitment_mismatch",
    "c2pa_asset_hash_mismatch",
    "c2pa_hard_binding_broken",
];

/// IMPORTANT: this string is reproduced byte-for-byte from
/// `daemon/verify.py::VerificationResult.to_dict`. It lands in
/// `artifacts/*_verification.json`, which the evidence bundle hashes, so
/// rewording it would break bundle parity with the Python implementation.
pub const QUALIFIED_SCOPE: &str = "Local POC integrity outcome; not a registry, identity, rights, or authorship result from any provider.";

const NOT_FOUND_CODE: &str = "not_found";
const PORTABLE_SIGNATURE_MISSING_CODE: &str = "portable_signature_missing";

impl VerificationReport {
    pub fn new() -> Self {
        VerificationReport::default()
    }

    pub fn error(&mut self, code: &str, message: impl Into<String>) {
        self.push(Severity::Error, code, message);
    }

    pub fn warn(&mut self, code: &str, message: impl Into<String>) {
        self.push(Severity::Warning, code, message);
    }

    pub fn info(&mut self, code: &str, message: impl Into<String>) {
        self.push(Severity::Info, code, message);
    }

    pub fn extend(&mut self, other: VerificationReport) {
        self.findings.extend(other.findings);
        self.completed_checks.extend(other.completed_checks);
    }

    pub fn complete(&mut self, check: &str) {
        self.completed_checks.insert(check.to_owned());
    }

    /// Required checks that never ran, with the reason each leaves unclaimed.
    pub fn unchecked(&self) -> Vec<(&'static str, &'static str)> {
        REQUIRED_CHECKS
            .into_iter()
            .filter(|(name, _)| !self.completed_checks.contains(*name))
            .collect()
    }

    pub fn errors(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == Severity::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == Severity::Warning)
    }

    pub fn passed(&self) -> bool {
        self.errors().next().is_none()
    }

    /// Precedence, in order: any `not_found` code wins; then any
    /// [`CHANGED_CODES`]; then any error OR the `portable_signature_missing`
    /// finding at any severity; then any unrun required check; else verified.
    ///
    /// IMPORTANT: the third clause is not "any error" alone. Python tests the
    /// code set, so a `portable_signature_missing` warning downgrades a report
    /// that carries no error at all.
    pub fn outcome(&self) -> LocalOutcome {
        if self.has_code(NOT_FOUND_CODE) {
            return LocalOutcome::NotFound;
        }
        if self
            .findings
            .iter()
            .any(|finding| CHANGED_CODES.contains(&finding.code.as_str()))
        {
            return LocalOutcome::Changed;
        }
        if !self.passed() || self.has_code(PORTABLE_SIGNATURE_MISSING_CODE) {
            return LocalOutcome::Untrusted;
        }
        if !self.unchecked().is_empty() {
            return LocalOutcome::Incomplete;
        }
        LocalOutcome::Verified
    }

    pub fn error_messages(&self) -> Vec<String> {
        self.errors()
            .map(|finding| finding.message.clone())
            .collect()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "verifier": "local_audio_provenance_poc",
            "outcome": self.outcome().as_str(),
            "qualified_scope": QUALIFIED_SCOPE,
            "passed": self.passed(),
            "checks_not_run": self
                .unchecked()
                .into_iter()
                .map(|(check, reason)| json!({"check": check, "reason": reason}))
                .collect::<Vec<Value>>(),
            "findings": self
                .findings
                .iter()
                .map(|finding| json!({
                    "severity": finding.severity.as_str(),
                    "code": finding.code,
                    "message": finding.message,
                }))
                .collect::<Vec<Value>>(),
        })
    }

    fn has_code(&self, code: &str) -> bool {
        self.findings.iter().any(|finding| finding.code == code)
    }

    fn push(&mut self, severity: Severity, code: &str, message: impl Into<String>) {
        self.findings.push(Finding {
            severity,
            code: code.to_owned(),
            message: message.into(),
        });
    }
}
