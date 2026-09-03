#pragma once

#include <juce_audio_processors/juce_audio_processors.h>

#include "AudioObserver.h"
#include "EventEmitter.h"
#include "SessionActionLog.h"
#include "VerificationClient.h"

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
        bool lastCallbackWasBypassed = false;
    };

    explicit AudioProvenanceCaptureAudioProcessor (int daemonPort = 9876);
    ~AudioProvenanceCaptureAudioProcessor() override;

    void prepareToPlay (double sampleRate, int samplesPerBlock) override;
    void releaseResources() override;

    bool isBusesLayoutSupported (const BusesLayout& layouts) const override;
    void processBlock (juce::AudioBuffer<float>& buffer, juce::MidiBuffer& midiMessages) override;
    void processBlock (juce::AudioBuffer<double>& buffer, juce::MidiBuffer& midiMessages) override;
    void processBlockBypassed (juce::AudioBuffer<float>& buffer,
                               juce::MidiBuffer& midiMessages) override;
    void processBlockBypassed (juce::AudioBuffer<double>& buffer,
                               juce::MidiBuffer& midiMessages) override;
    void reset() override;

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

    void requestVerification (const juce::File& file);
    apw::VerificationClient::Result getVerificationResult() const { return verificationClient.getResult(); }
    const apw::VerificationClient& getVerificationClient() const noexcept { return verificationClient; }

    bool isSigningArmed() const noexcept { return signingArmed.load (std::memory_order_relaxed); }
    void setSigningArmed (bool shouldBeArmed);
    juce::int64 getArmedAtUnixSeconds() const noexcept { return armedAtUnixSeconds.load (std::memory_order_relaxed); }
    juce::String getSigningIdentityFingerprint() const;

    bool hasTelemetryConsent() const noexcept { return sessionActionLog.hasConsent(); }
    void setTelemetryConsent (bool granted);
    const apw::SessionActionLog& getSessionActionLog() const noexcept { return sessionActionLog; }
    std::uint64_t getRejectedStateRestoreCount() const noexcept
    {
        return rejectedStateRestores.load (std::memory_order_relaxed);
    }

private:
    template <typename SampleType>
    void passThrough (juce::AudioBuffer<SampleType>& buffer);

    template <typename SampleType>
    void observeAudioBuffer (const juce::AudioBuffer<SampleType>& buffer) noexcept;

    template <typename SampleType>
    void handleBypassedBuffer (juce::AudioBuffer<SampleType>& buffer) noexcept;

    static bool hasSafeStateEnvelope (const void* data, int sizeInBytes) noexcept;

    std::atomic<int> observedChannelCount { 0 };
    std::atomic<int> observedSampleRateHz { 0 };
    std::atomic<int> observedBufferSizeSamples { 0 };
    std::atomic<std::uint64_t> lastBufferSeenMilliseconds { 0 };
    std::atomic<std::uint64_t> lastNonSilentBufferSeenMilliseconds { 0 };
    std::atomic<bool> lastBufferHadAudio { false };
    std::atomic<bool> lastCallbackWasBypassed { false };

    // Granular observation pipeline (identifiers and emitter must outlive observer).
    const juce::String pluginInstanceId;
    const juce::String pluginCaptureSessionId;
    apw::EventEmitter eventEmitter;
    apw::AudioObserver audioObserver;
    apw::VerificationClient verificationClient;
    apw::SessionActionLog sessionActionLog;
    std::atomic<std::uint64_t> eventSequence { 0 };
    std::atomic<bool> signingArmed { false };
    std::atomic<juce::int64> armedAtUnixSeconds { 0 };
    std::atomic<std::uint64_t> rejectedStateRestores { 0 };

    static constexpr int maxPluginStateBytes = 4096;

    static constexpr std::uint64_t signingIdentityRefreshMilliseconds = 2000;
    mutable juce::CriticalSection signingIdentityLock;
    mutable juce::String signingIdentityFingerprint;
    mutable std::uint64_t signingIdentityCheckedAtMilliseconds = 0;

    // Pre-allocated buffer for double-to-float conversion.
    juce::AudioBuffer<float> doubleConversionBuffer;

    JUCE_DECLARE_NON_COPYABLE_WITH_LEAK_DETECTOR (AudioProvenanceCaptureAudioProcessor)
};
