use alloc::string::String;
use core::fmt;

use audio_provenance_core::CodedError;

/// Why a backend could not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnavailableKind {
    Io,
    IndexCorrupt,
    Transport,
    Timeout,
    Unauthorized,
    RateLimited,
    ServerError,
    ResponseTooLarge,
    MalformedResponse,
}

impl UnavailableKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Io => "io",
            Self::IndexCorrupt => "index_corrupt",
            Self::Transport => "transport",
            Self::Timeout => "timeout",
            Self::Unauthorized => "unauthorized",
            Self::RateLimited => "rate_limited",
            Self::ServerError => "server_error",
            Self::ResponseTooLarge => "response_too_large",
            Self::MalformedResponse => "malformed_response",
        }
    }
}

impl fmt::Display for UnavailableKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    kind: UnavailableKind,
    detail: String,
}

impl Unavailable {
    pub fn new(kind: UnavailableKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub const fn kind(&self) -> UnavailableKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "registry unavailable ({}): {}", self.kind, self.detail)
    }
}

impl CodedError for Unavailable {
    fn code(&self) -> &'static str {
        match self.kind {
            UnavailableKind::Io => "registry_unavailable_io",
            UnavailableKind::IndexCorrupt => "registry_unavailable_index_corrupt",
            UnavailableKind::Transport => "registry_unavailable_transport",
            UnavailableKind::Timeout => "registry_unavailable_timeout",
            UnavailableKind::Unauthorized => "registry_unavailable_unauthorized",
            UnavailableKind::RateLimited => "registry_unavailable_rate_limited",
            UnavailableKind::ServerError => "registry_unavailable_server_error",
            UnavailableKind::ResponseTooLarge => "registry_unavailable_response_too_large",
            UnavailableKind::MalformedResponse => "registry_unavailable_malformed_response",
        }
    }
}

/// The outcome of a registry query.
///
/// IMPORTANT: there is deliberately no `Option` conversion, no `From<Lookup<T>> for Option<T>`
/// and no `ok()`. An outage collapsed into a miss turns a network fault into the provenance
/// verdict "unregistered", which is the one failure this crate exists to prevent. Callers must
/// match all three arms.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup<T> {
    Found(T),
    NotFound,
    Unavailable(Unavailable),
}

impl<T> Lookup<T> {
    pub fn unavailable(kind: UnavailableKind, detail: impl Into<String>) -> Self {
        Self::Unavailable(Unavailable::new(kind, detail))
    }

    pub const fn is_found(&self) -> bool {
        matches!(self, Self::Found(_))
    }

    pub const fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound)
    }

    pub const fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }

    pub const fn unavailable_reason(&self) -> Option<&Unavailable> {
        match self {
            Self::Unavailable(reason) => Some(reason),
            _ => None,
        }
    }

    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Lookup<U> {
        match self {
            Self::Found(value) => Lookup::Found(f(value)),
            Self::NotFound => Lookup::NotFound,
            Self::Unavailable(reason) => Lookup::Unavailable(reason),
        }
    }
}
