use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

pub use audio_provenance_core::LOCATOR_BYTES;
use audio_provenance_core::sha256;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::RegistryError;

pub const MARK_ID_BYTES: usize = 1 + LOCATOR_BYTES;
pub const MARK_ID_HEX_LEN: usize = MARK_ID_BYTES * 2;
pub const DIGEST_BYTES: usize = 32;
pub const DIGEST_HEX_LEN: usize = DIGEST_BYTES * 2;
pub const MAX_FINGERPRINT_BYTES: usize = 4096;

fn parse_lower_hex<const N: usize>(
    text: &str,
    field: &'static str,
) -> Result<[u8; N], RegistryError> {
    if text.len() != N * 2 {
        return Err(RegistryError::HexLength {
            field,
            expected: N * 2,
            found: text.len(),
        });
    }
    // IMPORTANT: uppercase is rejected rather than folded. These strings are filenames on a
    // case-insensitive volume, where two spellings of one id would collide on disk.
    if !text
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(RegistryError::HexCharset { field });
    }
    let mut out = [0u8; N];
    hex::decode_to_slice(text, &mut out).map_err(|_| RegistryError::HexCharset { field })?;
    Ok(out)
}

/// The 56-bit GenoMark payload that keys a registry record.
///
/// The locator half is derived by the record's own signer from its public key and the record's
/// `locator_salt` ([`audio_provenance_core::derive_locator`]), never from the record's contents. A holder
/// of the index cannot mint one for a key it does not have.
///
/// IMPORTANT: `GENOMARK_SPEC` §5 fixes the bit fields (version 0..3, namespace 4..7,
/// locator 8..55) but not their byte packing. This crate packs them into 7 bytes as
/// `[version | namespace << 4, locator[0..6]]` and renders that big-endian as 14 lowercase
/// hex characters. Every on-disk key and every URL path segment uses that spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MarkId {
    version: u8,
    namespace: u8,
    locator: [u8; LOCATOR_BYTES],
}

impl MarkId {
    pub fn new(
        version: u8,
        namespace: u8,
        locator: [u8; LOCATOR_BYTES],
    ) -> Result<Self, RegistryError> {
        if version > 0x0f {
            return Err(RegistryError::MarkField {
                field: "version",
                found: version,
            });
        }
        if namespace > 0x0f {
            return Err(RegistryError::MarkField {
                field: "namespace",
                found: namespace,
            });
        }
        Ok(Self {
            version,
            namespace,
            locator,
        })
    }

    pub fn parse_hex(text: &str) -> Result<Self, RegistryError> {
        let raw: [u8; MARK_ID_BYTES] = parse_lower_hex(text, "mark id")?;
        let mut locator = [0u8; LOCATOR_BYTES];
        locator.copy_from_slice(raw.get(1..).unwrap_or_default());
        Self::new(
            raw.first().copied().unwrap_or(0) & 0x0f,
            raw.first().copied().unwrap_or(0) >> 4,
            locator,
        )
    }

    pub fn to_hex(self) -> String {
        let mut raw = [0u8; MARK_ID_BYTES];
        if let Some(head) = raw.first_mut() {
            *head = self.version | (self.namespace << 4);
        }
        if let Some(tail) = raw.get_mut(1..) {
            tail.copy_from_slice(&self.locator);
        }
        hex::encode(raw)
    }

    pub const fn version(self) -> u8 {
        self.version
    }

    pub const fn namespace(self) -> u8 {
        self.namespace
    }

    pub const fn locator(self) -> [u8; LOCATOR_BYTES] {
        self.locator
    }

    pub fn locator_hex(self) -> String {
        hex::encode(self.locator)
    }
}

impl fmt::Display for MarkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// SHA-256 over the canonical bytes of a signed manifest.
///
/// It identifies a record exactly; it does NOT name the record's mark. A record fetched by mark id
/// is disambiguated, and a substituted manifest detected, by re-deriving the locator from the
/// received document's own key and salt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordId([u8; DIGEST_BYTES]);

impl RecordId {
    pub fn from_manifest_bytes(canonical_manifest: &[u8]) -> Self {
        Self(sha256(canonical_manifest))
    }

    pub fn parse_hex(text: &str) -> Result<Self, RegistryError> {
        Ok(Self(parse_lower_hex(text, "record id")?))
    }

    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }

    pub const fn as_bytes(&self) -> &[u8; DIGEST_BYTES] {
        &self.0
    }
}

impl fmt::Display for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// SHA-256 over the audio bytes a manifest's hard binding covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash([u8; DIGEST_BYTES]);

impl ContentHash {
    pub const fn from_bytes(digest: [u8; DIGEST_BYTES]) -> Self {
        Self(digest)
    }

    pub fn from_content(content: &[u8]) -> Self {
        Self(sha256(content))
    }

    pub fn parse_hex(text: &str) -> Result<Self, RegistryError> {
        Ok(Self(parse_lower_hex(text, "content hash")?))
    }

    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }

    pub const fn as_bytes(&self) -> &[u8; DIGEST_BYTES] {
        &self.0
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// An opaque perceptual fingerprint. The registry stores and compares these; it does not
/// define how they are computed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fingerprint(Vec<u8>);

impl Fingerprint {
    pub fn new(bytes: Vec<u8>) -> Result<Self, RegistryError> {
        if bytes.is_empty() || bytes.len() > MAX_FINGERPRINT_BYTES {
            return Err(RegistryError::FingerprintSize {
                limit: MAX_FINGERPRINT_BYTES,
                found: bytes.len(),
            });
        }
        Ok(Self(bytes))
    }

    pub fn parse_hex(text: &str) -> Result<Self, RegistryError> {
        if text.len() > MAX_FINGERPRINT_BYTES * 2 {
            return Err(RegistryError::FingerprintSize {
                limit: MAX_FINGERPRINT_BYTES,
                found: text.len() / 2,
            });
        }
        if !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(RegistryError::HexCharset {
                field: "fingerprint",
            });
        }
        let bytes = hex::decode(text).map_err(|_| RegistryError::HexCharset {
            field: "fingerprint",
        })?;
        Self::new(bytes)
    }

    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Fraction of bits shared with `other`, or `None` when the two are not comparable.
    ///
    /// Fingerprints of different lengths describe different amounts of audio, so no distance
    /// between them is meaningful; returning `None` keeps that out of a score.
    pub fn bit_agreement(&self, other: &Self) -> Option<f32> {
        if self.0.len() != other.0.len() {
            return None;
        }
        let differing: u32 = self
            .0
            .iter()
            .zip(other.0.iter())
            .map(|(a, b)| (a ^ b).count_ones())
            .sum();
        let bits = (self.0.len() as f32) * 8.0;
        Some(1.0 - (differing as f32) / bits)
    }
}

/// An RFC 3339 UTC instant, stored verbatim.
///
/// The registry never reads a clock; a signing time is always supplied by the caller.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SignedAt(String);

impl SignedAt {
    pub fn parse(text: &str) -> Result<Self, RegistryError> {
        if !text.is_ascii() || text.len() < 20 || text.len() > 30 {
            return Err(RegistryError::Timestamp);
        }
        let field = |range: core::ops::Range<usize>| -> Option<u32> {
            let part = text.get(range)?;
            if !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse::<u32>().ok()
        };
        let separator = |at: usize, expected: u8| -> bool {
            text.as_bytes().get(at).copied() == Some(expected)
        };

        let (Some(year), Some(month), Some(day)) = (field(0..4), field(5..7), field(8..10)) else {
            return Err(RegistryError::Timestamp);
        };
        let (Some(hour), Some(minute), Some(second)) =
            (field(11..13), field(14..16), field(17..19))
        else {
            return Err(RegistryError::Timestamp);
        };
        if !(separator(4, b'-')
            && separator(7, b'-')
            && separator(10, b'T')
            && separator(13, b':')
            && separator(16, b':'))
        {
            return Err(RegistryError::Timestamp);
        }
        if year == 0
            || !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return Err(RegistryError::Timestamp);
        }

        let tail = text.get(19..).ok_or(RegistryError::Timestamp)?;
        let fraction = match tail.strip_suffix('Z') {
            Some(rest) => rest,
            None => return Err(RegistryError::Timestamp),
        };
        if !fraction.is_empty() {
            let digits = fraction.strip_prefix('.').ok_or(RegistryError::Timestamp)?;
            if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(RegistryError::Timestamp);
            }
        }
        Ok(Self(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The `YYYY-MM-DD` prefix, which is what a verifier renders.
    pub fn date(&self) -> &str {
        self.0.split('T').next().unwrap_or(&self.0)
    }
}

impl fmt::Display for SignedAt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

macro_rules! hex_serde {
    ($type:ty, $render:expr, $parse:expr) => {
        impl Serialize for $type {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&$render(self))
            }
        }

        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                $parse(text.as_str()).map_err(D::Error::custom)
            }
        }
    };
}

hex_serde!(MarkId, |v: &MarkId| v.to_hex(), MarkId::parse_hex);
hex_serde!(RecordId, |v: &RecordId| v.to_hex(), RecordId::parse_hex);
hex_serde!(
    ContentHash,
    |v: &ContentHash| v.to_hex(),
    ContentHash::parse_hex
);
hex_serde!(Fingerprint, Fingerprint::to_hex, Fingerprint::parse_hex);
hex_serde!(
    SignedAt,
    |v: &SignedAt| v.as_str().to_string(),
    SignedAt::parse
);
