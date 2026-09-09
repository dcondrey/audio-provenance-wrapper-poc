#!/bin/sh
set -eu

workspace=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$workspace"

cargo test -p apw_trace --lib
cargo test -p apw_trace --test qualification_manifest
cargo test -p apw_trace --test fingerprint interior_substitution_cannot_hide_between_aligned_edges
cargo test -p apw_trace --test soft_mark_locator
cargo test -p apw_watermark --test properties playback_rate_drift_decodes_across_the_searched_range
cargo test -p audio-provenance-cli --test brief_invocation a_marked_signature_binds_the_marked_audio_and_the_mark_survives_into_the_file
