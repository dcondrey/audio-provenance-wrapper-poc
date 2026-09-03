#include "PluginEditor.h"
#include "PluginProcessor.h"

#include <iterator>

namespace
{
constexpr std::uint64_t activeTimeoutMilliseconds = 1000;

const auto backdropColour = juce::Colour::fromRGB (11, 15, 20);
const auto panelColour    = juce::Colour::fromRGB (19, 27, 36);
const auto insetColour    = juce::Colour::fromRGB (15, 21, 29);
const auto borderColour   = juce::Colour::fromRGB (38, 51, 68);
const auto accentColour   = juce::Colour::fromRGB (77, 227, 255);
const auto textColour     = juce::Colour::fromRGB (225, 236, 247);
const auto mutedColour    = juce::Colour::fromRGB (137, 157, 179);
const auto dimColour      = juce::Colour::fromRGB (99, 117, 138);
const auto warningColour  = juce::Colour::fromRGB (240, 184, 96);
const auto successColour  = juce::Colour::fromRGB (118, 220, 158);
const auto alertColour    = juce::Colour::fromRGB (240, 118, 118);
const auto neutralColour  = juce::Colour::fromRGB (150, 172, 197);

// REQUIRED: the four public result states are a closed vocabulary. Rendered
// verbatim, lowercase, hyphens intact; emphasis comes from weight and colour.
const char* const stateTokens[] = { "verified",
                                    "registered-but-changed",
                                    "mark-found-claim-not-trusted",
                                    "nothing-found" };

juce::Colour stateTokenColour (int index)
{
    switch (index)
    {
        case 0:  return successColour;
        case 1:  return alertColour;
        case 2:  return warningColour;
        case 3:  return neutralColour;
        default: return mutedColour;
    }
}

struct VerdictPresentation
{
    int tokenIndex = -1;
    juce::String glyph;
    juce::String headline;
    juce::String proves;
    juce::String notProves;
};

// IMPORTANT: docs/PROVIDER_CONTRACT.md ordering rule: an untrusted signer
// outranks a broken hard binding, because registered-but-changed asserts the
// claim WAS trusted and only the bytes moved. Nothing reached through the
// plug-in's own evidence read validates a credential, so a hash mismatch found
// there grades as mark-found-claim-not-trusted, never as a registration.
VerdictPresentation presentOutcome (apw::VerificationClient::Outcome outcome, bool signatureChecked)
{
    using Outcome = apw::VerificationClient::Outcome;

    switch (outcome)
    {
        case Outcome::verified:
            if (! signatureChecked)
                return { 2, "?", stateTokens[2],
                         "Proves: a claim was found for this file and its bytes match it.",
                         "Does not prove: that the claim is trusted. No signing credential was validated at this step." };
            return { 0, juce::String::fromUTF8 ("\xe2\x9c\x93"), stateTokens[0],
                     "Proves: the bytes hashed here match the signed claim, and the signing chain validated against a trust anchor on this machine.",
                     "Does not prove: who signed it. The chain is self-issued, so this is possession of a key, not an established identity." };

        case Outcome::changed:
            if (! signatureChecked)
                return { 2, "?", stateTokens[2],
                         "Proves: a local evidence record names this file, and the bytes hashed now are not the bytes it recorded.",
                         "Does not prove: that a trusted registration was broken. No signature was checked at this step, so no claim was ever established here." };
            return { 1, juce::String::fromUTF8 ("\xe2\x89\xa0"), stateTokens[1],
                     "Proves: a trusted claim describes this file, and the bytes hashed now are not the bytes it covered.",
                     "Does not prove: tampering. A second render, a format conversion or a trim of the same material reads exactly the same way." };

        case Outcome::claimUntrusted:
        case Outcome::claimNotChecked:
        case Outcome::boundToLocalManifest:
            return { 2, "?", stateTokens[2],
                     "Proves: something was found for this file. The line below says whether it was a C2PA claim or a local evidence record.",
                     "Does not prove: that it can be trusted. Either no signature was checked at this step, or the verifier checked one and did not trust it." };

        case Outcome::notFound:
            return { 3, juce::String::fromUTF8 ("\xe2\x80\x94"), stateTokens[3],
                     "Proves: nothing about the audio. No embedded claim, no sidecar and no local evidence record binds these bytes.",
                     "Does not prove: that the audio is synthetic, edited or untrustworthy. A missing mark is only a missing mark." };

        case Outcome::inProgress:
            return { -1, juce::String::fromUTF8 ("\xc2\xb7"), "inspecting",
                     "No result yet. Hashing the file and searching for a claim beside it.", {} };

        case Outcome::failed:
            return { -1, "!", "could not inspect",
                     "Not a result. The file could not be read, so nothing was decided about it either way.", {} };

        case Outcome::idle:
            break;
    }

    return { -1, juce::String::fromUTF8 ("\xc2\xb7"), "awaiting a file",
             "Drop a rendered audio file to get exactly one of the four results above.", {} };
}

bool isInspectableAudioPath (const juce::String& path)
{
    const auto extension = juce::File (path).getFileExtension().toLowerCase();
    return extension == ".wav" || extension == ".aif" || extension == ".aiff"
        || extension == ".flac" || extension == ".mp3" || extension == ".m4a"
        || extension == ".c2pa";
}

// A detached ".c2pa" claim describes the audio beside it. Inspecting the
// sidecar's own bytes matches no manifest and lets the sidecar stand as its own
// evidence, so the drop resolves to the file the claim is about.
juce::File resolveInspectionTarget (const juce::File& dropped)
{
    if (dropped.getFileExtension().toLowerCase() != ".c2pa")
        return dropped;

    const auto path = dropped.getFullPathName();
    const auto described = juce::File (path.dropLastCharacters (5));
    return described.existsAsFile() ? described : dropped;
}

void configureLabel (juce::Label& label, float fontSize, juce::Colour colour,
                     juce::Justification justification = juce::Justification::centredLeft)
{
    label.setJustificationType (justification);
    label.setFont (juce::FontOptions (fontSize));
    label.setColour (juce::Label::textColourId, colour);
}

void configureSectionHeader (juce::Label& label, const juce::String& text)
{
    label.setText (text, juce::dontSendNotification);
    label.setJustificationType (juce::Justification::centredLeft);
    label.setFont (juce::FontOptions (11.0f, juce::Font::bold));
    label.setColour (juce::Label::textColourId, dimColour);
}

void paintChip (juce::Graphics& g, juce::Rectangle<int> bounds, juce::Colour colour, bool emphatic)
{
    if (bounds.isEmpty())
        return;

    g.setColour (colour.withAlpha (emphatic ? 0.16f : 0.06f));
    g.fillRoundedRectangle (bounds.toFloat(), 9.0f);
    g.setColour (colour.withAlpha (emphatic ? 0.95f : 0.40f));
    g.drawRoundedRectangle (bounds.toFloat().reduced (0.5f), 9.0f, emphatic ? 1.4f : 1.0f);
}
}

AudioProvenanceCaptureAudioProcessorEditor::AudioProvenanceCaptureAudioProcessorEditor (
    AudioProvenanceCaptureAudioProcessor& processorRef)
    : AudioProcessorEditor (&processorRef),
      audioProcessor (processorRef),
      verdictColour (mutedColour)
{
    titleLabel.setText ("Audio Provenance Capture", juce::dontSendNotification);
    titleLabel.setJustificationType (juce::Justification::centredLeft);
    titleLabel.setFont (juce::FontOptions (21.0f, juce::Font::bold));
    titleLabel.setColour (juce::Label::textColourId, textColour);
    addAndMakeVisible (titleLabel);

    subtitleLabel.setText ("Observes the one audio path routed through it. It does not capture the DAW.",
                           juce::dontSendNotification);
    configureLabel (subtitleLabel, 12.5f, mutedColour);
    addAndMakeVisible (subtitleLabel);

    configureLabel (sessionIdLabel, 11.0f, accentColour);
    addAndMakeVisible (sessionIdLabel);

    configureSectionHeader (observationHeaderLabel, "OBSERVED CAPTURE");
    addAndMakeVisible (observationHeaderLabel);

    captureStatusLabel.setJustificationType (juce::Justification::centredLeft);
    captureStatusLabel.setFont (juce::FontOptions (15.0f, juce::Font::bold));
    captureStatusLabel.setColour (juce::Label::textColourId, textColour);
    addAndMakeVisible (captureStatusLabel);

    for (auto* label : { &formatLabel, &lastBufferSeenLabel, &hashChainLabel })
    {
        configureLabel (*label, 13.0f, textColour);
        addAndMakeVisible (*label);
    }

    configureLabel (lastHashLabel, 12.5f, mutedColour);
    addAndMakeVisible (lastHashLabel);

    configureLabel (acknowledgementLabel, 12.5f, textColour, juce::Justification::topLeft);
    addAndMakeVisible (acknowledgementLabel);

    configureLabel (responderLabel, 12.0f, mutedColour, juce::Justification::topLeft);
    addAndMakeVisible (responderLabel);

    configureSectionHeader (observationDetailHeaderLabel, "DELIVERY AND LOCAL HEALTH");
    addAndMakeVisible (observationDetailHeaderLabel);

    for (auto* label : { &throughputLabel, &deliveryLabel, &coverageLabel })
    {
        configureLabel (*label, 11.5f, dimColour, juce::Justification::topLeft);
        addAndMakeVisible (*label);
    }

    scopeLabel.setText ("Scope: only audio routed through this plug-in is observed. Bypassed paths, other tracks "
                        "and everything else in the session stay unobserved.",
                        juce::dontSendNotification);
    configureLabel (scopeLabel, 12.0f, mutedColour, juce::Justification::topLeft);
    addAndMakeVisible (scopeLabel);

    proofLevelLabel.setText ("Every emitted field carries a proof level: directly_observed for what this plug-in saw "
                             "on this path, and inferred, user_declared, externally_verified or unknown_unobserved "
                             "for everything it did not.",
                             juce::dontSendNotification);
    configureLabel (proofLevelLabel, 11.5f, dimColour, juce::Justification::topLeft);
    addAndMakeVisible (proofLevelLabel);

    configureSectionHeader (provenanceHeaderLabel, "CLAIM INSPECTION");
    addAndMakeVisible (provenanceHeaderLabel);

    dropZoneLabel.setText ("Drop a rendered audio file here to inspect it.\n"
                           "Files dropped on the DAW timeline are not visible to this plug-in.",
                           juce::dontSendNotification);
    configureLabel (dropZoneLabel, 12.5f, mutedColour, juce::Justification::centred);
    dropZoneLabel.setInterceptsMouseClicks (false, false);
    addAndMakeVisible (dropZoneLabel);

    verificationStateLabel.setJustificationType (juce::Justification::centredLeft);
    verificationStateLabel.setFont (juce::FontOptions (17.0f, juce::Font::bold));
    verificationStateLabel.setColour (juce::Label::textColourId, mutedColour);
    addAndMakeVisible (verificationStateLabel);

    configureLabel (verificationProvesLabel, 12.5f, textColour, juce::Justification::topLeft);
    addAndMakeVisible (verificationProvesLabel);

    configureLabel (verificationNotProvesLabel, 12.5f, warningColour, juce::Justification::topLeft);
    addAndMakeVisible (verificationNotProvesLabel);

    configureLabel (verificationDetailLabel, 12.0f, mutedColour, juce::Justification::topLeft);
    addAndMakeVisible (verificationDetailLabel);

    configureLabel (verificationSourceLabel, 11.0f, dimColour, juce::Justification::topLeft);
    addAndMakeVisible (verificationSourceLabel);

    configureSectionHeader (signingHeaderLabel, "SIGNING");
    addAndMakeVisible (signingHeaderLabel);

    armStateLabel.setJustificationType (juce::Justification::centred);
    armStateLabel.setFont (juce::FontOptions (13.0f, juce::Font::bold));
    addAndMakeVisible (armStateLabel);

    armButton.setColour (juce::TextButton::buttonColourId, juce::Colour::fromRGB (28, 39, 52));
    armButton.setColour (juce::TextButton::textColourOffId, textColour);
    armButton.onClick = [this]
    {
        audioProcessor.setSigningArmed (! audioProcessor.isSigningArmed());
        updateProvenanceLabels();
    };
    addAndMakeVisible (armButton);

    configureLabel (renderDisclosureLabel, 12.0f, textColour, juce::Justification::topLeft);
    addAndMakeVisible (renderDisclosureLabel);

    configureLabel (identityLabel, 11.5f, mutedColour, juce::Justification::topLeft);
    addAndMakeVisible (identityLabel);

    telemetryToggle.setButtonText ("Capture session actions to a local file");
    telemetryToggle.setColour (juce::ToggleButton::textColourId, textColour);
    telemetryToggle.setColour (juce::ToggleButton::tickColourId, accentColour);
    telemetryToggle.setToggleState (audioProcessor.hasTelemetryConsent(), juce::dontSendNotification);
    telemetryToggle.onClick = [this]
    {
        audioProcessor.setTelemetryConsent (telemetryToggle.getToggleState());
        updateProvenanceLabels();
    };
    addAndMakeVisible (telemetryToggle);

    configureLabel (telemetryStateLabel, 12.0f, mutedColour, juce::Justification::topLeft);
    addAndMakeVisible (telemetryStateLabel);

    updateObservationLabels();
    updateProvenanceLabels();
    startTimerHz (4);

    setSize (1020, 730);
}

void AudioProvenanceCaptureAudioProcessorEditor::paint (juce::Graphics& g)
{
    g.fillAll (backdropColour);

    for (const auto& panel : { observationPanelBounds, provenancePanelBounds })
    {
        if (panel.isEmpty())
            continue;
        g.setColour (panelColour);
        g.fillRoundedRectangle (panel.toFloat(), 14.0f);
        g.setColour (borderColour);
        g.drawRoundedRectangle (panel.toFloat().reduced (0.5f), 14.0f, 1.0f);
    }

    for (const auto& divider : { leftDividerBounds, rightDividerBounds })
    {
        if (divider.isEmpty())
            continue;
        g.setColour (borderColour);
        g.fillRect (divider);
    }

    paintChip (g, captureChipBounds, activityActive ? successColour : mutedColour, activityActive);
    if (! captureChipBounds.isEmpty())
    {
        g.setColour (activityActive ? successColour : mutedColour.withAlpha (0.55f));
        g.fillEllipse (static_cast<float> (captureChipBounds.getX()) + 14.0f,
                       static_cast<float> (captureChipBounds.getCentreY()) - 4.0f, 8.0f, 8.0f);
    }

    if (! dropZoneBounds.isEmpty())
    {
        g.setColour (dragHighlight ? accentColour.withAlpha (0.18f) : insetColour);
        g.fillRoundedRectangle (dropZoneBounds.toFloat(), 10.0f);
        g.setColour (dragHighlight ? accentColour : borderColour);
        g.drawRoundedRectangle (dropZoneBounds.toFloat().reduced (0.5f), 10.0f, dragHighlight ? 2.0f : 1.0f);
    }

    paintChip (g, verdictChipBounds, verdictColour, verdictTokenIndex >= 0);
    if (! verdictChipBounds.isEmpty() && verdictGlyph.isNotEmpty())
    {
        g.setColour (verdictColour);
        g.setFont (juce::FontOptions (17.0f, juce::Font::bold));
        g.drawText (verdictGlyph, verdictChipBounds.withTrimmedLeft (14).withWidth (24),
                    juce::Justification::centredLeft, false);
    }

    paintStateLegend (g);

    paintChip (g, armBadgeBounds, armedIndicator ? successColour : mutedColour, armedIndicator);
}

void AudioProvenanceCaptureAudioProcessorEditor::paintStateLegend (juce::Graphics& g)
{
    if (legendBounds.isEmpty())
        return;

    const juce::Font legendFont (juce::FontOptions (10.5f, juce::Font::bold));
    constexpr int tokenCount = static_cast<int> (std::size (stateTokens));
    constexpr float gap = 6.0f;

    float widths[tokenCount];
    float total = 0.0f;
    for (int i = 0; i < tokenCount; ++i)
    {
        widths[i] = juce::GlyphArrangement::getStringWidth (legendFont, stateTokens[i]) + 16.0f;
        total += widths[i];
    }

    const auto room = static_cast<float> (legendBounds.getWidth()) - gap * static_cast<float> (tokenCount - 1);
    if (room <= 0.0f)
        return;
    if (total > room)
        for (auto& width : widths)
            width *= room / total;

    auto x = static_cast<float> (legendBounds.getX());
    for (int i = 0; i < tokenCount; ++i)
    {
        const juce::Rectangle<float> pill (x, static_cast<float> (legendBounds.getY()),
                                           widths[i], static_cast<float> (legendBounds.getHeight()));
        const auto colour = stateTokenColour (i);
        const auto active = (i == verdictTokenIndex);

        g.setColour (active ? colour.withAlpha (0.20f) : insetColour);
        g.fillRoundedRectangle (pill, 8.0f);
        g.setColour (active ? colour : borderColour);
        g.drawRoundedRectangle (pill.reduced (0.5f), 8.0f, active ? 1.4f : 1.0f);
        g.setFont (legendFont);
        g.setColour (active ? colour : dimColour);
        g.drawFittedText (stateTokens[i], pill.toNearestInt(), juce::Justification::centred, 1, 0.6f);

        x += widths[i] + gap;
    }
}

void AudioProvenanceCaptureAudioProcessorEditor::resized()
{
    auto bounds = getLocalBounds().reduced (24);
    titleLabel.setBounds (bounds.removeFromTop (30));
    subtitleLabel.setBounds (bounds.removeFromTop (20));
    sessionIdLabel.setBounds (bounds.removeFromTop (18));
    bounds.removeFromTop (16);

    auto leftPanel = bounds.removeFromLeft (bounds.getWidth() * 40 / 100);
    bounds.removeFromLeft (16);
    observationPanelBounds = leftPanel;
    provenancePanelBounds = bounds;

    auto left = leftPanel.reduced (16);
    observationHeaderLabel.setBounds (left.removeFromTop (18));
    left.removeFromTop (8);
    captureChipBounds = left.removeFromTop (36);
    captureStatusLabel.setBounds (captureChipBounds.withTrimmedLeft (32));
    left.removeFromTop (10);
    formatLabel.setBounds (left.removeFromTop (22));
    lastBufferSeenLabel.setBounds (left.removeFromTop (22));
    hashChainLabel.setBounds (left.removeFromTop (22));
    lastHashLabel.setBounds (left.removeFromTop (22));
    left.removeFromTop (12);
    acknowledgementLabel.setBounds (left.removeFromTop (48));
    left.removeFromTop (8);
    responderLabel.setBounds (left.removeFromTop (48));
    left.removeFromTop (8);
    leftDividerBounds = left.removeFromTop (1);
    left.removeFromTop (12);
    observationDetailHeaderLabel.setBounds (left.removeFromTop (16));
    left.removeFromTop (4);
    throughputLabel.setBounds (left.removeFromTop (22));
    deliveryLabel.setBounds (left.removeFromTop (38));
    coverageLabel.setBounds (left.removeFromTop (38));
    left.removeFromTop (12);
    scopeLabel.setBounds (left.removeFromTop (50));
    left.removeFromTop (8);
    proofLevelLabel.setBounds (left.removeFromTop (52));

    auto right = provenancePanelBounds.reduced (16);
    provenanceHeaderLabel.setBounds (right.removeFromTop (18));
    right.removeFromTop (6);
    dropZoneBounds = right.removeFromTop (52);
    dropZoneLabel.setBounds (dropZoneBounds.reduced (10, 8));
    right.removeFromTop (10);
    verdictChipBounds = right.removeFromTop (38);
    verificationStateLabel.setBounds (verdictChipBounds.withTrimmedLeft (40));
    right.removeFromTop (6);
    legendBounds = right.removeFromTop (22);
    right.removeFromTop (10);
    verificationProvesLabel.setBounds (right.removeFromTop (40));
    verificationNotProvesLabel.setBounds (right.removeFromTop (40));
    right.removeFromTop (4);
    verificationDetailLabel.setBounds (right.removeFromTop (38));
    verificationSourceLabel.setBounds (right.removeFromTop (28));
    right.removeFromTop (10);
    rightDividerBounds = right.removeFromTop (1);
    right.removeFromTop (10);
    signingHeaderLabel.setBounds (right.removeFromTop (16));
    right.removeFromTop (6);
    auto armRow = right.removeFromTop (32);
    armBadgeBounds = armRow.removeFromLeft (132);
    armStateLabel.setBounds (armBadgeBounds);
    armRow.removeFromLeft (12);
    armButton.setBounds (armRow.removeFromLeft (190));
    right.removeFromTop (8);
    renderDisclosureLabel.setBounds (right.removeFromTop (40));
    identityLabel.setBounds (right.removeFromTop (38));
    right.removeFromTop (8);
    telemetryToggle.setBounds (right.removeFromTop (24));
    telemetryStateLabel.setBounds (right.removeFromTop (40));
}

bool AudioProvenanceCaptureAudioProcessorEditor::isInterestedInFileDrag (const juce::StringArray& files)
{
    for (const auto& path : files)
        if (isInspectableAudioPath (path))
            return true;
    return false;
}

void AudioProvenanceCaptureAudioProcessorEditor::fileDragEnter (const juce::StringArray&, int, int)
{
    dragHighlight = true;
    repaint (dropZoneBounds.expanded (3));
}

void AudioProvenanceCaptureAudioProcessorEditor::fileDragExit (const juce::StringArray&)
{
    dragHighlight = false;
    repaint (dropZoneBounds.expanded (3));
}

void AudioProvenanceCaptureAudioProcessorEditor::filesDropped (const juce::StringArray& files, int, int)
{
    dragHighlight = false;
    repaint (dropZoneBounds.expanded (3));

    for (const auto& path : files)
    {
        if (! isInspectableAudioPath (path))
            continue;
        audioProcessor.requestVerification (resolveInspectionTarget (juce::File (path)));
        updateProvenanceLabels();
        return;
    }
}

void AudioProvenanceCaptureAudioProcessorEditor::timerCallback()
{
    updateObservationLabels();
    updateProvenanceLabels();
}

void AudioProvenanceCaptureAudioProcessorEditor::updateProvenanceLabels()
{
    const auto result = audioProcessor.getVerificationResult();
    const auto presentation = presentOutcome (result.outcome, result.signatureChecked);
    const auto stateColour = presentation.tokenIndex >= 0
                                 ? stateTokenColour (presentation.tokenIndex)
                                 : mutedColour;

    const auto verdictChanged = presentation.tokenIndex != verdictTokenIndex
        || verdictColour != stateColour
        || verdictGlyph != presentation.glyph;
    verdictTokenIndex = presentation.tokenIndex;
    verdictColour = stateColour;
    verdictGlyph = presentation.glyph;

    verificationStateLabel.setText (presentation.headline, juce::dontSendNotification);
    verificationStateLabel.setColour (juce::Label::textColourId, stateColour);
    verificationProvesLabel.setText (presentation.proves, juce::dontSendNotification);
    verificationNotProvesLabel.setText (presentation.notProves, juce::dontSendNotification);

    verificationDetailLabel.setText (result.detail, juce::dontSendNotification);

    juce::String sourceText;
    if (result.fileName.isNotEmpty())
        sourceText << result.fileName << "  ·  ";
    if (result.fileSha256.isNotEmpty())
        sourceText << "SHA-256 " << result.fileSha256.substring (0, 16) << "…  ·  ";
    if (result.manifestPath.isNotEmpty())
        sourceText << juce::File (result.manifestPath).getFileName() << "  ·  ";
    if (result.evidenceSource.isNotEmpty())
        sourceText << result.evidenceSource;
    else
        sourceText << "The local verifier on port " << audioProcessor.getVerificationClient().getResponderPort()
                   << " is asked first; the plug-in's own evidence read answers when it stays silent.";
    verificationSourceLabel.setText (sourceText, juce::dontSendNotification);

    const auto armed = audioProcessor.isSigningArmed();
    const auto armChanged = armed != armedIndicator;
    armedIndicator = armed;

    armButton.setButtonText (armed ? "Disarm" : "Arm for signing");
    armButton.setColour (juce::TextButton::buttonColourId,
                         armed ? juce::Colour::fromRGB (24, 62, 52) : juce::Colour::fromRGB (28, 39, 52));
    armStateLabel.setText (armed ? "ARMED" : "DISARMED", juce::dontSendNotification);
    armStateLabel.setColour (juce::Label::textColourId, armed ? successColour : mutedColour);

    juce::String disclosureText;
    if (armed)
    {
        const auto armedAt = audioProcessor.getArmedAtUnixSeconds();
        disclosureText << "Armed";
        if (armedAt > 0)
            disclosureText << " at " << juce::Time (armedAt * 1000).formatted ("%H:%M:%S");
        disclosureText << ". The render dialog is not hooked: the daemon signs when it detects the exported file.";
    }
    else
    {
        disclosureText << "Nothing will be signed. Arm before you render: the render dialog is not hooked, "
                          "so signing happens only when the daemon detects the exported file.";
    }
    renderDisclosureLabel.setText (disclosureText, juce::dontSendNotification);

    const auto identity = audioProcessor.getSigningIdentityFingerprint();
    juce::String identityText;
    if (identity.isNotEmpty())
        identityText << "Key on this machine: local demo key " << identity
                     << ". Signing proves possession of that key; the chain is self-issued, "
                        "so signer identity is not established.";
    else
        identityText << "No local signing key was found. The daemon cannot sign until one exists.";
    identityLabel.setText (identityText, juce::dontSendNotification);

    const auto consent = audioProcessor.hasTelemetryConsent();
    if (telemetryToggle.getToggleState() != consent)
        telemetryToggle.setToggleState (consent, juce::dontSendNotification);

    auto& log = audioProcessor.getSessionActionLog();
    juce::String telemetryText;
    if (consent)
    {
        telemetryText << "ON · " << juce::String (static_cast<juce::int64> (log.getActionsRecorded()))
                      << " actions · last: " << log.getRecentActionSummary()
                      << " · nothing is transmitted · " << log.getLogFile().getFullPathName();
        if (log.getWriteFailures() > 0)
            telemetryText << " · " << juce::String (static_cast<juce::int64> (log.getWriteFailures()))
                          << " writes failed";
    }
    else
    {
        telemetryText << "OFF · no session action is captured, no file is written and nothing leaves this machine.";
    }
    telemetryStateLabel.setText (telemetryText, juce::dontSendNotification);
    telemetryStateLabel.setColour (juce::Label::textColourId, consent ? accentColour : mutedColour);

    if (verdictChanged || armChanged || result.completedAtMilliseconds != lastRenderedVerificationMilliseconds)
    {
        lastRenderedVerificationMilliseconds = result.completedAtMilliseconds;
        repaint();
    }
}

void AudioProvenanceCaptureAudioProcessorEditor::updateObservationLabels()
{
    const auto snapshot = audioProcessor.getAudioBufferObservationSnapshot();
    const auto nowMilliseconds = static_cast<std::uint64_t> (juce::Time::getMillisecondCounterHiRes());
    const auto hasRecentAudio = snapshot.lastNonSilentBufferSeenMilliseconds > 0
        && nowMilliseconds >= snapshot.lastNonSilentBufferSeenMilliseconds
        && nowMilliseconds - snapshot.lastNonSilentBufferSeenMilliseconds <= activeTimeoutMilliseconds;

    if (hasRecentAudio != activityActive)
    {
        activityActive = hasRecentAudio;
        repaint (captureChipBounds.expanded (2));
    }

    if (snapshot.lastBufferSeenMilliseconds > 0
        && snapshot.lastBufferSeenMilliseconds != lastRenderedBufferSeenMilliseconds)
    {
        lastRenderedBufferSeenMilliseconds = snapshot.lastBufferSeenMilliseconds;
        lastRenderedBufferSeenText = juce::Time::getCurrentTime().formatted ("%H:%M:%S");
    }

    captureStatusLabel.setText (hasRecentAudio ? "OBSERVING AUDIO ON THIS PATH"
                                               : "NO AUDIO ON THIS PATH",
                                juce::dontSendNotification);
    sessionIdLabel.setText (audioProcessor.getPluginInstanceId() + "  ·  "
                            + audioProcessor.getPluginCaptureSessionId(), juce::dontSendNotification);
    formatLabel.setText (juce::String ("Format: ") + juce::String (snapshot.sampleRateHz) + " Hz · "
                         + juce::String (snapshot.channelCount) + " ch · "
                         + juce::String (snapshot.bufferSizeSamples) + " samples per block",
                         juce::dontSendNotification);
    lastBufferSeenLabel.setText (juce::String ("Last buffer seen: ") + lastRenderedBufferSeenText,
                                 juce::dontSendNotification);

    auto& observer = audioProcessor.getAudioObserver();
    const auto windowsHashed = observer.getTotalWindowsHashed();
    const auto eventsEmitted = observer.getTotalEventsEmitted();

    hashChainLabel.setText (juce::String ("Hash chain: ")
                            + juce::String (static_cast<juce::int64> (windowsHashed)) + " windows, "
                            + juce::String (static_cast<juce::int64> (eventsEmitted)) + " events prepared",
                            juce::dontSendNotification);

    const auto lastHash = observer.getLastHash();
    lastHashLabel.setText (lastHash.isNotEmpty()
                               ? juce::String ("Last hash: ") + lastHash.substring (0, 24) + "…"
                               : juce::String ("Last hash: none yet"),
                           juce::dontSendNotification);

    throughputLabel.setText (juce::String ("Submitted: ")
                             + juce::String (static_cast<juce::int64> (observer.getBuffersSubmitted()))
                             + " buffers · "
                             + juce::String (static_cast<juce::int64> (observer.getSamplesSubmitted()))
                             + " samples",
                             juce::dontSendNotification);

    auto& emitter = audioProcessor.getEventEmitter();
    const auto acknowledgement = emitter.getAcknowledgementSnapshot();
    // Sequence-space comparison: getSendAccepted() is a success COUNT, so a
    // failed send shrank "missing" and the panel claimed missing=0 next to a
    // nonzero send-failed figure (a label stronger than the evidence).
    const auto highestPrepared = audioProcessor.getHighestPreparedSequence();
    const auto missing = highestPrepared > acknowledgement.highestAcceptedSequence
        ? highestPrepared - acknowledgement.highestAcceptedSequence : 0;

    deliveryLabel.setText (juce::String ("Prepared ")
                           + juce::String (static_cast<juce::int64> (eventsEmitted))
                           + " · UDP attempted "
                           + juce::String (static_cast<juce::int64> (emitter.getSendAttempts()))
                           + " · locally emitted "
                           + juce::String (static_cast<juce::int64> (emitter.getSendAccepted()))
                           + " · send failed "
                           + juce::String (static_cast<juce::int64> (emitter.getSendFailures())),
                           juce::dontSendNotification);

    juce::String acknowledgementState = "NOT YET ACKNOWLEDGED";
    auto acknowledgementColour = mutedColour;
    if (acknowledgement.acknowledgementsProcessed > 0)
    {
        if (acknowledgement.stale)
        {
            acknowledgementState = "STALE";
            acknowledgementColour = warningColour;
        }
        else if (acknowledgement.lastReceiptRejected)
        {
            acknowledgementState = "REJECTED BY THE DAEMON";
            acknowledgementColour = alertColour;
        }
        else if (acknowledgement.streamChainBreaks > 0)
        {
            acknowledgementState = "ACKNOWLEDGED, CHAIN BREAK SEEN";
            acknowledgementColour = alertColour;
        }
        else if (acknowledgement.streamGaps > 0)
        {
            acknowledgementState = "ACKNOWLEDGED, GAP SEEN";
            acknowledgementColour = warningColour;
        }
        else if (acknowledgement.lastReceiptAccepted)
        {
            acknowledgementState = "ACKNOWLEDGED BY THE DAEMON";
            acknowledgementColour = successColour;
        }
    }
    acknowledgementLabel.setText (
        juce::String ("Daemon receipt: ") + acknowledgementState
        + "\naccepted through " + juce::String (static_cast<juce::int64> (acknowledgement.highestAcceptedSequence))
        + " · contiguous through "
        + juce::String (static_cast<juce::int64> (acknowledgement.highestContiguousSequence))
        + " · missing or pending " + juce::String (static_cast<juce::int64> (missing)),
        juce::dontSendNotification);
    acknowledgementLabel.setColour (juce::Label::textColourId, acknowledgementColour);

    const auto& verifier = audioProcessor.getVerificationClient();
    responderLabel.setText (juce::String ("Local verifier: port ")
                            + juce::String (verifier.getResponderPort())
                            + (verifier.isResponderSocketBound() ? ", reply socket bound"
                                                                 : ", reply socket NOT bound")
                            + " · asked " + juce::String (static_cast<juce::int64> (verifier.getResponderRequests()))
                            + " · answered " + juce::String (static_cast<juce::int64> (verifier.getResponderAnswers()))
                            + " · timed out " + juce::String (static_cast<juce::int64> (verifier.getResponderTimeouts())),
                            juce::dontSendNotification);

    coverageLabel.setText (juce::String ("FIFO dropped ")
                           + juce::String (static_cast<juce::int64> (observer.getFifoSamplesDropped())) + " samples / "
                           + juce::String (static_cast<juce::int64> (observer.getFifoWindowsDropped())) + " windows · "
                           + "ACK gaps " + juce::String (static_cast<juce::int64> (acknowledgement.streamGaps))
                           + " · rejected "
                           + juce::String (static_cast<juce::int64> (acknowledgement.streamRejections))
                           + " · mismatched ACKs ignored "
                           + juce::String (static_cast<juce::int64> (acknowledgement.sessionMismatchesIgnored)),
                           juce::dontSendNotification);
}
