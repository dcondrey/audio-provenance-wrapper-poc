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
}

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
pub const QUALIFIED_SCOPE: &str = "Local POC integrity outcome; not a Audio Provenance registry, identity, rights, or authorship result.";

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
    /// finding at any severity; else verified.
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
