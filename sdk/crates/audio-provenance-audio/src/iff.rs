use crate::error::{AudioError, ChunkId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

#[derive(Clone, Copy, Debug)]
pub struct Chunk {
    pub id: ChunkId,
    pub start: usize,
    pub len: usize,
}

/// Walks the top-level chunk table of a RIFF/IFF container.
///
/// Every declared length is checked against what actually remains in the buffer
/// before it is used, so no caller can be handed a range that outruns the file
/// and no length ever reaches an allocation unchecked.
#[derive(Debug)]
pub struct ChunkWalker<'a> {
    bytes: &'a [u8],
    endian: Endian,
    offset: usize,
    container: &'static str,
}

impl<'a> ChunkWalker<'a> {
    /// `bytes` must be the whole container. The 12-byte form header
    /// (`RIFF`/`FORM` + size + form type) is validated by the caller; walking
    /// starts at byte 12.
    pub const fn new(bytes: &'a [u8], endian: Endian, container: &'static str) -> Self {
        Self {
            bytes,
            endian,
            offset: 12,
            container,
        }
    }

    fn read_u32(&self, at: usize) -> Option<u32> {
        let raw: [u8; 4] = self.bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(match self.endian {
            Endian::Little => u32::from_le_bytes(raw),
            Endian::Big => u32::from_be_bytes(raw),
        })
    }

    pub fn next_chunk(&mut self) -> Result<Option<Chunk>, AudioError> {
        let header_end = match self.offset.checked_add(8) {
            Some(end) if end <= self.bytes.len() => end,
            // A trailing fragment shorter than a chunk header ends the walk;
            // writers routinely pad, and refusing here would reject real files.
            _ => return Ok(None),
        };
        let mut id = [0u8; 4];
        id.copy_from_slice(&self.bytes[self.offset..self.offset + 4]);
        let id = ChunkId(id);

        let declared = u64::from(self.read_u32(self.offset + 4).ok_or(
            AudioError::MalformedHeader {
                container: self.container,
                reason: "chunk header truncated",
            },
        )?);
        let available = (self.bytes.len() - header_end) as u64;
        if declared > available {
            return Err(AudioError::TruncatedChunk {
                chunk: id,
                declared,
                available,
            });
        }

        let len = declared as usize;
        let chunk = Chunk {
            id,
            start: header_end,
            len,
        };

        let padded = len + (len & 1);
        self.offset = match header_end.checked_add(padded) {
            Some(next) => next,
            None => {
                return Err(AudioError::MalformedHeader {
                    container: self.container,
                    reason: "chunk table overflows the address space",
                });
            }
        };
        Ok(Some(chunk))
    }

    pub fn payload(&self, chunk: &Chunk) -> Result<&'a [u8], AudioError> {
        let end = chunk
            .start
            .checked_add(chunk.len)
            .ok_or(AudioError::MalformedHeader {
                container: self.container,
                reason: "chunk range overflows the address space",
            })?;
        self.bytes
            .get(chunk.start..end)
            .ok_or(AudioError::TruncatedChunk {
                chunk: chunk.id,
                declared: chunk.len as u64,
                available: (self.bytes.len().saturating_sub(chunk.start)) as u64,
            })
    }
}

/// Validates the 12-byte form header and returns the declared payload size.
pub fn form_header(
    bytes: &[u8],
    magic: &[u8; 4],
    endian: Endian,
    container: &'static str,
) -> Result<u32, AudioError> {
    if bytes.len() < 12 {
        return Err(AudioError::MalformedHeader {
            container,
            reason: "file is shorter than a 12-byte form header",
        });
    }
    if &bytes[0..4] != magic {
        return Err(AudioError::UnsupportedContainer(container));
    }
    let raw: [u8; 4] = match bytes[4..8].try_into() {
        Ok(raw) => raw,
        Err(_) => {
            return Err(AudioError::MalformedHeader {
                container,
                reason: "form size field truncated",
            });
        }
    };
    let declared = match endian {
        Endian::Little => u32::from_le_bytes(raw),
        Endian::Big => u32::from_be_bytes(raw),
    };
    let available = (bytes.len() - 8) as u64;
    if u64::from(declared) > available {
        return Err(AudioError::TruncatedChunk {
            chunk: ChunkId(*magic),
            declared: u64::from(declared),
            available,
        });
    }
    Ok(declared)
}

/// Rejects an AIFF/AIFC whose chunk table does not fit inside the file.
///
/// Used to bound hostile input before it reaches a streaming decoder that would
/// otherwise size its own buffers from the same untrusted fields.
pub fn validate_aiff(bytes: &[u8]) -> Result<(), AudioError> {
    form_header(bytes, b"FORM", Endian::Big, "aiff")?;
    match &bytes[8..12] {
        b"AIFF" | b"AIFC" => {}
        _ => return Err(AudioError::UnsupportedContainer("aiff")),
    }
    let mut walker = ChunkWalker::new(bytes, Endian::Big, "aiff");
    let mut saw_comm = false;
    while let Some(chunk) = walker.next_chunk()? {
        if chunk.id == ChunkId::COMM {
            saw_comm = true;
            if chunk.len < 18 {
                return Err(AudioError::MalformedHeader {
                    container: "aiff",
                    reason: "COMM chunk shorter than 18 bytes",
                });
            }
        }
    }
    if saw_comm {
        Ok(())
    } else {
        Err(AudioError::MissingChunk {
            chunk: ChunkId::COMM,
        })
    }
}
