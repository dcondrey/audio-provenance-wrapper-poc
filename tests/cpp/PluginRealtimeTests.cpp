#include "PluginProcessor.h"
#include "AcknowledgementLogic.h"

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <limits>
#include <new>
#include <string>
#include <vector>

namespace
{
thread_local bool countAllocationsOnThisThread = false;
thread_local std::uint64_t allocationsOnThisThread = 0;

void noteAllocation() noexcept
{
    if (countAllocationsOnThisThread)
        ++allocationsOnThisThread;
}
}

void* operator new (std::size_t size)
{
    noteAllocation();
    if (auto* memory = std::malloc (size))
        return memory;
    throw std::bad_alloc();
}

void* operator new[] (std::size_t size)
{
    noteAllocation();
    if (auto* memory = std::malloc (size))
        return memory;
    throw std::bad_alloc();
}

void operator delete (void* memory) noexcept { std::free (memory); }
void operator delete[] (void* memory) noexcept { std::free (memory); }
void operator delete (void* memory, std::size_t) noexcept { std::free (memory); }
void operator delete[] (void* memory, std::size_t) noexcept { std::free (memory); }

namespace
{
struct AllocationProbe
{
    AllocationProbe()
    {
        allocationsOnThisThread = 0;
        countAllocationsOnThisThread = true;
    }

    ~AllocationProbe() { countAllocationsOnThisThread = false; }

    std::uint64_t count() const noexcept { return allocationsOnThisThread; }
};

int failures = 0;

void check (bool condition, const std::string& message)
{
    if (condition)
        return;
    ++failures;
    std::cerr << "FAIL: " << message << '\n';
}

template <typename Sample>
void fillSignal (juce::AudioBuffer<Sample>& buffer)
{
    for (int channel = 0; channel < buffer.getNumChannels(); ++channel)
        for (int sample = 0; sample < buffer.getNumSamples(); ++sample)
        {
            const auto phase = static_cast<double> ((sample * 37 + channel * 11) % 257) / 257.0;
            buffer.setSample (channel, sample, static_cast<Sample> (phase * 1.6 - 0.8));
        }

    if (buffer.getNumSamples() >= 4)
    {
        buffer.setSample (0, 0, static_cast<Sample> (-0.0));
        buffer.setSample (0, 1, std::numeric_limits<Sample>::denorm_min());
        buffer.setSample (0, 2, -std::numeric_limits<Sample>::denorm_min());
        buffer.setSample (0, 3, std::numeric_limits<Sample>::quiet_NaN());
    }
}

template <typename Sample, typename Process>
void expectBitIdenticalAndAllocationFree (int channels, int samples,
                                           const std::string& label, Process&& process)
{
    juce::AudioBuffer<Sample> buffer (channels, samples);
    fillSignal (buffer);
    std::vector<Sample> before (static_cast<std::size_t> (channels * samples));
    for (int channel = 0; channel < channels; ++channel)
        std::memcpy (before.data() + static_cast<std::size_t> (channel * samples),
                     buffer.getReadPointer (channel),
                     static_cast<std::size_t> (samples) * sizeof (Sample));

    juce::MidiBuffer midi;
    midi.addEvent (juce::MidiMessage::noteOn (1, 64, static_cast<juce::uint8> (100)), 0);

    std::uint64_t allocationCount = 0;
    {
        AllocationProbe probe;
        process (buffer, midi);
        allocationCount = probe.count();
    }

    check (allocationCount == 0,
           label + " allocated " + std::to_string (allocationCount) + " time(s)");
    for (int channel = 0; channel < channels; ++channel)
    {
        const auto identical = std::memcmp (
            before.data() + static_cast<std::size_t> (channel * samples),
            buffer.getReadPointer (channel),
            static_cast<std::size_t> (samples) * sizeof (Sample)) == 0;
        check (identical, label + " changed channel " + std::to_string (channel));
    }
}

juce::MemoryBlock binaryXml (const juce::String& xml)
{
    juce::MemoryBlock block;
    juce::MemoryOutputStream output (block, false);
    output.writeInt (0x21324356);
    output.writeInt (static_cast<int> (xml.getNumBytesAsUTF8()));
    output.write (xml.toRawUTF8(), static_cast<std::size_t> (xml.getNumBytesAsUTF8()));
    output.writeByte (0);
    return block;
}

void testLayouts()
{
    AudioProvenanceCaptureAudioProcessor processor;
    auto layout = [] (juce::AudioChannelSet input, juce::AudioChannelSet output)
    {
        juce::AudioProcessor::BusesLayout buses;
        buses.inputBuses.add (input);
        buses.outputBuses.add (output);
        return buses;
    };

    check (processor.isBusesLayoutSupported (
               layout (juce::AudioChannelSet::mono(), juce::AudioChannelSet::mono())),
           "mono layout was rejected");
    check (processor.isBusesLayoutSupported (
               layout (juce::AudioChannelSet::stereo(), juce::AudioChannelSet::stereo())),
           "stereo layout was rejected");
    check (! processor.isBusesLayoutSupported (
               layout (juce::AudioChannelSet::mono(), juce::AudioChannelSet::stereo())),
           "mismatched layout was accepted");
    check (! processor.isBusesLayoutSupported (
               layout (juce::AudioChannelSet::create5point1(),
                       juce::AudioChannelSet::create5point1())),
           "unsupported surround layout was accepted");
}

void testPassThrough()
{
    for (const auto channels : { 1, 2 })
    {
        AudioProvenanceCaptureAudioProcessor processor;
        juce::AudioProcessor::BusesLayout buses;
        const auto set = channels == 1 ? juce::AudioChannelSet::mono()
                                       : juce::AudioChannelSet::stereo();
        buses.inputBuses.add (set);
        buses.outputBuses.add (set);
        check (processor.setBusesLayout (buses), "could not select test layout");
        processor.prepareToPlay (48'000.0, 4096);

        for (const auto samples : { 1, 7, 64, 511, 512, 4096, 8192 })
        {
            const auto stem = std::to_string (channels) + "ch/" + std::to_string (samples);
            expectBitIdenticalAndAllocationFree<float> (
                channels, samples, "float process " + stem,
                [&processor] (auto& buffer, auto& midi) { processor.processBlock (buffer, midi); });
            expectBitIdenticalAndAllocationFree<double> (
                channels, samples, "double process " + stem,
                [&processor] (auto& buffer, auto& midi) { processor.processBlock (buffer, midi); });
            expectBitIdenticalAndAllocationFree<float> (
                channels, samples, "float bypass " + stem,
                [&processor] (auto& buffer, auto& midi) {
                    processor.processBlockBypassed (buffer, midi);
                });
            expectBitIdenticalAndAllocationFree<double> (
                channels, samples, "double bypass " + stem,
                [&processor] (auto& buffer, auto& midi) {
                    processor.processBlockBypassed (buffer, midi);
                });
        }

        processor.releaseResources();
        processor.prepareToPlay (44'100.0, 127);
        expectBitIdenticalAndAllocationFree<float> (
            channels, 127, "sample-rate lifecycle change",
            [&processor] (auto& buffer, auto& midi) { processor.processBlock (buffer, midi); });
    }
}

void testBoundedLossAndInstanceIsolation()
{
    AudioProvenanceCaptureAudioProcessor first;
    AudioProvenanceCaptureAudioProcessor second;
    check (first.getPluginInstanceId() != second.getPluginInstanceId(),
           "two instances shared a plugin id");
    check (first.getPluginCaptureSessionId() != second.getPluginCaptureSessionId(),
           "two instances shared a capture-session id");

    first.prepareToPlay (48'000.0, 512);
    expectBitIdenticalAndAllocationFree<float> (
        2, 100'000, "FIFO saturation",
        [&first] (auto& buffer, auto& midi) { first.processBlock (buffer, midi); });
    check (first.getAudioObserver().getFifoSamplesDropped() > 0,
           "FIFO saturation was not counted");

    juce::AudioBuffer<float> bypassed (2, 256);
    fillSignal (bypassed);
    juce::MidiBuffer midi;
    first.processBlockBypassed (bypassed, midi);
    check (first.getAudioObserver().getBypassedBuffers() == 1,
           "host bypass was not counted");
    check (first.getAudioBufferObservationSnapshot().lastCallbackWasBypassed,
           "host bypass was not surfaced in the processor snapshot");
}

void testStateBoundary()
{
    AudioProvenanceCaptureAudioProcessor source;
    source.setSigningArmed (true);
    juce::MemoryBlock valid;
    source.getStateInformation (valid);

    AudioProvenanceCaptureAudioProcessor restored;
    restored.setStateInformation (valid.getData(), static_cast<int> (valid.getSize()));
    if (! restored.isSigningArmed())
    {
        const auto length = static_cast<int> (juce::ByteOrder::littleEndianInt (
            static_cast<const char*> (valid.getData()) + 4));
        std::cerr << "state payload: "
                  << juce::String::fromUTF8 (static_cast<const char*> (valid.getData()) + 8, length)
                  << " (rejections " << restored.getRejectedStateRestoreCount() << ")\n";
    }
    check (restored.isSigningArmed(), "valid state did not restore");

    std::vector<std::byte> oversized (4097);
    restored.setStateInformation (oversized.data(), static_cast<int> (oversized.size()));
    check (! restored.isSigningArmed(), "oversized state changed safe defaults");

    const auto nested = binaryXml (
        "<APW_PLUGIN_STATE state_version=\"1\" signing_armed=\"0\" "
        "armed_at_unix_seconds=\"0\" session_action_telemetry_consent=\"0\"><x/></APW_PLUGIN_STATE>");
    restored.setStateInformation (nested.getData(), static_cast<int> (nested.getSize()));

    const auto malformed = binaryXml (
        "<APW_PLUGIN_STATE state_version=\"1\" signing_armed=\"yes\" "
        "armed_at_unix_seconds=\"0\" session_action_telemetry_consent=\"0\"/>");
    restored.setStateInformation (malformed.getData(), static_cast<int> (malformed.getSize()));
    check (restored.getRejectedStateRestoreCount() == 3,
           "malformed/oversized state rejections were not counted exactly");
}

void testBoundedShutdown()
{
    const auto started = std::chrono::steady_clock::now();
    {
        AudioProvenanceCaptureAudioProcessor processor;
        processor.prepareToPlay (48'000.0, 512);
        juce::AudioBuffer<float> buffer (2, 100'000);
        fillSignal (buffer);
        juce::MidiBuffer midi;
        processor.processBlock (buffer, midi);
    }
    const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds> (
        std::chrono::steady_clock::now() - started);
    check (elapsed.count() < 1500,
           "processor shutdown exceeded its aggregate bound: "
               + std::to_string (elapsed.count()) + " ms");
}

void testPureAcknowledgementLogic()
{
    apw::ack::Fields fields;
    fields.envelopeValid = true;
    fields.pluginInstanceId = "plugin-a";
    fields.pluginCaptureSessionId = "session-a";
    fields.daemonInstanceId = "daemon-1";
    fields.acceptedFieldValid = true;
    fields.accepted = true;
    fields.countersValid = true;
    fields.highestAcceptedSequence = 12;
    fields.highestContiguousSequence = 11;
    fields.streamGaps = 1;

    apw::ack::State state;
    auto update = apw::ack::apply (fields, "plugin-a", "session-a", state);
    check (update.decision == apw::ack::Decision::accepted,
           "a well-formed scoped acknowledgement was rejected");
    check (update.state.highestAcceptedSequence == 12
               && update.state.highestContiguousSequence == 11,
           "valid acknowledgement counters were not applied");
    state = update.state;

    fields.pluginCaptureSessionId = "session-b";
    update = apw::ack::apply (fields, "plugin-a", "session-a", state);
    check (update.decision == apw::ack::Decision::scopeMismatch,
           "another instance/session acknowledgement was not isolated");
    check (update.state.highestAcceptedSequence == state.highestAcceptedSequence,
           "a scope mismatch mutated acknowledgement state");

    fields.pluginCaptureSessionId = "session-a";
    fields.highestAcceptedSequence = apw::ack::maxCounter + 1;
    update = apw::ack::apply (fields, "plugin-a", "session-a", state);
    check (update.decision == apw::ack::Decision::malformed,
           "an oversized acknowledgement counter was accepted");
    check (update.state.highestAcceptedSequence == state.highestAcceptedSequence,
           "a malformed acknowledgement mutated state");

    fields.highestAcceptedSequence = 5;
    fields.highestContiguousSequence = 6;
    update = apw::ack::apply (fields, "plugin-a", "session-a", state);
    check (update.decision == apw::ack::Decision::malformed,
           "contiguous sequence beyond accepted sequence was accepted");

    fields.highestContiguousSequence = 5;
    fields.daemonInstanceId = "daemon-2";
    update = apw::ack::apply (fields, "plugin-a", "session-a", state);
    check (update.decision == apw::ack::Decision::accepted && update.daemonRestarted,
           "daemon restart was not surfaced");
    check (update.state.highestAcceptedSequence == 5,
           "a daemon restart mixed the previous daemon's sequence state");
}
}

int main()
{
    juce::ScopedJuceInitialiser_GUI juceInitialiser;
    testLayouts();
    testPassThrough();
    testBoundedLossAndInstanceIsolation();
    testStateBoundary();
    testPureAcknowledgementLogic();
    testBoundedShutdown();

    if (failures == 0)
    {
        std::cout << "plugin realtime/lifecycle tests passed\n";
        return 0;
    }
    std::cerr << failures << " plugin realtime/lifecycle test(s) failed\n";
    return 1;
}
