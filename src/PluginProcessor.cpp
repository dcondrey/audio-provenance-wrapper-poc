#include "PluginProcessor.h"
#include "PluginEditor.h"
#include "SafeJson.h"

#include <cmath>

namespace
{
constexpr auto audioDetectionThreshold = 1.0e-5;

std::uint64_t getMonotonicMilliseconds() noexcept
{
    return static_cast<std::uint64_t> (juce::Time::getMillisecondCounterHiRes());
}

// The local demo signing identity is the daemon's Ed25519 public key. Absent
// key means the UI must say no identity is available, never invent one.
juce::String readLocalSigningIdentity()
{
    const auto keyFile = juce::File::getSpecialLocation (juce::File::userHomeDirectory)
                             .getChildFile (".apw")
                             .getChildFile ("demo_ed25519_public.key");
    juce::MemoryBlock keyBytes;
    if (! keyFile.existsAsFile() || ! keyFile.loadFileAsData (keyBytes) || keyBytes.getSize() != 32)
        return {};
    return juce::String::toHexString (keyBytes.getData(), 8, 0);
}
}

AudioProvenanceCaptureAudioProcessor::AudioProvenanceCaptureAudioProcessor (int daemonPort)
    : AudioProcessor (BusesProperties()
                          .withInput  ("Input",  juce::AudioChannelSet::stereo(), true)
                          .withOutput ("Output", juce::AudioChannelSet::stereo(), true)),
      pluginInstanceId ("plugin-" + juce::Uuid().toString().substring (0, 12)),
      pluginCaptureSessionId ("plugin-session-" + juce::Uuid().toString().substring (0, 12)),
      eventEmitter (pluginInstanceId, pluginCaptureSessionId, "127.0.0.1", daemonPort),
      sessionActionLog (pluginCaptureSessionId)
{
    audioObserver.start ([this] (const juce::String& jsonEvent)
    {
        auto parsed = apw::safejson::parseBounded (jsonEvent);
        if (auto* object = parsed.getDynamicObject())
        {
            const auto sequence = eventSequence.fetch_add (1, std::memory_order_relaxed) + 1;
            object->setProperty ("plugin_instance_id", pluginInstanceId);
            object->setProperty ("plugin_capture_session_id", pluginCaptureSessionId);
            object->setProperty ("event_sequence", static_cast<juce::int64> (sequence));

            auto* telemetry = new juce::DynamicObject();
            telemetry->setProperty ("buffers_submitted", static_cast<juce::int64> (audioObserver.getBuffersSubmitted()));
            telemetry->setProperty ("samples_submitted", static_cast<juce::int64> (audioObserver.getSamplesSubmitted()));
            telemetry->setProperty ("windows_hashed", static_cast<juce::int64> (
                audioObserver.getTotalWindowsHashed()));
            telemetry->setProperty ("fifo_samples_dropped", static_cast<juce::int64> (audioObserver.getFifoSamplesDropped()));
            telemetry->setProperty ("fifo_windows_dropped", static_cast<juce::int64> (audioObserver.getFifoWindowsDropped()));
            telemetry->setProperty ("midi_events_dropped", static_cast<juce::int64> (audioObserver.getMidiEventsDropped()));
            telemetry->setProperty ("midi_unsupported_dropped", static_cast<juce::int64> (audioObserver.getUnsupportedMidiEventsDropped()));
            telemetry->setProperty ("midi_cc_subthreshold_discarded", static_cast<juce::int64> (
                audioObserver.getSubThresholdCcChangesDiscarded()));
            telemetry->setProperty ("bypassed_buffers", static_cast<juce::int64> (
                audioObserver.getBypassedBuffers()));
            telemetry->setProperty ("bypassed_samples", static_cast<juce::int64> (
                audioObserver.getBypassedSamples()));
            telemetry->setProperty ("observation_discontinuities", static_cast<juce::int64> (
                audioObserver.getObservationDiscontinuities()));
            telemetry->setProperty ("events_prepared", static_cast<juce::int64> (sequence));
            telemetry->setProperty ("udp_sends_attempted", static_cast<juce::int64> (eventEmitter.getSendAttempts() + 1));
            telemetry->setProperty ("udp_sends_failed", static_cast<juce::int64> (eventEmitter.getSendFailures()));
            const auto acknowledgement = eventEmitter.getAcknowledgementSnapshot();
            telemetry->setProperty ("daemon_acknowledgements_processed", static_cast<juce::int64> (
                acknowledgement.acknowledgementsProcessed));
            telemetry->setProperty ("daemon_highest_accepted_sequence", static_cast<juce::int64> (
                acknowledgement.highestAcceptedSequence));
            telemetry->setProperty ("daemon_highest_contiguous_sequence", static_cast<juce::int64> (
                acknowledgement.highestContiguousSequence));
            telemetry->setProperty ("daemon_ack_session_mismatches_ignored", static_cast<juce::int64> (
                acknowledgement.sessionMismatchesIgnored));
            telemetry->setProperty ("daemon_malformed_acknowledgements", static_cast<juce::int64> (
                eventEmitter.getMalformedAcknowledgements()));
            telemetry->setProperty ("daemon_restarts_observed", static_cast<juce::int64> (
                acknowledgement.daemonRestartsObserved));
            object->setProperty ("telemetry", juce::var (telemetry));
            eventEmitter.sendEvent (juce::JSON::toString (parsed, true));
        }
    });
}

AudioProvenanceCaptureAudioProcessor::~AudioProvenanceCaptureAudioProcessor()
{
    audioObserver.stop();
}

void AudioProvenanceCaptureAudioProcessor::prepareToPlay (double sampleRate, int samplesPerBlock)
{
    observedSampleRateHz.store (static_cast<int> (std::lround (sampleRate)), std::memory_order_relaxed);
    observedBufferSizeSamples.store (samplesPerBlock, std::memory_order_relaxed);
    observedChannelCount.store (getTotalNumInputChannels(), std::memory_order_relaxed);
    lastBufferHadAudio.store (false, std::memory_order_relaxed);
    lastCallbackWasBypassed.store (false, std::memory_order_relaxed);
    lastBufferSeenMilliseconds.store (0, std::memory_order_relaxed);
    lastNonSilentBufferSeenMilliseconds.store (0, std::memory_order_relaxed);

    audioObserver.updateSessionConfig (static_cast<int> (std::lround (sampleRate)),
                                        getTotalNumInputChannels(),
                                        samplesPerBlock);

    doubleConversionBuffer.setSize (getTotalNumInputChannels(), samplesPerBlock);
}

void AudioProvenanceCaptureAudioProcessor::releaseResources()
{
    // A host disable/enable, freeze, or render boundary must not splice two
    // unrelated callback eras into one observation hash window.
    audioObserver.markLifecycleDiscontinuity();
}

void AudioProvenanceCaptureAudioProcessor::reset()
{
    audioObserver.markLifecycleDiscontinuity();
}

bool AudioProvenanceCaptureAudioProcessor::isBusesLayoutSupported (const BusesLayout& layouts) const
{
    const auto& input = layouts.getMainInputChannelSet();
    const auto& output = layouts.getMainOutputChannelSet();

    if (input.isDisabled() || output.isDisabled())
        return false;

    if (input != output)
        return false;

    return input == juce::AudioChannelSet::mono()
        || input == juce::AudioChannelSet::stereo();
}

void AudioProvenanceCaptureAudioProcessor::processBlock (juce::AudioBuffer<float>& buffer,
                                                         juce::MidiBuffer& midiMessages)
{
    audioObserver.setBypassActive (false);
    observeAudioBuffer (buffer);

    // Feed the granular observation pipeline.
    const int numCh   = buffer.getNumChannels();
    const int numSamp = buffer.getNumSamples();
    audioObserver.updateSessionConfig (
        observedSampleRateHz.load (std::memory_order_relaxed), numCh, numSamp);
    audioObserver.pushAudioBlock (buffer.getArrayOfReadPointers(), numCh, numSamp);
    audioObserver.pushMidiMessages (midiMessages);
    audioObserver.updateTransportState (getPlayHead());

    passThrough (buffer);
}

void AudioProvenanceCaptureAudioProcessor::processBlock (juce::AudioBuffer<double>& buffer,
                                                         juce::MidiBuffer& midiMessages)
{
    audioObserver.setBypassActive (false);
    observeAudioBuffer (buffer);

    // Convert double buffer to float for the observation pipeline.
    const int numCh   = buffer.getNumChannels();
    const int numSamp = buffer.getNumSamples();
    audioObserver.updateSessionConfig (
        observedSampleRateHz.load (std::memory_order_relaxed), numCh, numSamp);

    if (doubleConversionBuffer.getNumChannels() < numCh
        || doubleConversionBuffer.getNumSamples() < numSamp)
    {
        audioObserver.recordExternalAudioDrop (numSamp);
        audioObserver.pushMidiMessages (midiMessages);
        audioObserver.updateTransportState (getPlayHead());
        passThrough (buffer);
        return;
    }

    for (int ch = 0; ch < numCh; ++ch)
    {
        const auto* src = buffer.getReadPointer (ch);
        auto* dst = doubleConversionBuffer.getWritePointer (ch);
        for (int i = 0; i < numSamp; ++i)
            dst[i] = static_cast<float> (src[i]);
    }

    audioObserver.pushAudioBlock (doubleConversionBuffer.getArrayOfReadPointers(), numCh, numSamp);
    audioObserver.pushMidiMessages (midiMessages);
    audioObserver.updateTransportState (getPlayHead());

    passThrough (buffer);
}

void AudioProvenanceCaptureAudioProcessor::processBlockBypassed (
    juce::AudioBuffer<float>& buffer, juce::MidiBuffer&)
{
    handleBypassedBuffer (buffer);
}

void AudioProvenanceCaptureAudioProcessor::processBlockBypassed (
    juce::AudioBuffer<double>& buffer, juce::MidiBuffer&)
{
    handleBypassedBuffer (buffer);
}

template <typename SampleType>
void AudioProvenanceCaptureAudioProcessor::passThrough (juce::AudioBuffer<SampleType>& buffer)
{
    const auto totalInputChannels = getTotalNumInputChannels();
    const auto totalOutputChannels = getTotalNumOutputChannels();

    for (auto channel = totalInputChannels; channel < totalOutputChannels; ++channel)
        buffer.clear (channel, 0, buffer.getNumSamples());
}

template <typename SampleType>
void AudioProvenanceCaptureAudioProcessor::observeAudioBuffer (const juce::AudioBuffer<SampleType>& buffer) noexcept
{
    const auto inputChannels = juce::jmin (getTotalNumInputChannels(), buffer.getNumChannels());
    const auto numSamples = buffer.getNumSamples();
    auto hasAudio = false;

    for (auto channel = 0; channel < inputChannels && ! hasAudio; ++channel)
    {
        const auto* samples = buffer.getReadPointer (channel);

        for (auto sample = 0; sample < numSamples; ++sample)
        {
            if (std::abs (samples[sample]) > static_cast<SampleType> (audioDetectionThreshold))
            {
                hasAudio = true;
                break;
            }
        }
    }

    const auto nowMilliseconds = getMonotonicMilliseconds();
    observedChannelCount.store (inputChannels, std::memory_order_relaxed);
    observedBufferSizeSamples.store (numSamples, std::memory_order_relaxed);
    lastBufferSeenMilliseconds.store (nowMilliseconds, std::memory_order_relaxed);
    lastBufferHadAudio.store (hasAudio, std::memory_order_relaxed);
    lastCallbackWasBypassed.store (false, std::memory_order_relaxed);

    if (hasAudio)
        lastNonSilentBufferSeenMilliseconds.store (nowMilliseconds, std::memory_order_relaxed);
}

template <typename SampleType>
void AudioProvenanceCaptureAudioProcessor::handleBypassedBuffer (
    juce::AudioBuffer<SampleType>& buffer) noexcept
{
    // Supported layouts have identical input and output channel sets, so doing
    // literally nothing is the bit-identical bypass. Only atomics are touched.
    const auto numSamples = buffer.getNumSamples();
    observedChannelCount.store (buffer.getNumChannels(), std::memory_order_relaxed);
    observedBufferSizeSamples.store (numSamples, std::memory_order_relaxed);
    lastBufferSeenMilliseconds.store (getMonotonicMilliseconds(), std::memory_order_relaxed);
    lastCallbackWasBypassed.store (true, std::memory_order_relaxed);
    audioObserver.recordBypassedBlock (numSamples);
}

AudioProvenanceCaptureAudioProcessor::AudioBufferObservationSnapshot
AudioProvenanceCaptureAudioProcessor::getAudioBufferObservationSnapshot() const noexcept
{
    AudioBufferObservationSnapshot snapshot;
    snapshot.channelCount = observedChannelCount.load (std::memory_order_relaxed);
    snapshot.sampleRateHz = observedSampleRateHz.load (std::memory_order_relaxed);
    snapshot.bufferSizeSamples = observedBufferSizeSamples.load (std::memory_order_relaxed);
    snapshot.lastBufferSeenMilliseconds = lastBufferSeenMilliseconds.load (std::memory_order_relaxed);
    snapshot.lastNonSilentBufferSeenMilliseconds = lastNonSilentBufferSeenMilliseconds.load (std::memory_order_relaxed);
    snapshot.lastBufferHadAudio = lastBufferHadAudio.load (std::memory_order_relaxed);
    snapshot.lastCallbackWasBypassed = lastCallbackWasBypassed.load (std::memory_order_relaxed);
    return snapshot;
}

juce::AudioProcessorEditor* AudioProvenanceCaptureAudioProcessor::createEditor()
{
    return new AudioProvenanceCaptureAudioProcessorEditor (*this);
}

bool AudioProvenanceCaptureAudioProcessor::hasEditor() const
{
    return true;
}

const juce::String AudioProvenanceCaptureAudioProcessor::getName() const
{
    return JucePlugin_Name;
}

bool AudioProvenanceCaptureAudioProcessor::acceptsMidi() const
{
    return true;
}

bool AudioProvenanceCaptureAudioProcessor::producesMidi() const
{
    return false;
}

bool AudioProvenanceCaptureAudioProcessor::isMidiEffect() const
{
    return false;
}

double AudioProvenanceCaptureAudioProcessor::getTailLengthSeconds() const
{
    return 0.0;
}

int AudioProvenanceCaptureAudioProcessor::getNumPrograms()
{
    return 1;
}

int AudioProvenanceCaptureAudioProcessor::getCurrentProgram()
{
    return 0;
}

void AudioProvenanceCaptureAudioProcessor::setCurrentProgram (int)
{
}

const juce::String AudioProvenanceCaptureAudioProcessor::getProgramName (int)
{
    return {};
}

void AudioProvenanceCaptureAudioProcessor::changeProgramName (int, const juce::String&)
{
}

void AudioProvenanceCaptureAudioProcessor::getStateInformation (juce::MemoryBlock& destData)
{
    juce::XmlElement state ("APW_PLUGIN_STATE");
    state.setAttribute ("state_version", 1);
    state.setAttribute ("signing_armed", signingArmed.load (std::memory_order_relaxed) ? 1 : 0);
    state.setAttribute ("armed_at_unix_seconds",
                        juce::String (armedAtUnixSeconds.load (std::memory_order_relaxed)));
    state.setAttribute ("session_action_telemetry_consent", sessionActionLog.hasConsent() ? 1 : 0);
    copyXmlToBinary (state, destData);
}

void AudioProvenanceCaptureAudioProcessor::setStateInformation (const void* data, int sizeInBytes)
{
    // IMPORTANT: host state is untrusted input. Anything unparseable restores
    // the safe defaults: disarmed, and telemetry consent withheld.
    signingArmed.store (false, std::memory_order_relaxed);
    armedAtUnixSeconds.store (0, std::memory_order_relaxed);
    sessionActionLog.setConsent (false);

    if (data == nullptr || sizeInBytes <= 0)
        return;

    if (! hasSafeStateEnvelope (data, sizeInBytes))
    {
        rejectedStateRestores.fetch_add (1, std::memory_order_relaxed);
        return;
    }

    const std::unique_ptr<juce::XmlElement> state (getXmlFromBinary (data, sizeInBytes));
    if (state == nullptr || ! state->hasTagName ("APW_PLUGIN_STATE")
        || state->getIntAttribute ("state_version", -1) != 1
        || state->getFirstChildElement() != nullptr)
    {
        rejectedStateRestores.fetch_add (1, std::memory_order_relaxed);
        return;
    }

    const auto armedText = state->getStringAttribute ("signing_armed");
    const auto consentText = state->getStringAttribute ("session_action_telemetry_consent");
    const auto timestampText = state->getStringAttribute ("armed_at_unix_seconds");
    const auto isBooleanText = [] (const juce::String& value)
    {
        return value == "0" || value == "1";
    };

    if (! isBooleanText (armedText) || ! isBooleanText (consentText)
        || timestampText.isEmpty() || timestampText.length() > 20
        || ! timestampText.containsOnly ("0123456789"))
    {
        rejectedStateRestores.fetch_add (1, std::memory_order_relaxed);
        return;
    }

    const auto restoredArmed = armedText == "1";
    const auto restoredTimestamp = timestampText.getLargeIntValue();
    if (restoredTimestamp < 0 || (restoredArmed && restoredTimestamp == 0)
        || (! restoredArmed && restoredTimestamp != 0))
    {
        rejectedStateRestores.fetch_add (1, std::memory_order_relaxed);
        return;
    }

    signingArmed.store (restoredArmed, std::memory_order_relaxed);
    armedAtUnixSeconds.store (restoredTimestamp, std::memory_order_relaxed);
    sessionActionLog.setConsent (consentText == "1");
}

bool AudioProvenanceCaptureAudioProcessor::hasSafeStateEnvelope (
    const void* data, int sizeInBytes) noexcept
{
    constexpr juce::uint32 xmlStateMagic = 0x21324356;
    if (data == nullptr || sizeInBytes <= 8 || sizeInBytes > maxPluginStateBytes
        || juce::ByteOrder::littleEndianInt (data) != xmlStateMagic)
        return false;

    const auto declaredLength = static_cast<int> (juce::ByteOrder::littleEndianInt (
        static_cast<const char*> (data) + 4));
    if (declaredLength <= 0 || declaredLength > sizeInBytes - 8)
        return false;

    const auto* xml = static_cast<const char*> (data) + 8;
    int openingAngles = 0;
    for (int index = 0; index < declaredLength; ++index)
    {
        const auto byte = static_cast<unsigned char> (xml[index]);
        if (byte == 0 || byte >= 0x80)
            return false;
        if (byte == '<' && ++openingAngles > 2)
            return false;
        if (byte == '<' && index + 1 < declaredLength && xml[index + 1] == '!')
            return false;
    }
    // JUCE emits an XML declaration plus one self-closing root. A caller may
    // also supply the root without the declaration. More element boundaries
    // were rejected above before the recursive XML parser runs.
    return openingAngles == 1 || openingAngles == 2;
}

void AudioProvenanceCaptureAudioProcessor::requestVerification (const juce::File& file)
{
    sessionActionLog.record ("verification_requested", file.getFileName());
    verificationClient.requestVerification (file);
}

void AudioProvenanceCaptureAudioProcessor::setSigningArmed (bool shouldBeArmed)
{
    signingArmed.store (shouldBeArmed, std::memory_order_relaxed);
    armedAtUnixSeconds.store (shouldBeArmed ? juce::Time::currentTimeMillis() / 1000 : 0,
                              std::memory_order_relaxed);
    sessionActionLog.record (shouldBeArmed ? "signing_armed" : "signing_disarmed",
                             shouldBeArmed
                                 ? "Operator armed this session for signing before rendering."
                                 : "Operator disarmed this session.");
}

juce::String AudioProvenanceCaptureAudioProcessor::getSigningIdentityFingerprint() const
{
    // The daemon creates the keypair lazily, on its first signature, which on a
    // fresh machine is long after this plug-in was constructed. A one-shot read
    // leaves the editor asserting signing is impossible for the whole session.
    const auto nowMilliseconds = getMonotonicMilliseconds();
    const juce::ScopedLock lock (signingIdentityLock);

    if (signingIdentityCheckedAtMilliseconds == 0
        || nowMilliseconds - signingIdentityCheckedAtMilliseconds >= signingIdentityRefreshMilliseconds)
    {
        signingIdentityCheckedAtMilliseconds = nowMilliseconds;
        signingIdentityFingerprint = readLocalSigningIdentity();
    }
    return signingIdentityFingerprint;
}

void AudioProvenanceCaptureAudioProcessor::setTelemetryConsent (bool granted)
{
    sessionActionLog.setConsent (granted);
    if (granted)
        sessionActionLog.record ("telemetry_consent_granted",
                                 "Local session action capture switched on by the operator.");
}

juce::AudioProcessor* JUCE_CALLTYPE createPluginFilter()
{
    return new AudioProvenanceCaptureAudioProcessor();
}
