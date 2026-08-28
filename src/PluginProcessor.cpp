#include "PluginProcessor.h"
#include "PluginEditor.h"

#include <cmath>

namespace
{
constexpr auto audioDetectionThreshold = 1.0e-5;

std::uint64_t getMonotonicMilliseconds() noexcept
{
    return static_cast<std::uint64_t> (juce::Time::getMillisecondCounterHiRes());
}
}

AudioProvenanceCaptureAudioProcessor::AudioProvenanceCaptureAudioProcessor()
    : AudioProcessor (BusesProperties()
                          .withInput  ("Input",  juce::AudioChannelSet::stereo(), true)
                          .withOutput ("Output", juce::AudioChannelSet::stereo(), true)),
      pluginInstanceId ("plugin-" + juce::Uuid().toString().substring (0, 12)),
      pluginCaptureSessionId ("plugin-session-" + juce::Uuid().toString().substring (0, 12)),
      eventEmitter (pluginInstanceId, pluginCaptureSessionId)
{
    audioObserver.start ([this] (const juce::String& jsonEvent)
    {
        auto parsed = juce::JSON::parse (jsonEvent);
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
    lastBufferSeenMilliseconds.store (0, std::memory_order_relaxed);
    lastNonSilentBufferSeenMilliseconds.store (0, std::memory_order_relaxed);

    audioObserver.updateSessionConfig (static_cast<int> (std::lround (sampleRate)),
                                        getTotalNumInputChannels(),
                                        samplesPerBlock);

    doubleConversionBuffer.setSize (getTotalNumInputChannels(), samplesPerBlock);
}

void AudioProvenanceCaptureAudioProcessor::releaseResources()
{
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
    observeAudioBuffer (buffer);

    // Feed the granular observation pipeline.
    const int numCh   = buffer.getNumChannels();
    const int numSamp = buffer.getNumSamples();
    audioObserver.pushAudioBlock (buffer.getArrayOfReadPointers(), numCh, numSamp);
    audioObserver.pushMidiMessages (midiMessages);
    audioObserver.updateTransportState (getPlayHead());

    passThrough (buffer);
}

void AudioProvenanceCaptureAudioProcessor::processBlock (juce::AudioBuffer<double>& buffer,
                                                         juce::MidiBuffer& midiMessages)
{
    observeAudioBuffer (buffer);

    // Convert double buffer to float for the observation pipeline.
    const int numCh   = buffer.getNumChannels();
    const int numSamp = buffer.getNumSamples();

    if (doubleConversionBuffer.getNumChannels() < numCh
        || doubleConversionBuffer.getNumSamples() < numSamp)
    {
        audioObserver.recordExternalAudioDrop (numSamp);
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

    if (hasAudio)
        lastNonSilentBufferSeenMilliseconds.store (nowMilliseconds, std::memory_order_relaxed);
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

void AudioProvenanceCaptureAudioProcessor::getStateInformation (juce::MemoryBlock&)
{
}

void AudioProvenanceCaptureAudioProcessor::setStateInformation (const void*, int)
{
}

juce::AudioProcessor* JUCE_CALLTYPE createPluginFilter()
{
    return new AudioProvenanceCaptureAudioProcessor();
}
