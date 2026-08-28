#pragma once

#include <juce_audio_processors/juce_audio_processors.h>

#include <cstdint>

class AudioProvenanceCaptureAudioProcessor;

class AudioProvenanceCaptureAudioProcessorEditor final : public juce::AudioProcessorEditor,
                                                        private juce::Timer
{
public:
    explicit AudioProvenanceCaptureAudioProcessorEditor (AudioProvenanceCaptureAudioProcessor&);
    ~AudioProvenanceCaptureAudioProcessorEditor() override = default;

    void paint (juce::Graphics&) override;
    void resized() override;

private:
    void timerCallback() override;
    void updateObservationLabels();

    AudioProvenanceCaptureAudioProcessor& audioProcessor;
    juce::Label titleLabel;
    juce::Label subtitleLabel;
    juce::Label captureStatusLabel;
    juce::Label audioDetectedLabel;
    juce::Label channelCountLabel;
    juce::Label sampleRateLabel;
    juce::Label bufferSizeLabel;
    juce::Label lastBufferSeenLabel;
    juce::Label hashChainLabel;
    juce::Label lastHashLabel;
    juce::Label scopeLabel;
    juce::Label sessionIdLabel;
    juce::Label coverageLabel;
    juce::Label deliveryLabel;
    bool activityActive = false;
    std::uint64_t lastRenderedBufferSeenMilliseconds = 0;
    juce::String lastRenderedBufferSeenText = "never";

    JUCE_DECLARE_NON_COPYABLE_WITH_LEAK_DETECTOR (AudioProvenanceCaptureAudioProcessorEditor)
};
