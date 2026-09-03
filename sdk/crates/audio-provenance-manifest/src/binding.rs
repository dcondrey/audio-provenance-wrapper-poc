use alloc::string::{String, ToString};

use audio_provenance_core::ProofLevel;

use crate::error::ManifestError;

/// The result of recomputing a binding over the audio actually presented.
///
/// `Uncoverable` is not a soft "maybe": it means the manifest declares no binding of that kind, so
/// nothing was recomputed. GENOTRACE's predicate 3 treats it differently from `Mismatch` on
/// purpose, because the soft binding may substitute for an ABSENT hard binding and may never
/// override one that FAILED.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingOutcome {
    Match,
    Mismatch,
    Uncoverable,
}

impl BindingOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Mismatch => "mismatch",
            Self::Uncoverable => "uncoverable",
        }
    }
}

/// The exact-digest binding a manifest commits to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardBinding {
    content_sha256: String,
    content_bytes: Option<u64>,
    decoded_audio_sha256: Option<String>,
}

impl HardBinding {
    pub fn new(
        content_sha256: &str,
        content_bytes: Option<u64>,
        decoded_audio_sha256: Option<&str>,
    ) -> Result<Self, ManifestError> {
        let content_sha256 = require_digest(content_sha256, "content_sha256")?;
        let decoded_audio_sha256 = match decoded_audio_sha256 {
            Some(text) => Some(require_digest(text, "decoded_audio_sha256")?),
            None => None,
        };
        Ok(Self {
            content_sha256,
            content_bytes,
            decoded_audio_sha256,
        })
    }

    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    pub const fn content_bytes(&self) -> Option<u64> {
        self.content_bytes
    }

    pub fn decoded_audio_sha256(&self) -> Option<&str> {
        self.decoded_audio_sha256.as_deref()
    }

    /// A hard binding that matches is `directly_observed`: the index key is the whole file, and no
    /// inference stands between the digest and the verdict.
    pub const fn proof_level(&self) -> ProofLevel {
        ProofLevel::DirectlyObserved
    }

    pub fn evaluate_content(&self, observed_sha256: &str) -> BindingOutcome {
        compare(Some(&self.content_sha256), observed_sha256)
    }

    /// Survives a container-metadata-only rewrite (an added tag, a rebuilt chunk table) and breaks
    /// on any re-encode, so it is a distinct answer from [`Self::evaluate_content`] and never a
    /// substitute for it.
    pub fn evaluate_decoded_audio(&self, observed_sha256: &str) -> BindingOutcome {
        compare(self.decoded_audio_sha256.as_deref(), observed_sha256)
    }
}

fn compare(expected: Option<&str>, observed: &str) -> BindingOutcome {
    match expected {
        None => BindingOutcome::Uncoverable,
        Some(expected) if expected.eq_ignore_ascii_case(observed) => BindingOutcome::Match,
        Some(_) => BindingOutcome::Mismatch,
    }
}

/// A perceptual descriptor of the work a record was signed over.
///
/// It never decides whether audio is UNMODIFIED, so it carries no method that can produce a
/// `Match`; that remains the hard binding's job alone. What it can answer is whether the audio
/// presented is the same WORK, which is the question a lossy path leaves open and the hard binding
/// cannot reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFingerprint {
    algorithm: String,
    digest_hex: String,
}

/// A fingerprint is a bounded descriptor, not a payload channel. The descriptor GenoTrace writes is
/// a landmark constellation, whose size is linear in the duration of the work: 1 MiB of hex is
/// about two hours of audio at the scheme's peak density.
pub const MAX_FINGERPRINT_HEX_LEN: usize = 1_048_576;
const MAX_ALGORITHM_LEN: usize = 64;

impl AudioFingerprint {
    pub fn new(algorithm: &str, digest_hex: &str) -> Result<Self, ManifestError> {
        if algorithm.is_empty() || algorithm.len() > MAX_ALGORITHM_LEN {
            return Err(ManifestError::FieldType {
                field: "fingerprint.algorithm",
                expected: "a non-empty name of at most 64 bytes",
            });
        }
        if digest_hex.is_empty()
            || digest_hex.len() > MAX_FINGERPRINT_HEX_LEN
            || !is_lower_hex(digest_hex)
        {
            return Err(ManifestError::FieldType {
                field: "fingerprint.digest",
                expected: "non-empty lowercase hexadecimal within the fingerprint size budget",
            });
        }
        Ok(Self {
            algorithm: algorithm.to_string(),
            digest_hex: digest_hex.to_string(),
        })
    }

    pub fn algorithm(&self) -> &str {
        &self.algorithm
    }

    pub fn digest_hex(&self) -> &str {
        &self.digest_hex
    }

    /// IMPORTANT: never higher. A fingerprint match is perceptual similarity, so the manifest it
    /// resolves is a guess about which work this is.
    pub const fn proof_level(&self) -> ProofLevel {
        ProofLevel::Inferred
    }
}

pub(crate) fn require_digest(text: &str, field: &'static str) -> Result<String, ManifestError> {
    if text.len() != 64 || !is_lower_hex(text) {
        return Err(ManifestError::MalformedDigest { field });
    }
    Ok(text.to_string())
}

pub(crate) fn is_lower_hex(text: &str) -> bool {
    text.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `YYYY-MM-DD`, the calendar-date form `VerifyResult.signedAt` publishes. Validated as a shape,
/// not resolved against a calendar: this crate has no clock and does not decide what "today" is.
pub(crate) fn require_calendar_date(
    text: &str,
    field: &'static str,
) -> Result<String, ManifestError> {
    let bytes = text.as_bytes();
    let shaped = bytes.len() == 10
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && [0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|i| bytes.get(*i).is_some_and(u8::is_ascii_digit));
    if !shaped {
        return Err(ManifestError::MalformedDate { field });
    }
    let month = text.get(5..7).and_then(|m| m.parse::<u8>().ok());
    let day = text.get(8..10).and_then(|d| d.parse::<u8>().ok());
    match (month, day) {
        (Some(1..=12), Some(1..=31)) => Ok(text.to_string()),
        _ => Err(ManifestError::MalformedDate { field }),
    }
}
