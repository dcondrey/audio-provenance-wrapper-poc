"""Filesystem observation of sample imports.

Layer 5 of the observation architecture: watches a folder for audio files
arriving and records one `sample_file_observed` event per settled file.
"""

from .watcher import (
    AUDIO_EXTENSIONS,
    SampleWatcher,
    append_event,
    build_sample_file_event,
    is_audio_file,
)

__all__ = [
    "AUDIO_EXTENSIONS",
    "SampleWatcher",
    "append_event",
    "build_sample_file_event",
    "is_audio_file",
]
