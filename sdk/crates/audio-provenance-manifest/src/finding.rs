use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// One validation result.
///
/// `daemon/schema.py` returns a flat list of message strings and never fails fast, so a single
/// malformed section yields several entries. This type keeps that one-finding-per-rule fidelity and
/// adds the stable `code` and the JSON `path` the Python only encoded inside its prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    severity: Severity,
    code: &'static str,
    path: String,
    message: String,
}

impl Finding {
    pub fn error(code: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, path, message)
    }

    pub fn warning(
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(Severity::Warning, code, path, message)
    }

    pub fn info(code: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Severity::Info, code, path, message)
    }

    fn new(
        severity: Severity,
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code,
            path: path.into(),
            message: message.into(),
        }
    }

    pub const fn severity(&self) -> Severity {
        self.severity
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

pub fn has_errors(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.severity == Severity::Error)
}

pub(crate) fn error_messages(findings: &[Finding]) -> Vec<String> {
    findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .map(|f| f.message.to_string())
        .collect()
}
