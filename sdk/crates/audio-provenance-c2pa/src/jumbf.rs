//! A JUMBF walk that yields each labelled superbox's PAYLOAD.
//!
//! IMPORTANT: this is deliberately not `audio_provenance_manifest::C2paStore::assertion`, which returns the
//! `cbor` content box's bytes. A hashed URI's preimage is a different byte range: the C2PA
//! specification's "Hashing JUMBF Boxes" says the hash is computed over the contents of the JUMBF
//! superbox EXCLUDING its header, which spans the `jumd` description box (salt included) and every
//! content box beside it. Hashing the content box alone reproduces no hashed URI in any real
//! manifest, so verifying the claim needs this traversal and cannot borrow that one.

use alloc::vec::Vec;

use crate::error::ClaimError;

pub const MAX_DEPTH: usize = 16;
pub const MAX_BOXES: usize = 4096;

const JUMD_TOGGLE_LABEL_PRESENT: u8 = 0x02;
const JUMD_HEADER_BYTES: usize = 17;

/// One labelled JUMBF superbox, in document order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelledBox<'a> {
    pub label: &'a str,
    /// The superbox contents with its own 8-byte header removed: the hashed-URI preimage.
    pub payload: &'a [u8],
    /// The label of the nearest labelled ancestor, which is what distinguishes an assertion inside
    /// `c2pa.assertions` from a same-named box anywhere else in the store.
    pub parent: Option<&'a str>,
    pub depth: usize,
}

impl LabelledBox<'_> {
    /// The first `cbor` content box beside the description box, if there is one.
    pub fn cbor(&self) -> Option<&[u8]> {
        let mut offset = 0usize;
        while offset.checked_add(8)? <= self.payload.len() {
            let header = self.payload.get(offset..offset + 8)?;
            let length = be_u32(header.get(..4)?)? as usize;
            if length < 8 || offset.checked_add(length)? > self.payload.len() {
                return None;
            }
            if header.get(4..8) == Some(b"cbor") {
                return self.payload.get(offset + 8..offset + length);
            }
            offset = offset.checked_add(length)?;
        }
        None
    }
}

/// Every labelled superbox in `store`, outermost first.
pub fn collect(store: &[u8]) -> Result<Vec<LabelledBox<'_>>, ClaimError> {
    let mut out = Vec::new();
    let mut budget = MAX_BOXES;
    walk(store, 0, 0, None, &mut budget, &mut out)?;
    Ok(out)
}

fn walk<'a>(
    data: &'a [u8],
    base: usize,
    depth: usize,
    parent: Option<&'a str>,
    budget: &mut usize,
    out: &mut Vec<LabelledBox<'a>>,
) -> Result<(), ClaimError> {
    if depth >= MAX_DEPTH {
        return Err(ClaimError::DepthExceeded { limit: MAX_DEPTH });
    }
    let mut offset = 0usize;
    while offset < data.len() {
        if *budget == 0 {
            return Err(ClaimError::BoxLimit { limit: MAX_BOXES });
        }
        *budget -= 1;

        let header =
            data.get(offset..offset.saturating_add(8))
                .ok_or(ClaimError::MalformedJumbf {
                    offset: base.saturating_add(offset),
                    reason: "box header is truncated",
                })?;
        let length = be_u32(header.get(..4).unwrap_or_default()).unwrap_or(0) as usize;
        if length == 0 || length == 1 {
            return Err(ClaimError::ExtendedLength {
                offset: base.saturating_add(offset),
            });
        }
        if length < 8 {
            return Err(ClaimError::MalformedJumbf {
                offset: base.saturating_add(offset),
                reason: "box length is below the 8-byte header",
            });
        }
        let end = offset
            .checked_add(length)
            .ok_or(ClaimError::MalformedJumbf {
                offset: base.saturating_add(offset),
                reason: "box length overflows the address space",
            })?;
        if end > data.len() {
            return Err(ClaimError::MalformedJumbf {
                offset: base.saturating_add(offset),
                reason: "box declares more bytes than the store holds",
            });
        }

        if header.get(4..8) == Some(b"jumb") {
            let payload = data.get(offset + 8..end).unwrap_or_default();
            let label = description_label(payload);
            if let Some(label) = label {
                out.push(LabelledBox {
                    label,
                    payload,
                    parent,
                    depth,
                });
            }
            walk(
                payload,
                base.saturating_add(offset).saturating_add(8),
                depth.saturating_add(1),
                label.or(parent),
                budget,
                out,
            )?;
        }
        offset = end;
    }
    Ok(())
}

/// ISO/IEC 19566-5 puts a 16-byte type UUID and a toggles byte at the front of the `jumd` box, and
/// only carries a NUL-terminated label when the toggles say so.
fn description_label(superbox_payload: &[u8]) -> Option<&str> {
    let header = superbox_payload.get(..8)?;
    if header.get(4..8) != Some(b"jumd") {
        return None;
    }
    let length = be_u32(header.get(..4)?)? as usize;
    let body = superbox_payload.get(8..length.max(8))?;
    if body.get(16)? & JUMD_TOGGLE_LABEL_PRESENT == 0 {
        return None;
    }
    let tail = body.get(JUMD_HEADER_BYTES..)?;
    let end = tail.iter().position(|b| *b == 0).unwrap_or(tail.len());
    core::str::from_utf8(tail.get(..end)?).ok()
}

fn be_u32(bytes: &[u8]) -> Option<u32> {
    let mut buf = [0u8; 4];
    buf.copy_from_slice(bytes.get(..4)?);
    Some(u32::from_be_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extended_length_box_is_refused_rather_than_guessed_at() {
        let mut store = Vec::from(*b"\x00\x00\x00\x01jumb");
        store.extend_from_slice(&[0u8; 16]);
        assert!(matches!(
            collect(&store),
            Err(ClaimError::ExtendedLength { .. })
        ));
    }

    #[test]
    fn a_box_longer_than_its_store_is_refused() {
        let mut store = Vec::from(*b"\x00\x00\x00\x20jumb");
        store.extend_from_slice(&[0u8; 4]);
        assert!(matches!(
            collect(&store),
            Err(ClaimError::MalformedJumbf { .. })
        ));
    }
}
