#pragma once

// Opaque handle behind a LADSPA/DSSI instance. The definition lives in
// CaptureInstance.cpp so the format entry points need no JUCE headers (and
// therefore no JUCE sources): only the core static library carries JUCE.
struct snd_seq_event;

namespace apw::shim
{
enum class Format { ladspa, dssi };

// in L, in R, out L, out R
inline constexpr unsigned long portCount = 4;

class CaptureInstance;

// Everything except run() may allocate. create() returns null on failure
// (exceptions must not cross the C boundary).
CaptureInstance* create (Format format, unsigned long sampleRate) noexcept;
void destroy (CaptureInstance* instance) noexcept;
void connectPort (CaptureInstance& instance, unsigned long port, float* data) noexcept;
void activate (CaptureInstance& instance) noexcept;
void deactivate (CaptureInstance& instance) noexcept;

// Real-time safe. events may be null when eventCount is zero. Event times are
// frame offsets from the start of this run (DSSI convention).
void run (CaptureInstance& instance, unsigned long sampleCount,
          const snd_seq_event* events, unsigned long eventCount) noexcept;
}
