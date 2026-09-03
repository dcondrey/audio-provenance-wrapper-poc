//! Writing a manifest into a WAV's `aprv` provenance slot.

use genotrace::container::write_riff_chunk;

use crate::error::SdkError;

/// Returns a copy of `wav` carrying `manifest` in its `aprv` chunk.
///
/// IMPORTANT: the file bytes change, so the manifest's own `content_sha256` can never recompute
/// over the result. That is not a defect to be hidden: the manifest's `decoded_audio_sha256` still
/// recomputes exactly, GenoTrace reports the binding as `hard_exact_decoded_audio_only`, and it
/// attaches a `container_bytes_changed` finding saying the container was rewritten and no sample
/// moved. A caller that needs `content_sha256` to be the binding must use a sidecar instead.
pub(crate) fn with_manifest_chunk(
    wav: &[u8],
    manifest: &[u8],
    path: &str,
) -> Result<Vec<u8>, SdkError> {
    write_riff_chunk(wav, manifest).map_err(|error| SdkError::MalformedRiff {
        path: path.to_string(),
        reason: error.reason(),
    })
}
