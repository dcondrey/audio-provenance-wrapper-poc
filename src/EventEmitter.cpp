#include "EventEmitter.h"
#include "SafeJson.h"

#include <algorithm>

namespace apw
{

EventEmitter::EventEmitter (const juce::String& pluginInstanceId,
                            const juce::String& pluginCaptureSessionId,
                            const juce::String& host,
                            int port)
    : juce::Thread ("DaemonAcknowledgementReceiver"),
      expectedPluginInstanceId (pluginInstanceId),
      expectedPluginCaptureSessionId (pluginCaptureSessionId),
      targetHost (host),
      targetPort (port)
{
    // REQUIRED: loopback only. bindToPort(0) alone binds INADDR_ANY and
    // hands every interface a path to this parser.
    if (socket.bindToPort (0, "127.0.0.1"))
    {
        // JUCE switches a datagram socket to non-blocking mode when read(...,
        // false) is first used. Do that before the first observation can send:
        // a saturated UDP buffer must return a counted failure, never strand
        // the observer or its bounded shutdown flush inside sendto().
        char ignored = 0;
        (void) socket.read (&ignored, 1, false);
        startThread (juce::Thread::Priority::normal);
    }
}

EventEmitter::~EventEmitter()
{
    signalThreadShouldExit();
    socket.shutdown();
    stopThread (250);
}

bool EventEmitter::sendEvent (const juce::String& jsonEvent)
{
    sendAttempts.fetch_add (1, std::memory_order_relaxed);
    const auto expected = static_cast<int> (jsonEvent.getNumBytesAsUTF8());
    if (expected <= 0 || expected > maxEventBytes)
    {
        sendFailures.fetch_add (1, std::memory_order_relaxed);
        return false;
    }
    const auto written = socket.write (targetHost, targetPort, jsonEvent.toRawUTF8(), expected);
    if (written == expected)
    {
        sendAccepted.fetch_add (1, std::memory_order_relaxed);
        return true;
    }
    sendFailures.fetch_add (1, std::memory_order_relaxed);
    return false;
}

void EventEmitter::run()
{
    char buffer[8192] {};
    while (! threadShouldExit())
    {
        if (socket.waitUntilReady (true, 200) <= 0)
            continue;
        const auto bytesRead = socket.read (buffer, static_cast<int> (sizeof (buffer) - 1), false);
        if (bytesRead <= 0)
            continue;
        buffer[bytesRead] = '\0';
        processAcknowledgement (juce::String::fromUTF8 (buffer, bytesRead));
    }
}

void EventEmitter::processAcknowledgement (const juce::String& jsonAcknowledgement)
{
    const auto parsed = safejson::parseBounded (jsonAcknowledgement);
    const auto* object = parsed.getDynamicObject();
    ack::Fields fields;
    if (object != nullptr)
    {
        fields.envelopeValid = object->getProperty ("message_type").toString()
                == "daemon_receipt_acknowledgement"
            && object->getProperty ("protocol").toString() == "apw-local-udp-ack-v1";
        fields.pluginInstanceId = object->getProperty ("plugin_instance_id").toString().toStdString();
        fields.pluginCaptureSessionId = object->getProperty (
            "plugin_capture_session_id").toString().toStdString();
        fields.daemonInstanceId = object->getProperty ("daemon_instance_id").toString().toStdString();
    }

    const auto readCounter = [object] (const char* name, std::uint64_t& destination)
    {
        if (object == nullptr)
            return false;
        const auto value = object->getProperty (name);
        if (! value.isInt() && ! value.isInt64())
            return false;
        const auto signedValue = static_cast<juce::int64> (value);
        if (signedValue < 0)
            return false;
        destination = static_cast<std::uint64_t> (signedValue);
        return true;
    };
    if (object != nullptr)
    {
        const auto acceptedValue = object->getProperty ("accepted");
        fields.acceptedFieldValid = acceptedValue.isBool();
        fields.accepted = fields.acceptedFieldValid && static_cast<bool> (acceptedValue);
    }
    fields.countersValid = readCounter (
            "highest_accepted_sequence", fields.highestAcceptedSequence)
        && readCounter ("highest_contiguous_sequence", fields.highestContiguousSequence)
        && readCounter ("stream_gaps", fields.streamGaps)
        && readCounter ("stream_rejections", fields.streamRejections)
        && readCounter ("stream_chain_breaks", fields.streamChainBreaks);

    const auto update = ack::apply (
        fields, expectedPluginInstanceId.toStdString(),
        expectedPluginCaptureSessionId.toStdString(), acknowledgementState);
    if (update.decision == ack::Decision::malformed)
    {
        malformedAcknowledgements.fetch_add (1, std::memory_order_relaxed);
        return;
    }
    if (update.decision == ack::Decision::scopeMismatch)
    {
        sessionMismatchesIgnored.fetch_add (1, std::memory_order_relaxed);
        return;
    }

    acknowledgementState = update.state;
    if (update.daemonRestarted)
        daemonRestartsObserved.fetch_add (1, std::memory_order_relaxed);
    highestAcceptedSequence.store (update.state.highestAcceptedSequence, std::memory_order_relaxed);
    highestContiguousSequence.store (
        update.state.highestContiguousSequence, std::memory_order_relaxed);
    streamGaps.store (update.state.streamGaps, std::memory_order_relaxed);
    streamRejections.store (update.state.streamRejections, std::memory_order_relaxed);
    streamChainBreaks.store (update.state.streamChainBreaks, std::memory_order_relaxed);
    lastReceiptAccepted.store (update.state.lastReceiptAccepted, std::memory_order_relaxed);
    lastReceiptRejected.store (! update.state.lastReceiptAccepted, std::memory_order_relaxed);
    lastAcknowledgementMilliseconds.store (
        static_cast<std::uint64_t> (juce::Time::getMillisecondCounterHiRes()),
        std::memory_order_relaxed);
    acknowledgementsProcessed.fetch_add (1, std::memory_order_relaxed);
}

EventEmitter::AcknowledgementSnapshot EventEmitter::getAcknowledgementSnapshot() const noexcept
{
    AcknowledgementSnapshot snapshot;
    snapshot.acknowledgementsProcessed = acknowledgementsProcessed.load (std::memory_order_relaxed);
    snapshot.highestAcceptedSequence = highestAcceptedSequence.load (std::memory_order_relaxed);
    snapshot.highestContiguousSequence = highestContiguousSequence.load (std::memory_order_relaxed);
    snapshot.streamGaps = streamGaps.load (std::memory_order_relaxed);
    snapshot.streamRejections = streamRejections.load (std::memory_order_relaxed);
    snapshot.streamChainBreaks = streamChainBreaks.load (std::memory_order_relaxed);
    snapshot.sessionMismatchesIgnored = sessionMismatchesIgnored.load (std::memory_order_relaxed);
    snapshot.daemonRestartsObserved = daemonRestartsObserved.load (std::memory_order_relaxed);
    snapshot.lastAcknowledgementMilliseconds = lastAcknowledgementMilliseconds.load (
        std::memory_order_relaxed);
    snapshot.lastReceiptAccepted = lastReceiptAccepted.load (std::memory_order_relaxed);
    snapshot.lastReceiptRejected = lastReceiptRejected.load (std::memory_order_relaxed);
    const auto now = static_cast<std::uint64_t> (juce::Time::getMillisecondCounterHiRes());
    snapshot.stale = snapshot.lastAcknowledgementMilliseconds > 0
        && now >= snapshot.lastAcknowledgementMilliseconds
        && now - snapshot.lastAcknowledgementMilliseconds > acknowledgementStaleMilliseconds;
    return snapshot;
}

} // namespace apw
