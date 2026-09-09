use alloc::string::String;
use core::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::error::LocatorError;
use crate::hashing::sha256;

/// Domain separator for the locator preimage. Nothing else in this workspace hashes a bare
/// public key, and the tag keeps it that way if something ever does.
pub const LOCATOR_DOMAIN: &[u8] = b"audio-provenance-locator-v1";
pub const LOCATOR_SALT_BYTES: usize = 16;
pub const LOCATOR_BYTES: usize = 6;

/// `WATERMARK_SPEC` section 5's payload version. It lives here rather than in `apw_watermark` because
/// `audio-provenance-registry` needs it to default a record's mark fields and must not depend on the
/// detector, which pulls an FFT.
pub const WATERMARK_PAYLOAD_VERSION: u8 = 1;

pub const LOCATOR_SALT_HEX_LEN: usize = LOCATOR_SALT_BYTES * 2;

const PREIMAGE_BYTES: usize = LOCATOR_DOMAIN.len() + 1 + 32 + LOCATOR_SALT_BYTES;

/// The per-record randomness a locator is derived from.
///
/// It is public: it travels inside the signed manifest, and the public key beside it is already
/// published in `portable_signature`. Its job is not secrecy but allocation order. The locator has
/// to exist BEFORE the audio is marked, and everything else that could name a record (the manifest
/// digest, the content hash, the decoded-audio digest) is a function of audio the mark has not been
/// written into yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocatorSalt([u8; LOCATOR_SALT_BYTES]);

impl LocatorSalt {
    pub const fn from_bytes(raw: [u8; LOCATOR_SALT_BYTES]) -> Self {
        Self(raw)
    }

    pub const fn as_bytes(&self) -> &[u8; LOCATOR_SALT_BYTES] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }

    /// IMPORTANT: uppercase is rejected rather than folded, matching `audio-provenance-registry`'s id
    /// parsing. Two spellings of one salt would derive one locator while reading as two distinct
    /// signed documents.
    pub fn parse_hex(text: &str) -> Result<Self, LocatorError> {
        if text.len() != LOCATOR_SALT_HEX_LEN {
            return Err(LocatorError::SaltHexLength {
                expected: LOCATOR_SALT_HEX_LEN,
                found: text.len(),
            });
        }
        if !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(LocatorError::SaltHexCharset);
        }
        let mut raw = [0u8; LOCATOR_SALT_BYTES];
        hex::decode_to_slice(text, &mut raw).map_err(|_| LocatorError::SaltHexCharset)?;
        Ok(Self(raw))
    }
}

impl fmt::Display for LocatorSalt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for LocatorSalt {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for LocatorSalt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse_hex(&text).map_err(D::Error::custom)
    }
}

/// The 48-bit Watermark locator: `sha256(domain || 0x00 || signer public key || salt)[..6]`.
///
/// IMPORTANT: nothing in the preimage is derived from the audio, the manifest or the signature.
/// That is the whole point. The locator has to be allocated before the mark is embedded, and the
/// manifest signed afterwards commits to the salt, so the mark still resolves only to a record its
/// own signer intended. Binding the public key in is what makes squatting another signer's locator
/// cost a 2^48 search rather than copying their published salt.
///
/// The preimage is fixed-length (68 bytes), so no length prefix is needed to keep it unambiguous.
pub fn derive_locator(public_key: &[u8; 32], salt: &LocatorSalt) -> [u8; LOCATOR_BYTES] {
    let mut preimage = [0u8; PREIMAGE_BYTES];
    let domain = LOCATOR_DOMAIN.len();
    preimage[..domain].copy_from_slice(LOCATOR_DOMAIN);
    preimage[domain] = 0;
    preimage[domain + 1..domain + 33].copy_from_slice(public_key);
    preimage[domain + 33..].copy_from_slice(&salt.0);

    let digest = sha256(&preimage);
    let mut locator = [0u8; LOCATOR_BYTES];
    locator.copy_from_slice(&digest[..LOCATOR_BYTES]);
    locator
}

/// The locator a signed manifest declares, derived from the document's OWN public key and salt.
///
/// The one implementation, shared by the registry and by Trace's mark rung, so the write path
/// and the re-derivation that polices it cannot drift apart. It is deliberately schema-agnostic:
/// any document carrying `portable_signature.public_key_hex` and a root `locator_salt` derives,
/// including the POC's `audio-provenance-manifest-v0`.
pub fn locator_from_signed_manifest(value: &Value) -> Option<[u8; LOCATOR_BYTES]> {
    let key_hex = value
        .get("portable_signature")?
        .get("public_key_hex")?
        .as_str()?;
    let mut public_key = [0u8; 32];
    hex::decode_to_slice(key_hex, &mut public_key).ok()?;
    let salt = LocatorSalt::parse_hex(value.get("locator_salt")?.as_str()?).ok()?;
    Some(derive_locator(&public_key, &salt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CodedError;

    /// The property the whole two-phase publish rests on: a locator names one (key, salt) pair.
    /// Move either and it moves, so a squatter cannot reuse a victim's published salt, and a
    /// signer cannot re-point one mark at a second record without a 2^48 search.
    #[test]
    fn a_locator_is_bound_to_both_the_signer_key_and_the_salt() {
        let salt = LocatorSalt::from_bytes([9u8; LOCATOR_SALT_BYTES]);
        let other_salt = LocatorSalt::from_bytes([10u8; LOCATOR_SALT_BYTES]);
        let key = [3u8; 32];
        let other_key = [4u8; 32];

        let locator = derive_locator(&key, &salt);
        assert_ne!(locator, derive_locator(&other_key, &salt));
        assert_ne!(locator, derive_locator(&key, &other_salt));
        assert_eq!(locator, derive_locator(&key, &salt));

        let manifest = serde_json::json!({
            "locator_salt": salt.to_hex(),
            "portable_signature": { "public_key_hex": hex::encode(key) },
        });
        assert_eq!(locator_from_signed_manifest(&manifest), Some(locator));

        let stripped = serde_json::json!({
            "portable_signature": { "public_key_hex": hex::encode(key) },
        });
        assert_eq!(locator_from_signed_manifest(&stripped), None);
    }

    #[test]
    fn a_salt_is_exactly_thirty_two_lowercase_hex_characters() {
        assert!(LocatorSalt::parse_hex(&"ab".repeat(16)).is_ok());
        assert_eq!(
            LocatorSalt::parse_hex(&"ab".repeat(15)).unwrap_err().code(),
            "locator_salt_hex_length"
        );
        assert_eq!(
            LocatorSalt::parse_hex(&"AB".repeat(16)).unwrap_err().code(),
            "locator_salt_hex_charset"
        );
    }
}
