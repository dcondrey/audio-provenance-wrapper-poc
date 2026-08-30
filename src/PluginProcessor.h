#pragma once

#include <juce_audio_processors/juce_audio_processors.h>

#include "AudioObserver.h"
#include "EventEmitter.h"

#include <atomic>
#include <cstdint>

class AudioProvenanceCaptureAudioProcessor final : public juce::AudioProcessor
{
public:
    struct AudioBufferObservationSnapshot
    {
        int channelCount = 0;
        int sampleRateHz = 0;
        int bufferSizeSamples = 0;
        std::uint64_t lastBufferSeenMilliseconds = 0;
        std::uint64_t lastNonSilentBufferSeenMilliseconds = 0;
        bool lastBufferHadAudio = false;
    };

    AudioProvenanceCaptureAudioProcessor();
    ~AudioProvenanceCaptureAudioProcessor() override;

    void prepareToPlay (double sampleRate, int samplesPerBlock) override;
    void releaseResources() override;

    bool isBusesLayoutSupported (const BusesLayout& layouts) const override;
    void processBlock (juce::AudioBuffer<float>& buffer, juce::MidiBuffer& midiMessages) override;
    void processBlock (juce::AudioBuffer<double>& buffer, juce::MidiBuffer& midiMessages) override;

    juce::AudioProcessorEditor* createEditor() override;
    bool hasEditor() const override;

    const juce::String getName() const override;
    bool acceptsMidi() const override;
    bool producesMidi() const override;
    bool isMidiEffect() const override;
    double getTailLengthSeconds() const override;

    int getNumPrograms() override;
    int getCurrentProgram() override;
    void setCurrentProgram (int index) override;
    const juce::String getProgramName (int index) override;
    void changeProgramName (int index, const juce::String& newName) override;

    void getStateInformation (juce::MemoryBlock& destData) override;
    void setStateInformation (const void* data, int sizeInBytes) override;

    AudioBufferObservationSnapshot getAudioBufferObservationSnapshot() const noexcept;

    // Granular observation stats for the UI.
    apw::AudioObserver& getAudioObserver() noexcept { return audioObserver; }
    const apw::EventEmitter& getEventEmitter() const noexcept { return eventEmitter; }
    std::uint64_t getHighestPreparedSequence() const noexcept { return eventSequence.load (std::memory_order_relaxed); }
    const juce::String& getPluginInstanceId() const noexcept { return pluginInstanceId; }
    const juce::String& getPluginCaptureSessionId() const noexcept { return pluginCaptureSessionId; }

private:
    template <typename SampleType>
    void passThrough (juce::AudioBuffer<SampleType>& buffer);

    template <typename SampleType>
    void observeAudioBuffer (const juce::AudioBuffer<SampleType>& buffer) noexcept;

    std::atomic<int> observedChannelCount { 0 };
    std::atomic<int> observedSampleRateHz { 0 };
    std::atomic<int> observedBufferSizeSamples { 0 };
    std::atomic<std::uint64_t> lastBufferSeenMilliseconds { 0 };
    std::atomic<std::uint64_t> lastNonSilentBufferSeenMilliseconds { 0 };
    std::atomic<bool> lastBufferHadAudio { false };

    // Granular observation pipeline (identifiers and emitter must outlive observer).
    const juce::String pluginInstanceId;
    const juce::String pluginCaptureSessionId;
    apw::EventEmitter eventEmitter;
    apw::AudioObserver audioObserver;
    std::atomic<std::uint64_t> eventSequence { 0 };

    // Pre-allocated buffer for double-to-float conversion.
    juce::AudioBuffer<float> doubleConversionBuffer;

    JUCE_DECLARE_NON_COPYABLE_WITH_LEAK_DETECTOR (AudioProvenanceCaptureAudioProcessor)
};
