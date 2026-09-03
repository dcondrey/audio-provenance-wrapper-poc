//! Reading a C2PA manifest store out of an audio container.
//!
//! SCOPE, STATED PLAINLY. This module locates the store, walks its JUMBF boxes, returns any
//! assertion's raw CBOR, and RECOMPUTES the `c2pa.hash.data` hard binding over a presented asset.
//! It does NOT parse or verify the COSE_Sign1 claim signature, and it does not build or validate an
//! X.509 chain, so it can never report a C2PA validation state on its own: a matching hard binding
//! proves the bytes did not move since signing, not that anyone trustworthy signed them. Signing is
//! out of scope entirely.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use audio_provenance_core::sha256;

use crate::binding::BindingOutcome;
use crate::error::C2paError;

/// The prior art's own stores run to ~14 KB. A C2PA store carrying a thumbnail can be larger, but
/// an allocation sized from an untrusted length field needs a ceiling.
pub const MAX_STORE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_JUMBF_DEPTH: usize = 16;
pub const MAX_JUMBF_BOXES: usize = 4096;

pub const HARD_BINDING_LABEL: &str = "c2pa.hash.data";
pub const SIDECAR_EXTENSION: &str = "c2pa";

const JUMD_TOGGLE_LABEL_PRESENT: u8 = 0x02;
const JUMD_HEADER_BYTES: usize = 17;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreLocation {
    /// A RIFF `C2PA` chunk. `offset` addresses the chunk header, `length` spans header, payload and
    /// any pad byte, which is exactly the region the hard binding excludes.
    EmbeddedWavChunk { offset: usize, length: usize },
    /// A bare JUMBF superbox with no container framing, as `Builder.set_no_embed()` writes beside
    /// an AIFF.
    Sidecar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct C2paStore {
    bytes: Vec<u8>,
    location: StoreLocation,
}

/// The sidecar a POC-signed non-embeddable asset gets, derived the way `sign_asset` derives it:
/// the asset's extension is REPLACED with `c2pa`, so `mix.aiff` becomes `mix.c2pa`.
///
/// The result is a bare file name or path with no directory traversal introduced; a caller still
/// resolves it against the input's real directory and never against a path read out of a manifest.
pub fn sidecar_path_for(asset_path: &str) -> String {
    let stem_end = asset_path
        .rfind('.')
        .filter(|dot| !asset_path[dot + 1..].contains(['/', '\\']))
        .filter(|dot| {
            let name_start = asset_path
                .rfind(['/', '\\'])
                .map_or(0, |sep| sep.saturating_add(1));
            *dot > name_start
        })
        .unwrap_or(asset_path.len());
    let mut out = String::with_capacity(stem_end + SIDECAR_EXTENSION.len() + 1);
    out.push_str(&asset_path[..stem_end]);
    out.push('.');
    out.push_str(SIDECAR_EXTENSION);
    out
}

impl C2paStore {
    /// Scans the RIFF chunk table of a WAV for the `C2PA` chunk.
    ///
    /// Returns [`C2paError::NotPresent`] for a well-formed WAV carrying no provenance, which is a
    /// finding and not a failure.
    pub fn from_wav(container: &[u8]) -> Result<Self, C2paError> {
        if container.len() < 12
            || container.get(..4) != Some(b"RIFF")
            || container.get(8..12) != Some(b"WAVE")
        {
            return Err(C2paError::MalformedRiff {
                offset: 0,
                reason: "not a RIFF/WAVE header",
            });
        }
        let mut offset = 12usize;
        while offset.saturating_add(8) <= container.len() {
            let Some(header) = container.get(offset..offset + 8) else {
                break;
            };
            let mut size_bytes = [0u8; 4];
            size_bytes.copy_from_slice(header.get(4..8).unwrap_or(&[0; 4]));
            let size = u32::from_le_bytes(size_bytes) as usize;
            let body_start = offset + 8;
            let body_end = body_start
                .checked_add(size)
                .ok_or(C2paError::MalformedRiff {
                    offset,
                    reason: "chunk length overflows the address space",
                })?;
            if body_end > container.len() {
                return Err(C2paError::MalformedRiff {
                    offset,
                    reason: "chunk declares more bytes than the file holds",
                });
            }
            if header.get(..4) == Some(b"C2PA") {
                let payload =
                    container
                        .get(body_start..body_end)
                        .ok_or(C2paError::MalformedRiff {
                            offset,
                            reason: "chunk body is truncated",
                        })?;
                let bytes = Self::checked(payload)?;
                return Ok(Self {
                    bytes,
                    location: StoreLocation::EmbeddedWavChunk {
                        offset,
                        length: 8 + size + (size & 1),
                    },
                });
            }
            let advance = 8usize
                .checked_add(size)
                .and_then(|n| n.checked_add(size & 1))
                .filter(|n| *n > 0)
                .ok_or(C2paError::MalformedRiff {
                    offset,
                    reason: "chunk length overflows the address space",
                })?;
            offset = offset.saturating_add(advance);
        }
        Err(C2paError::NotPresent)
    }

    pub fn from_sidecar(bytes: &[u8]) -> Result<Self, C2paError> {
        let bytes = Self::checked(bytes)?;
        Ok(Self {
            bytes,
            location: StoreLocation::Sidecar,
        })
    }

    fn checked(bytes: &[u8]) -> Result<Vec<u8>, C2paError> {
        if bytes.len() > MAX_STORE_BYTES {
            return Err(C2paError::TooLarge {
                limit: MAX_STORE_BYTES,
                found: bytes.len(),
            });
        }
        // A store that does not open with a JUMBF superbox is not a store; refusing here keeps a
        // caller from handing arbitrary chunk bytes to the box walker.
        if bytes.len() < 8 || bytes.get(4..8) != Some(b"jumb") {
            return Err(C2paError::MalformedJumbf {
                offset: 0,
                reason: "store does not begin with a JUMBF superbox",
            });
        }
        Ok(bytes.to_vec())
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn location(&self) -> StoreLocation {
        self.location
    }

    /// Every JUMBF description label in the store, outermost first.
    pub fn labels(&self) -> Result<Vec<String>, C2paError> {
        let mut out = Vec::new();
        let mut budget = MAX_JUMBF_BOXES;
        walk(&self.bytes, 0, 0, &mut budget, &mut |label, _| {
            out.push(label.to_string());
        })?;
        Ok(out)
    }

    /// The raw CBOR payload of the assertion carrying `label`, if the store holds one.
    pub fn assertion(&self, label: &str) -> Result<Option<Vec<u8>>, C2paError> {
        let mut found = None;
        let mut budget = MAX_JUMBF_BOXES;
        walk(&self.bytes, 0, 0, &mut budget, &mut |seen, payload| {
            if found.is_none() && seen == label {
                found = payload.map(<[u8]>::to_vec);
            }
        })?;
        Ok(found)
    }

    pub fn hard_binding(&self) -> Result<C2paHardBinding, C2paError> {
        let payload = self
            .assertion(HARD_BINDING_LABEL)?
            .ok_or(C2paError::NoHardBinding)?;
        C2paHardBinding::from_cbor(&payload)
    }
}

/// A byte range the hard binding does not cover, in the coordinates of the SIGNED asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exclusion {
    pub start: u64,
    pub length: u64,
}

/// A parsed `c2pa.hash.data` assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct C2paHardBinding {
    digest: [u8; 32],
    exclusions: Vec<Exclusion>,
    name: Option<String>,
}

impl C2paHardBinding {
    fn from_cbor(payload: &[u8]) -> Result<Self, C2paError> {
        let value: ciborium::value::Value =
            ciborium::from_reader(payload).map_err(|error| C2paError::MalformedAssertion {
                label: HARD_BINDING_LABEL,
                reason: to_message(&error),
            })?;
        let map = value
            .as_map()
            .ok_or_else(|| C2paError::MalformedAssertion {
                label: HARD_BINDING_LABEL,
                reason: "assertion is not a CBOR map".into(),
            })?;

        // The C2PA spec defaults `alg` to the claim's algorithm when absent, and every claim this
        // module has to read is SHA-256. Anything else is refused, never assumed.
        match entry(map, "alg").and_then(ciborium::value::Value::as_text) {
            None | Some("sha256") => {}
            Some(other) => {
                return Err(C2paError::UnsupportedHashAlgorithm {
                    found: other.to_string(),
                });
            }
        }

        let raw = entry(map, "hash")
            .and_then(ciborium::value::Value::as_bytes)
            .ok_or_else(|| C2paError::MalformedAssertion {
                label: HARD_BINDING_LABEL,
                reason: "assertion carries no hash byte string".into(),
            })?;
        let digest: [u8; 32] =
            raw.as_slice()
                .try_into()
                .map_err(|_| C2paError::MalformedAssertion {
                    label: HARD_BINDING_LABEL,
                    reason: "hash is not 32 bytes".into(),
                })?;

        let mut exclusions = Vec::new();
        if let Some(list) = entry(map, "exclusions").and_then(ciborium::value::Value::as_array) {
            for item in list {
                let entries = item.as_map().ok_or_else(|| C2paError::MalformedAssertion {
                    label: HARD_BINDING_LABEL,
                    reason: "an exclusion is not a CBOR map".into(),
                })?;
                let start = unsigned(entries, "start")?;
                let length = unsigned(entries, "length")?;
                exclusions.push(Exclusion { start, length });
            }
        }
        exclusions.sort_unstable_by_key(|e| (e.start, e.length));

        Ok(Self {
            digest,
            exclusions,
            name: entry(map, "name")
                .and_then(ciborium::value::Value::as_text)
                .map(ToString::to_string),
        })
    }

    pub const fn digest(&self) -> &[u8; 32] {
        &self.digest
    }

    pub fn digest_hex(&self) -> String {
        hex::encode(self.digest)
    }

    pub fn exclusions(&self) -> &[Exclusion] {
        &self.exclusions
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Recomputes the binding over `asset`: SHA-256 over every byte outside the excluded ranges, in
    /// order.
    ///
    /// The exclusions are the SIGNED asset's coordinates, so an exclusion that does not lie inside
    /// `asset` is an error rather than a mismatch: the file presented is not the shape the binding
    /// describes, and reporting that as "the audio changed" would be a guess.
    pub fn evaluate(&self, asset: &[u8]) -> Result<BindingOutcome, C2paError> {
        let mut hasher_input = Vec::with_capacity(asset.len());
        let mut cursor = 0usize;
        for exclusion in &self.exclusions {
            let start = usize::try_from(exclusion.start).ok();
            let end = start.and_then(|s| {
                usize::try_from(exclusion.length)
                    .ok()
                    .and_then(|len| s.checked_add(len))
            });
            let (Some(start), Some(end)) = (start, end) else {
                return Err(C2paError::ExclusionOutOfRange {
                    start: exclusion.start,
                    length: exclusion.length,
                    total: asset.len(),
                });
            };
            if start < cursor || end > asset.len() {
                return Err(C2paError::ExclusionOutOfRange {
                    start: exclusion.start,
                    length: exclusion.length,
                    total: asset.len(),
                });
            }
            hasher_input.extend_from_slice(asset.get(cursor..start).unwrap_or_default());
            cursor = end;
        }
        hasher_input.extend_from_slice(asset.get(cursor..).unwrap_or_default());
        if sha256(&hasher_input) == self.digest {
            Ok(BindingOutcome::Match)
        } else {
            Ok(BindingOutcome::Mismatch)
        }
    }
}

fn entry<'a>(
    map: &'a [(ciborium::value::Value, ciborium::value::Value)],
    key: &str,
) -> Option<&'a ciborium::value::Value> {
    map.iter()
        .find(|(k, _)| k.as_text() == Some(key))
        .map(|(_, v)| v)
}

fn unsigned(
    map: &[(ciborium::value::Value, ciborium::value::Value)],
    key: &'static str,
) -> Result<u64, C2paError> {
    let integer = entry(map, key)
        .and_then(ciborium::value::Value::as_integer)
        .ok_or_else(|| C2paError::MalformedAssertion {
            label: HARD_BINDING_LABEL,
            reason: alloc::format!("exclusion field {key} is missing or not an integer"),
        })?;
    u64::try_from(integer).map_err(|_| C2paError::MalformedAssertion {
        label: HARD_BINDING_LABEL,
        reason: alloc::format!("exclusion field {key} is not a non-negative 64-bit integer"),
    })
}

fn to_message<E: core::fmt::Debug>(error: &E) -> String {
    alloc::format!("{error:?}")
}

/// Visits every labelled JUMBF description box, handing the visitor the label and the first `cbor`
/// payload that sits beside it.
fn walk(
    data: &[u8],
    base: usize,
    depth: usize,
    budget: &mut usize,
    visit: &mut impl FnMut(&str, Option<&[u8]>),
) -> Result<(), C2paError> {
    if depth >= MAX_JUMBF_DEPTH {
        return Err(C2paError::JumbfDepthExceeded {
            limit: MAX_JUMBF_DEPTH,
        });
    }
    let mut offset = 0usize;
    while offset < data.len() {
        if *budget == 0 {
            return Err(C2paError::JumbfBoxLimit {
                limit: MAX_JUMBF_BOXES,
            });
        }
        *budget -= 1;

        let header = data
            .get(offset..offset + 8)
            .ok_or(C2paError::MalformedJumbf {
                offset: base + offset,
                reason: "box header is truncated",
            })?;
        let mut length_bytes = [0u8; 4];
        length_bytes.copy_from_slice(header.get(..4).unwrap_or(&[0; 4]));
        let length = u32::from_be_bytes(length_bytes) as usize;
        if length == 0 || length == 1 {
            return Err(C2paError::JumbfExtendedLength {
                offset: base + offset,
            });
        }
        if length < 8 {
            return Err(C2paError::MalformedJumbf {
                offset: base + offset,
                reason: "box length is below the 8-byte header",
            });
        }
        let end = offset
            .checked_add(length)
            .ok_or(C2paError::MalformedJumbf {
                offset: base + offset,
                reason: "box length overflows the address space",
            })?;
        if end > data.len() {
            return Err(C2paError::MalformedJumbf {
                offset: base + offset,
                reason: "box declares more bytes than the store holds",
            });
        }
        let payload = data.get(offset + 8..end).unwrap_or_default();

        if header.get(4..8) == Some(b"jumb") {
            if let Some(label) = description_label(payload) {
                visit(label, sibling_cbor(payload));
            }
            walk(payload, base + offset + 8, depth + 1, budget, visit)?;
        }
        offset = end;
    }
    Ok(())
}

/// The label of the `jumd` box that opens a JUMBF superbox. ISO/IEC 19566-5 puts a 16-byte type
/// UUID and a toggles byte first, and only carries a NUL-terminated label when the toggles say so.
fn description_label(superbox_payload: &[u8]) -> Option<&str> {
    let header = superbox_payload.get(..8)?;
    if header.get(4..8) != Some(b"jumd") {
        return None;
    }
    let mut length_bytes = [0u8; 4];
    length_bytes.copy_from_slice(header.get(..4)?);
    let length = u32::from_be_bytes(length_bytes) as usize;
    let body = superbox_payload.get(8..length.max(8))?;
    let toggles = *body.get(16)?;
    if toggles & JUMD_TOGGLE_LABEL_PRESENT == 0 {
        return None;
    }
    let tail = body.get(JUMD_HEADER_BYTES..)?;
    let end = tail.iter().position(|b| *b == 0).unwrap_or(tail.len());
    core::str::from_utf8(tail.get(..end)?).ok()
}

fn sibling_cbor(superbox_payload: &[u8]) -> Option<&[u8]> {
    let mut offset = 0usize;
    while offset + 8 <= superbox_payload.len() {
        let header = superbox_payload.get(offset..offset + 8)?;
        let mut length_bytes = [0u8; 4];
        length_bytes.copy_from_slice(header.get(..4)?);
        let length = u32::from_be_bytes(length_bytes) as usize;
        if length < 8 || offset.checked_add(length)? > superbox_payload.len() {
            return None;
        }
        if header.get(4..8) == Some(b"cbor") {
            return superbox_payload.get(offset + 8..offset + length);
        }
        offset += length;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_path_replaces_the_extension() {
        assert_eq!(sidecar_path_for("mix.aiff"), "mix.c2pa");
        assert_eq!(sidecar_path_for("/a/b/mix.aif"), "/a/b/mix.c2pa");
        assert_eq!(sidecar_path_for("noext"), "noext.c2pa");
        // A leading dot is a hidden file, not an extension.
        assert_eq!(sidecar_path_for("/a/.hidden"), "/a/.hidden.c2pa");
        // A dot in a directory name never becomes the split point.
        assert_eq!(sidecar_path_for("/a.b/mix"), "/a.b/mix.c2pa");
    }

    #[test]
    fn a_box_longer_than_its_store_is_refused() {
        let mut store = Vec::from(*b"\x00\x00\x00\x20jumb");
        store.extend_from_slice(&[0u8; 4]);
        let error = C2paStore::from_sidecar(&store)
            .and_then(|s| s.labels())
            .unwrap_err();
        assert!(matches!(error, C2paError::MalformedJumbf { .. }));
    }

    #[test]
    fn an_extended_length_box_is_refused_rather_than_guessed_at() {
        let mut store = Vec::from(*b"\x00\x00\x00\x01jumb");
        store.extend_from_slice(&[0u8; 16]);
        let error = C2paStore::from_sidecar(&store)
            .and_then(|s| s.labels())
            .unwrap_err();
        assert!(matches!(error, C2paError::JumbfExtendedLength { .. }));
    }
}
