//! Writing the `aprv` provenance chunk into a RIFF/WAVE file.
//!
//! Container surgery, not provenance logic: what goes IN the chunk is decided entirely by
//! `audio-provenance-manifest`, and the byte layout is `genotrace::container`'s, which also reads it.

use genotrace::container::write_riff_chunk;

use crate::error::CliError;

/// Replaces any existing top-level `aprv` chunk with `payload`.
pub fn set_manifest_chunk(wav: &[u8], payload: &[u8]) -> Result<Vec<u8>, CliError> {
    write_riff_chunk(wav, payload).map_err(|error| {
        CliError::usage(match error {
            genotrace::container::ChunkWriteError::NotRiffWave => {
                "--embed writes a RIFF/WAVE `aprv` chunk and this file is not RIFF/WAVE; use a sidecar"
            }
            other => other.reason(),
        })
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use genotrace::container::{EmbeddedOutcome, find_embedded};
    use genotrace::ingest::Container;

    fn wav_with(data: &[u8]) -> Vec<u8> {
        let mut out = b"RIFF\0\0\0\0WAVE".to_vec();
        out.extend_from_slice(b"data");
        out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
        let size = u32::try_from(out.len() - 8).unwrap();
        out.splice(4..8, size.to_le_bytes());
        out
    }

    /// The writer and `genotrace::container`'s reader are two halves of one format. An odd-length
    /// body is the case where a missing pad byte silently shifts every later chunk, so the
    /// round-trip is pinned there and a second write must replace, never duplicate.
    #[test]
    fn an_odd_length_body_round_trips_and_a_second_write_replaces_the_first() {
        let source = wav_with(&[1, 2, 3, 4, 5]);

        let first = set_manifest_chunk(&source, b"{\"a\":1}").unwrap();
        assert_eq!(
            find_embedded(&first, Container::Wav),
            EmbeddedOutcome::Found(b"{\"a\":1}".to_vec())
        );

        let second = set_manifest_chunk(&first, b"{\"b\":2}").unwrap();
        assert_eq!(
            find_embedded(&second, Container::Wav),
            EmbeddedOutcome::Found(b"{\"b\":2}".to_vec())
        );
        assert_eq!(
            second.windows(4).filter(|w| *w == b"aprv").count(),
            1,
            "the second write duplicated the chunk instead of replacing it"
        );
    }

    #[test]
    fn a_non_riff_input_is_refused_rather_than_wrapped() {
        assert!(set_manifest_chunk(b"OggS.............", b"{}").is_err());
        assert!(set_manifest_chunk(&wav_with(&[1, 2]), b"").is_err());
    }
}
