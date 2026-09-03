#pragma once

#include <juce_audio_processors/juce_audio_processors.h>

#include <cstdint>

class AudioProvenanceCaptureAudioProcessor;

class AudioProvenanceCaptureAudioProcessorEditor final : public juce::AudioProcessorEditor,
                                                        public juce::FileDragAndDropTarget,
                                                        private juce::Timer
{
public:
    explicit AudioProvenanceCaptureAudioProcessorEditor (AudioProvenanceCaptureAudioProcessor&);
    ~AudioProvenanceCaptureAudioProcessorEditor() override = default;

    void paint (juce::Graphics&) override;
    void resized() override;

    bool isInterestedInFileDrag (const juce::StringArray& files) override;
    void fileDragEnter (const juce::StringArray& files, int x, int y) override;
    void fileDragExit (const juce::StringArray& files) override;
    void filesDropped (const juce::StringArray& files, int x, int y) override;

private:
    void timerCallback() override;
    void updateObservationLabels();
    void updateProvenanceLabels();
    void paintStateLegend (juce::Graphics&);

    AudioProvenanceCaptureAudioProcessor& audioProcessor;

    juce::Label titleLabel;
    juce::Label subtitleLabel;
    juce::Label sessionIdLabel;

    juce::Label observationHeaderLabel;
    juce::Label captureStatusLabel;
    juce::Label formatLabel;
    juce::Label lastBufferSeenLabel;
    juce::Label hashChainLabel;
    juce::Label lastHashLabel;
    juce::Label acknowledgementLabel;
    juce::Label responderLabel;
    juce::Label observationDetailHeaderLabel;
    juce::Label throughputLabel;
    juce::Label deliveryLabel;
    juce::Label coverageLabel;
    juce::Label scopeLabel;
    juce::Label proofLevelLabel;

    juce::Label provenanceHeaderLabel;
    juce::Label dropZoneLabel;
    juce::Label verificationStateLabel;
    juce::Label verificationProvesLabel;
    juce::Label verificationNotProvesLabel;
    juce::Label verificationDetailLabel;
    juce::Label verificationSourceLabel;

    juce::Label signingHeaderLabel;
    juce::Label armStateLabel;
    juce::TextButton armButton;
    juce::Label renderDisclosureLabel;
    juce::Label identityLabel;
    juce::ToggleButton telemetryToggle;
    juce::Label telemetryStateLabel;

    juce::Rectangle<int> observationPanelBounds;
    juce::Rectangle<int> provenancePanelBounds;
    juce::Rectangle<int> captureChipBounds;
    juce::Rectangle<int> leftDividerBounds;
    juce::Rectangle<int> dropZoneBounds;
    juce::Rectangle<int> verdictChipBounds;
    juce::Rectangle<int> legendBounds;
    juce::Rectangle<int> rightDividerBounds;
    juce::Rectangle<int> armBadgeBounds;

    bool activityActive = false;
    bool dragHighlight = false;
    bool armedIndicator = false;
    int verdictTokenIndex = -1;
    juce::Colour verdictColour;
    juce::String verdictGlyph;
    std::uint64_t lastRenderedBufferSeenMilliseconds = 0;
    std::uint64_t lastRenderedVerificationMilliseconds = 0;
    juce::String lastRenderedBufferSeenText = "never";

    JUCE_DECLARE_NON_COPYABLE_WITH_LEAK_DETECTOR (AudioProvenanceCaptureAudioProcessorEditor)
};
