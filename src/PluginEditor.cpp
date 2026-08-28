#include "PluginEditor.h"
#include "PluginProcessor.h"

namespace
{
constexpr std::uint64_t activeTimeoutMilliseconds = 1000;

void configureObservationLabel (juce::Label& label, float fontSize)
{
    label.setJustificationType (juce::Justification::centredLeft);
    label.setFont (juce::FontOptions (fontSize));
    label.setColour (juce::Label::textColourId, juce::Colour::fromRGB (225, 236, 247));
}
}

AudioProvenanceCaptureAudioProcessorEditor::AudioProvenanceCaptureAudioProcessorEditor (
    AudioProvenanceCaptureAudioProcessor& processorRef)
    : AudioProcessorEditor (&processorRef),
      audioProcessor (processorRef)
{
    titleLabel.setText ("Audio Provenance Capture", juce::dontSendNotification);
    titleLabel.setJustificationType (juce::Justification::centredLeft);
    titleLabel.setFont (juce::FontOptions (20.0f, juce::Font::bold));
    addAndMakeVisible (titleLabel);

    subtitleLabel.setText ("Routed audio evidence adapter · one observed path", juce::dontSendNotification);
    configureObservationLabel (subtitleLabel, 12.0f);
    subtitleLabel.setColour (juce::Label::textColourId, juce::Colour::fromRGB (137, 157, 179));
    addAndMakeVisible (subtitleLabel);

    configureObservationLabel (captureStatusLabel, 15.0f);
    addAndMakeVisible (captureStatusLabel);

    configureObservationLabel (audioDetectedLabel, 15.0f);
    addAndMakeVisible (audioDetectedLabel);

    configureObservationLabel (channelCountLabel, 15.0f);
    addAndMakeVisible (channelCountLabel);

    configureObservationLabel (sampleRateLabel, 15.0f);
    addAndMakeVisible (sampleRateLabel);

    configureObservationLabel (bufferSizeLabel, 15.0f);
    addAndMakeVisible (bufferSizeLabel);

    configureObservationLabel (lastBufferSeenLabel, 15.0f);
    addAndMakeVisible (lastBufferSeenLabel);

    configureObservationLabel (hashChainLabel, 15.0f);
    addAndMakeVisible (hashChainLabel);

    configureObservationLabel (lastHashLabel, 13.0f);
    addAndMakeVisible (lastHashLabel);

    scopeLabel.setText ("Scope: routed audio only - bypassed paths remain unobserved.",
                        juce::dontSendNotification);
    scopeLabel.setJustificationType (juce::Justification::centredLeft);
    scopeLabel.setFont (juce::FontOptions (13.0f));
    addAndMakeVisible (scopeLabel);

    configureObservationLabel (sessionIdLabel, 12.0f);
    sessionIdLabel.setColour (juce::Label::textColourId, juce::Colour::fromRGB (77, 227, 255));
    addAndMakeVisible (sessionIdLabel);

    configureObservationLabel (coverageLabel, 13.0f);
    addAndMakeVisible (coverageLabel);

    configureObservationLabel (deliveryLabel, 13.0f);
    addAndMakeVisible (deliveryLabel);

    updateObservationLabels();
    startTimerHz (4);

    setSize (620, 460);
}

void AudioProvenanceCaptureAudioProcessorEditor::paint (juce::Graphics& g)
{
    g.fillAll (juce::Colour::fromRGB (11, 15, 20));
    auto bounds = getLocalBounds().toFloat().reduced (18.0f);
    g.setColour (juce::Colour::fromRGB (19, 27, 36));
    g.fillRoundedRectangle (bounds.withTrimmedTop (82.0f), 14.0f);
    g.setColour (juce::Colour::fromRGB (38, 51, 68));
    g.drawRoundedRectangle (bounds.withTrimmedTop (82.0f), 14.0f, 1.0f);

    auto meter = juce::Rectangle<float> (bounds.getRight() - 150.0f, bounds.getY() + 18.0f, 150.0f, 34.0f);
    for (int i = 0; i < 12; ++i)
    {
        const auto intensity = activityActive ? (0.35f + 0.65f * static_cast<float> ((i * 7) % 11) / 10.0f) : 0.12f;
        g.setColour (juce::Colour::fromRGB (77, 227, 255).withAlpha (intensity));
        const auto height = activityActive ? 8.0f + static_cast<float> ((i * 13) % 25) : 5.0f;
        g.fillRoundedRectangle (meter.getX() + static_cast<float> (i) * 12.0f,
                                meter.getBottom() - height, 7.0f, height, 2.0f);
    }
}

void AudioProvenanceCaptureAudioProcessorEditor::resized()
{
    auto bounds = getLocalBounds().reduced (24);
    titleLabel.setBounds (bounds.removeFromTop (30));
    subtitleLabel.setBounds (bounds.removeFromTop (22));
    sessionIdLabel.setBounds (bounds.removeFromTop (24));
    bounds.removeFromTop (22);
    captureStatusLabel.setBounds (bounds.removeFromTop (28));
    audioDetectedLabel.setBounds (bounds.removeFromTop (24));
    sampleRateLabel.setBounds (bounds.removeFromTop (24));
    bufferSizeLabel.setBounds (bounds.removeFromTop (24));
    hashChainLabel.setBounds (bounds.removeFromTop (24));
    lastHashLabel.setBounds (bounds.removeFromTop (24));
    deliveryLabel.setBounds (bounds.removeFromTop (24));
    coverageLabel.setBounds (bounds.removeFromTop (24));
    lastBufferSeenLabel.setBounds (bounds.removeFromTop (24));
    scopeLabel.setBounds (bounds.removeFromTop (42));
    channelCountLabel.setBounds (0, 0, 0, 0);
}

void AudioProvenanceCaptureAudioProcessorEditor::timerCallback()
{
    updateObservationLabels();
}

void AudioProvenanceCaptureAudioProcessorEditor::updateObservationLabels()
{
    const auto snapshot = audioProcessor.getAudioBufferObservationSnapshot();
    const auto nowMilliseconds = static_cast<std::uint64_t> (juce::Time::getMillisecondCounterHiRes());
    const auto hasRecentAudio = snapshot.lastNonSilentBufferSeenMilliseconds > 0
        && nowMilliseconds >= snapshot.lastNonSilentBufferSeenMilliseconds
        && nowMilliseconds - snapshot.lastNonSilentBufferSeenMilliseconds <= activeTimeoutMilliseconds;
    activityActive = hasRecentAudio;

    if (snapshot.lastBufferSeenMilliseconds > 0
        && snapshot.lastBufferSeenMilliseconds != lastRenderedBufferSeenMilliseconds)
    {
        lastRenderedBufferSeenMilliseconds = snapshot.lastBufferSeenMilliseconds;
        lastRenderedBufferSeenText = juce::Time::getCurrentTime().formatted ("%H:%M:%S");
    }

    captureStatusLabel.setText (juce::String ("Capture status: ") + (hasRecentAudio ? "ACTIVE" : "IDLE"),
                                juce::dontSendNotification);
    audioDetectedLabel.setText (juce::String ("Audio detected: ") + (hasRecentAudio ? "yes" : "no"),
                                juce::dontSendNotification);
    sessionIdLabel.setText (audioProcessor.getPluginInstanceId() + "  ·  "
                            + audioProcessor.getPluginCaptureSessionId(), juce::dontSendNotification);
    sampleRateLabel.setText (juce::String ("Format: ") + juce::String (snapshot.sampleRateHz) + " Hz · "
                             + juce::String (snapshot.channelCount) + " ch",
                             juce::dontSendNotification);
    bufferSizeLabel.setText (juce::String ("Submitted: ")
                             + juce::String (static_cast<juce::int64> (audioProcessor.getAudioObserver().getBuffersSubmitted()))
                             + " buffers · "
                             + juce::String (static_cast<juce::int64> (audioProcessor.getAudioObserver().getSamplesSubmitted()))
                             + " samples",
                             juce::dontSendNotification);
    lastBufferSeenLabel.setText (juce::String ("Last buffer seen: ") + lastRenderedBufferSeenText,
                                 juce::dontSendNotification);

    // Granular observation stats.
    auto& observer = audioProcessor.getAudioObserver();
    const auto windowsHashed = observer.getTotalWindowsHashed();
    const auto eventsEmitted = observer.getTotalEventsEmitted();

    hashChainLabel.setText (juce::String ("Hash chain: ")
                            + juce::String (static_cast<juce::int64> (windowsHashed)) + " windows, "
                            + juce::String (static_cast<juce::int64> (eventsEmitted)) + " events prepared",
                            juce::dontSendNotification);

    auto lastHash = observer.getLastHash();
    if (lastHash.isNotEmpty())
        lastHashLabel.setText (juce::String ("Last hash: ") + lastHash.substring (0, 16) + "...",
                               juce::dontSendNotification);
    else
        lastHashLabel.setText ("Last hash: (none)", juce::dontSendNotification);

    const auto fifoSamplesDropped = observer.getFifoSamplesDropped();
    const auto fifoWindowsDropped = observer.getFifoWindowsDropped();
    auto& emitter = audioProcessor.getEventEmitter();
    deliveryLabel.setText (juce::String ("Emitted (local UDP): ")
                           + juce::String (static_cast<juce::int64> (emitter.getSendAccepted())) + " · "
                           + juce::String (static_cast<juce::int64> (emitter.getSendFailures()))
                           + " failed · received by daemon: UNKNOWN",
                           juce::dontSendNotification);
    coverageLabel.setText (juce::String ("Coverage: UNKNOWN_COVERAGE · FIFO loss ")
                           + juce::String (static_cast<juce::int64> (fifoSamplesDropped)) + " samples / "
                           + juce::String (static_cast<juce::int64> (fifoWindowsDropped)) + " windows",
                           juce::dontSendNotification);
    repaint();
}
