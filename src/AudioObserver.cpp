#include "AudioObserver.h"
#include "ObservationEvent.h"

#include <juce_cryptography/juce_cryptography.h>
#include <cmath>

namespace apw
{

AudioObserver::AudioObserver()
    : juce::Thread ("AudioObserver"),
      audioFifo (kFifoCapacity),
      audioFifoBuffer (static_cast<size_t> (kFifoCapacity), 0.0f),
      windowBuffer (static_cast<size_t> (kWindowSize), 0.0f),
      midiFifo (kMidiQueueSize),
      midiFifoBuffer (static_cast<size_t> (kMidiQueueSize)),
      fft (kFFTOrder),
      fftWorkspace (static_cast<size_t> (kWindowSize * 2), 0.0f)
{
}

AudioObserver::~AudioObserver()
{
    stop();
}

void AudioObserver::start (EventCallback callback)
{
    eventCallback = std::move (callback);
    startThread (juce::Thread::Priority::normal);
}

void AudioObserver::stop()
{
    signalThreadShouldExit();
    stopThread (2000);
}

// ──────────────────────────────────────────────────────────────────────
// Real-time thread interface
// ──────────────────────────────────────────────────────────────────────

void AudioObserver::pushAudioBlock (const float* const* channelData,
                                     int numChannels, int numSamples)
{
    if (numChannels <= 0 || numSamples <= 0)
        return;

    buffersSubmitted.fetch_add (1, std::memory_order_relaxed);
    samplesSubmitted.fetch_add (static_cast<std::uint64_t> (numSamples), std::memory_order_relaxed);

    int start1, size1, start2, size2;
    audioFifo.prepareToWrite (numSamples, start1, size1, start2, size2);

    const float gain = 1.0f / static_cast<float> (numChannels);

    auto writeMono = [&] (int destStart, int count, int srcOffset)
    {
        for (int i = 0; i < count; ++i)
        {
            float sum = 0.0f;
            for (int ch = 0; ch < numChannels; ++ch)
                sum += channelData[ch][srcOffset + i];
            audioFifoBuffer[static_cast<size_t> (destStart + i)] = sum * gain;
        }
    };

    if (size1 > 0) writeMono (start1, size1, 0);
    if (size2 > 0) writeMono (start2, size2, size1);

    const auto written = size1 + size2;
    audioFifo.finishedWrite (written);
    const auto dropped = numSamples - written;
    if (dropped > 0)
    {
        fifoSamplesDropped.fetch_add (static_cast<std::uint64_t> (dropped), std::memory_order_relaxed);
        fifoWindowsDropped.fetch_add (1, std::memory_order_relaxed);
    }
}

void AudioObserver::recordExternalAudioDrop (int numSamples) noexcept
{
    if (numSamples <= 0)
        return;
    buffersSubmitted.fetch_add (1, std::memory_order_relaxed);
    samplesSubmitted.fetch_add (static_cast<std::uint64_t> (numSamples), std::memory_order_relaxed);
    fifoSamplesDropped.fetch_add (static_cast<std::uint64_t> (numSamples), std::memory_order_relaxed);
    fifoWindowsDropped.fetch_add (1, std::memory_order_relaxed);
}

void AudioObserver::pushMidiMessages (const juce::MidiBuffer& midi)
{
    for (const auto metadata : midi)
    {
        const auto msg = metadata.getMessage();
        MidiRecord record {};

        if (msg.isNoteOn())
        {
            record.type  = 0x90;
            record.data1 = static_cast<std::uint8_t> (msg.getNoteNumber());
            record.data2 = static_cast<std::uint8_t> (msg.getVelocity());
        }
        else if (msg.isNoteOff())
        {
            record.type  = 0x80;
            record.data1 = static_cast<std::uint8_t> (msg.getNoteNumber());
            record.data2 = static_cast<std::uint8_t> (msg.getVelocity());
        }
        else if (msg.isController())
        {
            record.type  = 0xB0;
            record.data1 = static_cast<std::uint8_t> (msg.getControllerNumber());
            record.data2 = static_cast<std::uint8_t> (msg.getControllerValue());
        }
        else if (msg.isProgramChange())
        {
            record.type  = 0xC0;
            record.data1 = static_cast<std::uint8_t> (msg.getProgramChangeNumber());
            record.data2 = 0;
        }
        else
        {
            continue;
        }

        record.channel = static_cast<std::uint8_t> (msg.getChannel());

        int s1, sz1, s2, sz2;
        midiFifo.prepareToWrite (1, s1, sz1, s2, sz2);
        if (sz1 > 0)
            midiFifoBuffer[static_cast<size_t> (s1)] = record;
        else
            midiEventsDropped.fetch_add (1, std::memory_order_relaxed);
        midiFifo.finishedWrite (sz1 + sz2);
    }
}

void AudioObserver::updateTransportState (juce::AudioPlayHead* playHead)
{
    if (playHead == nullptr)
        return;

    if (auto pos = playHead->getPosition())
    {
        transportPlaying.store (pos->getIsPlaying(), std::memory_order_relaxed);
        transportRecording.store (pos->getIsRecording(), std::memory_order_relaxed);
        transportLooping.store (pos->getIsLooping(), std::memory_order_relaxed);

        if (auto samples = pos->getTimeInSamples())
            transportSamplePos.store (static_cast<juce::int64> (*samples),
                                       std::memory_order_relaxed);

        if (auto bpm = pos->getBpm())
            transportBpmX100.store (static_cast<int> (*bpm * 100.0),
                                     std::memory_order_relaxed);
    }
}

void AudioObserver::updateSessionConfig (int sampleRate, int channelCount, int bufferSize)
{
    sessionSampleRate.store (sampleRate, std::memory_order_relaxed);
    sessionChannelCount.store (channelCount, std::memory_order_relaxed);
    sessionBufferSize.store (bufferSize, std::memory_order_relaxed);
}

// ──────────────────────────────────────────────────────────────────────
// Background observation thread
// ──────────────────────────────────────────────────────────────────────

void AudioObserver::run()
{
    while (! threadShouldExit())
    {
        while (audioFifo.getNumReady() >= kWindowSize && ! threadShouldExit())
        {
            int start1, size1, start2, size2;
            audioFifo.prepareToRead (kWindowSize, start1, size1, start2, size2);

            if (size1 > 0)
                std::copy (audioFifoBuffer.begin() + start1,
                           audioFifoBuffer.begin() + start1 + size1,
                           windowBuffer.begin());
            if (size2 > 0)
                std::copy (audioFifoBuffer.begin() + start2,
                           audioFifoBuffer.begin() + start2 + size2,
                           windowBuffer.begin() + size1);

            audioFifo.finishedRead (size1 + size2);

            processWindow (windowBuffer.data(), kWindowSize);
        }

        drainMidiEvents();
        checkTransportChanges();
        checkSessionConfigChanges();

        juce::Thread::sleep (5);
    }
}

void AudioObserver::processWindow (const float* data, int numSamples)
{
    const auto timestampMs = static_cast<std::uint64_t> (
        juce::Time::getMillisecondCounterHiRes());
    const auto samplePos = transportSamplePos.load (std::memory_order_relaxed);
    const auto sampleRate = sessionSampleRate.load (std::memory_order_relaxed);
    const auto channels   = sessionChannelCount.load (std::memory_order_relaxed);
    const auto bpmX100    = transportBpmX100.load (std::memory_order_relaxed);

    const auto rms      = computeRMS (data, numSamples);
    const auto zcr      = computeZeroCrossingRate (data, numSamples);
    const auto centroid = computeSpectralCentroid (data, numSamples);
    const bool hasAudio = rms > kSilenceThreshold;
    double peak = 0.0;
    for (int i = 0; i < numSamples; ++i)
        peak = std::max (peak, std::abs (static_cast<double> (data[i])));
    const auto crestFactor = rms > 1.0e-9 ? peak / rms : 0.0;
    juce::Array<juce::var> energyEnvelope;
    constexpr int envelopeSegments = 4;
    for (int segment = 0; segment < envelopeSegments; ++segment)
    {
        const auto start = segment * numSamples / envelopeSegments;
        const auto end = (segment + 1) * numSamples / envelopeSegments;
        const auto segmentRms = computeRMS (data + start, end - start);
        energyEnvelope.add (rms > 1.0e-9 ? segmentRms / rms : 0.0);
    }

    // Spectral band profile (uses fftWorkspace already populated by centroid).
    const auto bands = computeSpectralBands (numSamples);

    auto windowHash = computeChainedHash (data, numSamples);
    auto prevHash   = previousHash.isEmpty() ? juce::String ("genesis") : previousHash;
    totalWindowsHashed.fetch_add (1, std::memory_order_relaxed);

    // ── buffer_hash event ──
    // Every window is emitted so the daemon can verify the complete chain.
    // At 48 kHz this is about twelve compact events per second.
    if (eventCallback)
    {
        auto json = buildJsonEvent (EventTypes::bufferHash, timestampMs, samplePos,
        {
            { "window_hash",          windowHash },
            { "prev_hash",            prevHash },
            { "rms_level",            rms },
            { "zero_crossing_rate",   zcr },
            { "crest_factor",         crestFactor },
            { "energy_envelope",      juce::var (energyEnvelope) },
            { "spectral_centroid_hz", centroid },
            { "channel_count",        channels },
            { "sample_rate_hz",       sampleRate },
            { "window_size_samples",  numSamples },
            { "bpm",                  bpmX100 / 100.0 },
            { "silent_windows_skipped", 0 },
            { "band_low",  bands.low },
            { "band_mid",  bands.mid },
            { "band_high", bands.high }
        });
        eventCallback (json);
        totalEventsEmitted.fetch_add (1, std::memory_order_relaxed);
    }

    // ── audio_transition event ──
    if (hasAudio != prevWindowHadAudio)
    {
        auto direction = hasAudio ? juce::String ("silence_to_audio")
                                  : juce::String ("audio_to_silence");

        if (eventCallback)
        {
            auto json = buildJsonEvent (EventTypes::audioTransition, timestampMs, samplePos,
            {
                { "direction",     direction },
                { "boundary_hash", windowHash }
            });
            eventCallback (json);
            totalEventsEmitted.fetch_add (1, std::memory_order_relaxed);
        }
    }

    // ── spectral_shift event ──
    if (hasAudio && prevWindowHadAudio
        && std::abs (centroid - prevSpectralCentroid) > kSpectralShiftThreshold)
    {
        if (eventCallback)
        {
            auto json = buildJsonEvent (EventTypes::spectralShift, timestampMs, samplePos,
            {
                { "prev_spectral_centroid_hz", prevSpectralCentroid },
                { "new_spectral_centroid_hz",  centroid },
                { "shift_magnitude",           std::abs (centroid - prevSpectralCentroid) }
            });
            eventCallback (json);
            totalEventsEmitted.fetch_add (1, std::memory_order_relaxed);
        }
    }

    // ── spectral_profile_change event ──
    // Detects EQ/filter/effect changes: band ratio shifted significantly
    // but audio didn't stop (distinguishes from content change).
    if (hasAudio && prevWindowHadAudio)
    {
        const double dLow  = std::abs (bands.low  - prevBands.low);
        const double dMid  = std::abs (bands.mid  - prevBands.mid);
        const double dHigh = std::abs (bands.high - prevBands.high);
        const double maxDelta = std::max ({ dLow, dMid, dHigh });

        if (maxDelta > kBandShiftThreshold && eventCallback)
        {
            auto json = buildJsonEvent ("spectral_profile_change", timestampMs, samplePos,
            {
                { "band_low_delta",  dLow },
                { "band_mid_delta",  dMid },
                { "band_high_delta", dHigh },
                { "band_low",  bands.low },
                { "band_mid",  bands.mid },
                { "band_high", bands.high },
                { "prev_band_low",  prevBands.low },
                { "prev_band_mid",  prevBands.mid },
                { "prev_band_high", prevBands.high }
            });
            eventCallback (json);
            totalEventsEmitted.fetch_add (1, std::memory_order_relaxed);
        }
    }

    previousHash = windowHash;
    prevWindowHadAudio = hasAudio;
    prevBands = bands;
    if (hasAudio)
        prevSpectralCentroid = centroid;

    {
        juce::SpinLock::ScopedLockType lock (lastHashLock);
        lastHashHex = windowHash;
    }
}

// ──────────────────────────────────────────────────────────────────────
// Feature computation
// ──────────────────────────────────────────────────────────────────────

juce::String AudioObserver::computeChainedHash (const float* data, int numSamples)
{
    juce::MemoryOutputStream mos;

    auto prev = previousHash.isEmpty() ? juce::String ("genesis") : previousHash;
    mos.write (prev.toRawUTF8(), prev.getNumBytesAsUTF8());
    mos.write (data, static_cast<size_t> (numSamples) * sizeof (float));

    juce::SHA256 hash (mos.getMemoryBlock());
    return hash.toHexString();
}

double AudioObserver::computeRMS (const float* data, int numSamples)
{
    double sum = 0.0;
    for (int i = 0; i < numSamples; ++i)
        sum += static_cast<double> (data[i]) * static_cast<double> (data[i]);
    return std::sqrt (sum / static_cast<double> (numSamples));
}

double AudioObserver::computeZeroCrossingRate (const float* data, int numSamples)
{
    if (numSamples < 2)
        return 0.0;

    int crossings = 0;
    for (int i = 1; i < numSamples; ++i)
    {
        if ((data[i] >= 0.0f) != (data[i - 1] >= 0.0f))
            ++crossings;
    }
    return static_cast<double> (crossings) / static_cast<double> (numSamples - 1);
}

double AudioObserver::computeSpectralCentroid (const float* data, int numSamples)
{
    const auto sr = sessionSampleRate.load (std::memory_order_relaxed);
    if (sr <= 0 || numSamples <= 0)
        return 0.0;

    std::fill (fftWorkspace.begin(), fftWorkspace.end(), 0.0f);

    for (int i = 0; i < numSamples; ++i)
    {
        const float w = 0.5f * (1.0f - std::cos (2.0f * juce::MathConstants<float>::pi
                                                   * static_cast<float> (i)
                                                   / static_cast<float> (numSamples - 1)));
        fftWorkspace[static_cast<size_t> (i)] = data[i] * w;
    }

    fft.performFrequencyOnlyForwardTransform (fftWorkspace.data());

    double weightedSum  = 0.0;
    double magnitudeSum = 0.0;
    const int numBins   = numSamples / 2;
    const double binHz  = static_cast<double> (sr) / static_cast<double> (numSamples);

    for (int i = 1; i < numBins; ++i)
    {
        const double freq = static_cast<double> (i) * binHz;
        const double mag  = static_cast<double> (fftWorkspace[static_cast<size_t> (i)]);
        weightedSum  += freq * mag;
        magnitudeSum += mag;
    }

    return magnitudeSum > 0.0 ? weightedSum / magnitudeSum : 0.0;
}

AudioObserver::SpectralBands AudioObserver::computeSpectralBands (int numSamples)
{
    // Assumes fftWorkspace already contains magnitude data from
    // computeSpectralCentroid (called immediately before this).
    const auto sr = sessionSampleRate.load (std::memory_order_relaxed);
    if (sr <= 0 || numSamples <= 0)
        return {};

    const int numBins  = numSamples / 2;
    const double binHz = static_cast<double> (sr) / static_cast<double> (numSamples);

    // Band boundaries: low 0-300 Hz, mid 300-4000 Hz, high 4000+ Hz.
    const int lowCutBin  = std::min (static_cast<int> (300.0 / binHz),  numBins);
    const int midCutBin  = std::min (static_cast<int> (4000.0 / binHz), numBins);

    double lowEnergy  = 0.0;
    double midEnergy  = 0.0;
    double highEnergy = 0.0;

    for (int i = 1; i < numBins; ++i)
    {
        const double mag = static_cast<double> (fftWorkspace[static_cast<size_t> (i)]);
        const double e   = mag * mag;
        if (i < lowCutBin)       lowEnergy  += e;
        else if (i < midCutBin)  midEnergy  += e;
        else                     highEnergy += e;
    }

    const double total = lowEnergy + midEnergy + highEnergy;
    if (total <= 0.0)
        return {};

    return { lowEnergy / total, midEnergy / total, highEnergy / total };
}

// ──────────────────────────────────────────────────────────────────────
// MIDI / transport / config change detection
// ──────────────────────────────────────────────────────────────────────

void AudioObserver::drainMidiEvents()
{
    const auto timestampMs = static_cast<std::uint64_t> (
        juce::Time::getMillisecondCounterHiRes());
    const auto samplePos = transportSamplePos.load (std::memory_order_relaxed);

    while (midiFifo.getNumReady() > 0)
    {
        int start1, size1, start2, size2;
        midiFifo.prepareToRead (1, start1, size1, start2, size2);

        if (size1 > 0)
        {
            const auto& rec = midiFifoBuffer[static_cast<size_t> (start1)];

            // ── CC knob-turn aggregation ──
            // Instead of emitting every CC message, detect rapid sequential
            // changes on the same controller (= user turning a knob) and
            // emit a single "parameter_change" event with start/end values.
            if (rec.type == 0xB0 && rec.data1 < kMaxCCTracked)
            {
                auto& cc = ccStates[rec.data1];
                if (cc.lastValue < 0)
                {
                    cc.firstValue   = rec.data2;
                    cc.lastValue    = rec.data2;
                    cc.channel      = rec.channel;
                    cc.changeCount  = 1;
                    cc.firstChangeMs = timestampMs;
                    cc.lastChangeMs  = timestampMs;
                }
                else if (timestampMs - cc.lastChangeMs < kKnobTurnWindowMs)
                {
                    cc.lastValue   = rec.data2;
                    cc.changeCount += 1;
                    cc.lastChangeMs = timestampMs;
                }
                else
                {
                    // Window expired: flush previous knob turn if it qualifies.
                    if (cc.changeCount >= kKnobTurnMinChanges && eventCallback)
                    {
                        auto json = buildJsonEvent ("parameter_change", cc.firstChangeMs, samplePos,
                        {
                            { "midi_channel",    static_cast<int> (cc.channel) },
                            { "cc_number",       static_cast<int> (rec.data1) },
                            { "start_value",     cc.firstValue },
                            { "end_value",       cc.lastValue },
                            { "change_count",    cc.changeCount },
                            { "duration_ms",     static_cast<int> (cc.lastChangeMs - cc.firstChangeMs) }
                        });
                        eventCallback (json);
                        totalEventsEmitted.fetch_add (1, std::memory_order_relaxed);
                    }
                    // Start new tracking window.
                    cc.firstValue    = rec.data2;
                    cc.lastValue     = rec.data2;
                    cc.channel       = rec.channel;
                    cc.changeCount   = 1;
                    cc.firstChangeMs = timestampMs;
                    cc.lastChangeMs  = timestampMs;
                }

                midiFifo.finishedRead (size1 + size2);
                continue;
            }

            // ── Non-CC MIDI events: emit individually ──
            juce::String midiType;
            switch (rec.type)
            {
                case 0x90: midiType = "note_on";         break;
                case 0x80: midiType = "note_off";        break;
                case 0xC0: midiType = "program_change";  break;
                default:   midiType = "unknown";          break;
            }

            if (eventCallback)
            {
                auto json = buildJsonEvent (EventTypes::midiEvent, timestampMs, samplePos,
                {
                    { "midi_event_type", midiType },
                    { "midi_channel",    static_cast<int> (rec.channel) },
                    { "midi_note_or_cc", static_cast<int> (rec.data1) },
                    { "midi_value",      static_cast<int> (rec.data2) }
                });
                eventCallback (json);
                totalEventsEmitted.fetch_add (1, std::memory_order_relaxed);
            }
        }

        midiFifo.finishedRead (size1 + size2);
    }

    // ── Flush any in-progress knob turns that have gone stale ──
    for (int cc = 0; cc < kMaxCCTracked; ++cc)
    {
        auto& state = ccStates[cc];
        if (state.changeCount >= kKnobTurnMinChanges
            && timestampMs - state.lastChangeMs >= kKnobTurnWindowMs
            && eventCallback)
        {
            auto json = buildJsonEvent ("parameter_change", state.firstChangeMs, samplePos,
            {
                { "midi_channel",    static_cast<int> (state.channel) },
                { "cc_number",       cc },
                { "start_value",     state.firstValue },
                { "end_value",       state.lastValue },
                { "change_count",    state.changeCount },
                { "duration_ms",     static_cast<int> (state.lastChangeMs - state.firstChangeMs) }
            });
            eventCallback (json);
            totalEventsEmitted.fetch_add (1, std::memory_order_relaxed);
        }
        if (state.changeCount > 0 && timestampMs - state.lastChangeMs >= kKnobTurnWindowMs)
        {
            state.firstValue   = -1;
            state.lastValue    = -1;
            state.channel      = 0;
            state.changeCount  = 0;
            state.firstChangeMs = 0;
            state.lastChangeMs  = 0;
        }
    }
}

void AudioObserver::checkTransportChanges()
{
    const bool playing   = transportPlaying.load (std::memory_order_relaxed);
    const bool recording = transportRecording.load (std::memory_order_relaxed);
    const bool looping   = transportLooping.load (std::memory_order_relaxed);
    const int  bpmX100   = transportBpmX100.load (std::memory_order_relaxed);

    if (playing == prevTransportPlaying && recording == prevTransportRecording
        && looping == prevTransportLooping && bpmX100 == prevTransportBpmX100)
        return;

    const auto timestampMs = static_cast<std::uint64_t> (
        juce::Time::getMillisecondCounterHiRes());
    const auto samplePos = transportSamplePos.load (std::memory_order_relaxed);

    juce::String state = recording ? "recording" : (playing ? "playing" : "stopped");

    if (eventCallback)
    {
        auto json = buildJsonEvent (EventTypes::transportChange, timestampMs, samplePos,
        {
            { "transport_state", state },
            { "is_looping",      looping },
            { "bpm",             bpmX100 / 100.0 }
        });
        eventCallback (json);
        totalEventsEmitted.fetch_add (1, std::memory_order_relaxed);
    }

    prevTransportPlaying   = playing;
    prevTransportRecording = recording;
    prevTransportLooping   = looping;
    prevTransportBpmX100   = bpmX100;
}

void AudioObserver::checkSessionConfigChanges()
{
    const int sr = sessionSampleRate.load (std::memory_order_relaxed);
    const int ch = sessionChannelCount.load (std::memory_order_relaxed);

    if (sr == prevSessionSampleRate && ch == prevSessionChannelCount)
        return;

    const auto timestampMs = static_cast<std::uint64_t> (
        juce::Time::getMillisecondCounterHiRes());
    const auto samplePos = transportSamplePos.load (std::memory_order_relaxed);
    const auto bpmX100   = transportBpmX100.load (std::memory_order_relaxed);

    if (eventCallback)
    {
        auto json = buildJsonEvent (EventTypes::sessionConfig, timestampMs, samplePos,
        {
            { "sample_rate_hz", sr },
            { "channel_count",  ch },
            { "bpm",            bpmX100 / 100.0 }
        });
        eventCallback (json);
        totalEventsEmitted.fetch_add (1, std::memory_order_relaxed);
    }

    prevSessionSampleRate   = sr;
    prevSessionChannelCount = ch;
}

// ──────────────────────────────────────────────────────────────────────
// Stats
// ──────────────────────────────────────────────────────────────────────

int AudioObserver::getWindowSize() const noexcept { return kWindowSize; }
std::uint64_t AudioObserver::getTotalWindowsHashed() const noexcept { return totalWindowsHashed.load (std::memory_order_relaxed); }
std::uint64_t AudioObserver::getTotalEventsEmitted() const noexcept { return totalEventsEmitted.load (std::memory_order_relaxed); }
std::uint64_t AudioObserver::getBuffersSubmitted() const noexcept { return buffersSubmitted.load (std::memory_order_relaxed); }
std::uint64_t AudioObserver::getSamplesSubmitted() const noexcept { return samplesSubmitted.load (std::memory_order_relaxed); }
std::uint64_t AudioObserver::getFifoSamplesDropped() const noexcept { return fifoSamplesDropped.load (std::memory_order_relaxed); }
std::uint64_t AudioObserver::getFifoWindowsDropped() const noexcept { return fifoWindowsDropped.load (std::memory_order_relaxed); }
std::uint64_t AudioObserver::getMidiEventsDropped() const noexcept { return midiEventsDropped.load (std::memory_order_relaxed); }

juce::String AudioObserver::getLastHash() const
{
    juce::SpinLock::ScopedLockType lock (lastHashLock);
    return lastHashHex;
}

} // namespace apw
