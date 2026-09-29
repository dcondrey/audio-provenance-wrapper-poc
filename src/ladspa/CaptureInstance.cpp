#include "CaptureInstance.h"

#include "PluginProcessor.h"

#include <alsa/seq_event.h>

#include <array>
#include <memory>
#include <cstring>
#include <mutex>

namespace apw::shim
{
namespace
{
// Frames handed to the processor per processBlock call. LADSPA has no maximum
// block size, so run() is split into pieces of at most this length and the
// processor is prepared for exactly this size.
constexpr int maxBlockFrames = 8192;

// Upper bound on MIDI events forwarded per processed block. The MidiBuffer is
// reserved for this many short messages up front so the run path never grows
// it: each stored event is a 4-byte position, a 2-byte size and <= 3 data bytes.
constexpr int maxMidiEventsPerBlock = 1024;
constexpr std::size_t midiBytesPerEvent = 4 + 2 + 3;

// JUCE needs its process-wide singletons (MessageManager, leak detector,
// SharedResourcePointers) alive while any instance exists. Hosts create and
// destroy instances on arbitrary threads and LADSPA has no library init hook,
// so a counted initialiser is taken per instance and dropped with the last one.
std::mutex juceLifetimeMutex;
int liveInstances = 0;
std::unique_ptr<juce::ScopedJuceInitialiser_GUI> juceLifetime;

void retainJuce()
{
    std::lock_guard<std::mutex> guard (juceLifetimeMutex);
    if (liveInstances++ == 0)
        juceLifetime = std::make_unique<juce::ScopedJuceInitialiser_GUI>();
}

void releaseJuce() noexcept
{
    std::lock_guard<std::mutex> guard (juceLifetimeMutex);
    if (--liveInstances == 0)
        juceLifetime.reset();
}

const char* formatName (Format format) noexcept
{
    return format == Format::ladspa ? "LADSPA" : "DSSI";
}

bool overlaps (const float* a, const float* b, std::size_t frames) noexcept
{
    return a < b + frames && b < a + frames;
}

// Maps a DSSI/ALSA sequencer event to a short MIDI message. Returns false for
// event types the capture does not observe (DSSI hosts must not send NOTE,
// bank or program changes through run_synth).
bool toMidi (const snd_seq_event_t& event, juce::MidiMessage& out) noexcept
{
    const auto channel = static_cast<int> ((event.type == SND_SEQ_EVENT_NOTEON
                                            || event.type == SND_SEQ_EVENT_NOTEOFF
                                            || event.type == SND_SEQ_EVENT_KEYPRESS)
                                               ? event.data.note.channel
                                               : event.data.control.channel) & 0x0f;
    const auto clamp7 = [] (int v) { return juce::jlimit (0, 127, v); };

    switch (event.type)
    {
        case SND_SEQ_EVENT_NOTEON:
            out = juce::MidiMessage (0x90 | channel, clamp7 (event.data.note.note),
                                     clamp7 (event.data.note.velocity));
            return true;
        case SND_SEQ_EVENT_NOTEOFF:
            out = juce::MidiMessage (0x80 | channel, clamp7 (event.data.note.note),
                                     clamp7 (event.data.note.velocity));
            return true;
        case SND_SEQ_EVENT_KEYPRESS:
            out = juce::MidiMessage (0xa0 | channel, clamp7 (event.data.note.note),
                                     clamp7 (event.data.note.velocity));
            return true;
        case SND_SEQ_EVENT_CONTROLLER:
            out = juce::MidiMessage (0xb0 | channel, clamp7 (static_cast<int> (event.data.control.param)),
                                     clamp7 (event.data.control.value));
            return true;
        case SND_SEQ_EVENT_CHANPRESS:
            out = juce::MidiMessage (0xd0 | channel, clamp7 (event.data.control.value));
            return true;
        case SND_SEQ_EVENT_PITCHBEND:
        {
            // ALSA carries pitch bend as -8192..8191; MIDI wants 0..16383.
            const auto value = juce::jlimit (0, 16383, event.data.control.value + 8192);
            out = juce::MidiMessage (0xe0 | channel, value & 0x7f, (value >> 7) & 0x7f);
            return true;
        }
        default:
            return false;
    }
}
}

// One instance of the capture processor: stereo in, stereo out.
class CaptureInstance
{
public:
    CaptureInstance (Format format, double sampleRate);
    ~CaptureInstance();

    void connectPort (unsigned long port, float* data) noexcept;
    void activate() noexcept;
    void deactivate() noexcept;
    void run (unsigned long sampleCount, const snd_seq_event_t* events,
              unsigned long eventCount) noexcept;

private:
    void prepare();
    void processChunk (unsigned long offset, int frames, const snd_seq_event_t* events,
                       unsigned long eventCount, unsigned long runFrames) noexcept;

    std::unique_ptr<AudioProvenanceCaptureAudioProcessor> processor;
    std::array<float*, portCount> ports {};
    std::array<std::array<float, maxBlockFrames>, 2> stage {};
    juce::MidiBuffer midi;
    double sampleRate;
    bool prepared = false;
};


CaptureInstance* create (Format format, unsigned long sampleRate) noexcept
{
    if (sampleRate == 0)
        return nullptr;

    try
    {
        retainJuce();
        try
        {
            return new CaptureInstance (format, static_cast<double> (sampleRate));
        }
        catch (...)
        {
            releaseJuce();
            return nullptr;
        }
    }
    catch (...)
    {
        return nullptr;
    }
}

void destroy (CaptureInstance* instance) noexcept
{
    if (instance == nullptr)
        return;

    try
    {
        delete instance;
        releaseJuce();
    }
    catch (...)
    {
    }
}

// Session identity (plugin_instance_id, plugin_capture_session_id) is generated
// by the processor constructor: LADSPA and DSSI define no state save, so an
// instance is a new session every time it is instantiated.
CaptureInstance::CaptureInstance (Format format, double rate)
    : processor (std::make_unique<AudioProvenanceCaptureAudioProcessor> (9876, formatName (format))),
      sampleRate (rate)
{
    // Reserved once so addEvent never reallocates on the run path.
    midi.ensureSize (static_cast<size_t> (maxMidiEventsPerBlock) * midiBytesPerEvent);
    // Activation is optional in LADSPA, so a plain instantiate must be runnable.
    prepare();
}

CaptureInstance::~CaptureInstance()
{
    if (prepared)
        processor->releaseResources();
}

void CaptureInstance::prepare()
{
    processor->setRateAndBufferSizeDetails (sampleRate, maxBlockFrames);
    processor->prepareToPlay (sampleRate, maxBlockFrames);
    prepared = true;
}

void CaptureInstance::connectPort (unsigned long port, float* data) noexcept
{
    if (port < portCount)
        ports[port] = data;
}

void CaptureInstance::activate() noexcept
{
    try
    {
        if (! prepared)
            prepare();
        else
            processor->reset();
    }
    catch (...)
    {
    }
}

void CaptureInstance::deactivate() noexcept
{
    if (prepared)
    {
        processor->releaseResources();
        prepared = false;
    }
}

void CaptureInstance::run (unsigned long sampleCount,
                           const snd_seq_event_t* events,
                           unsigned long eventCount) noexcept
{
    if (sampleCount == 0)
        return;

    for (unsigned long offset = 0; offset < sampleCount;)
    {
        const auto frames = static_cast<int> (
            sampleCount - offset < static_cast<unsigned long> (maxBlockFrames)
                ? sampleCount - offset
                : static_cast<unsigned long> (maxBlockFrames));
        processChunk (offset, frames, events, eventCount, sampleCount);
        offset += static_cast<unsigned long> (frames);
    }
}

void CaptureInstance::processChunk (unsigned long offset, int frames,
                                    const snd_seq_event_t* events, unsigned long eventCount,
                                    unsigned long runFrames) noexcept
{
    const auto n = static_cast<std::size_t> (frames);
    const float* in[2] = { ports[0] != nullptr ? ports[0] + offset : nullptr,
                           ports[1] != nullptr ? ports[1] + offset : nullptr };
    float* out[2] = { ports[2] != nullptr ? ports[2] + offset : nullptr,
                      ports[3] != nullptr ? ports[3] + offset : nullptr };

    // Unconnected ports: nothing can be observed or delivered, so do what the
    // connected ones allow and skip the capture.
    if (in[0] == nullptr || in[1] == nullptr || out[0] == nullptr || out[1] == nullptr || ! prepared)
    {
        for (int c = 0; c < 2; ++c)
        {
            if (out[c] == nullptr)
                continue;
            if (in[c] != nullptr)
                std::memmove (out[c], in[c], n * sizeof (float));
            else
                std::memset (out[c], 0, n * sizeof (float));
        }
        return;
    }

    // An input that overlaps a different output channel (swapped or shared
    // buffers) would be overwritten before it is read, so read both inputs first.
    bool crossAliased = false;
    for (int c = 0; c < 2; ++c)
        for (int j = 0; j < 2; ++j)
            if (overlaps (in[c], out[j], n) && ! (c == j && in[c] == out[j]))
                crossAliased = true;
    if (crossAliased)
    {
        std::memcpy (stage[0].data(), in[0], n * sizeof (float));
        std::memcpy (stage[1].data(), in[1], n * sizeof (float));
        std::memcpy (out[0], stage[0].data(), n * sizeof (float));
        std::memcpy (out[1], stage[1].data(), n * sizeof (float));
    }
    else
    {
        if (in[0] != out[0])
            std::memcpy (out[0], in[0], n * sizeof (float));
        if (in[1] != out[1])
            std::memcpy (out[1], in[1], n * sizeof (float));
    }

    midi.clear();
    if (events != nullptr)
    {
        int forwarded = 0;
        for (unsigned long i = 0; i < eventCount && forwarded < maxMidiEventsPerBlock; ++i)
        {
            // Times are frame offsets from the start of the run; late events
            // are pinned to its final frame, which lies in the last chunk.
            auto time = static_cast<unsigned long> (events[i].time.tick);
            if (time >= runFrames)
                time = runFrames - 1;
            if (time < offset || time >= offset + static_cast<unsigned long> (frames))
                continue;
            const auto position = time - offset;

            juce::MidiMessage message;
            if (toMidi (events[i], message))
            {
                midi.addEvent (message, static_cast<int> (position));
                ++forwarded;
            }
        }
    }

    // Wraps the output memory; ChannelPointerStorage is in-object for 2 channels.
    float* channels[2] = { out[0], out[1] };
    juce::AudioBuffer<float> buffer (channels, 2, frames);
    processor->processBlock (buffer, midi);
}

void connectPort (CaptureInstance& instance, unsigned long port, float* data) noexcept
{
    instance.connectPort (port, data);
}

void activate (CaptureInstance& instance) noexcept { instance.activate(); }
void deactivate (CaptureInstance& instance) noexcept { instance.deactivate(); }

void run (CaptureInstance& instance, unsigned long sampleCount,
          const snd_seq_event* events, unsigned long eventCount) noexcept
{
    instance.run (sampleCount, events, eventCount);
}
}
