#include "EventEmitter.h"

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
    if (socket.bindToPort (0))
        startThread (juce::Thread::Priority::normal);
}

EventEmitter::~EventEmitter()
{
    signalThreadShouldExit();
    socket.shutdown();
    stopThread (1000);
}

bool EventEmitter::sendEvent (const juce::String& jsonEvent)
{
    sendAttempts.fetch_add (1, std::memory_order_relaxed);
    const auto expected = static_cast<int> (jsonEvent.getNumBytesAsUTF8());
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
    const auto parsed = juce::JSON::parse (jsonAcknowledgement);
    const auto* object = parsed.getDynamicObject();
    if (object == nullptr
        || object->getProperty ("message_type").toString() != "daemon_receipt_acknowledgement"
        || object->getProperty ("protocol").toString() != "apw-local-udp-ack-v1")
        return;

    if (object->getProperty ("plugin_instance_id").toString() != expectedPluginInstanceId
        || object->getProperty ("plugin_capture_session_id").toString()
            != expectedPluginCaptureSessionId)
    {
        sessionMismatchesIgnored.fetch_add (1, std::memory_order_relaxed);
        return;
    }

    const auto daemonId = object->getProperty ("daemon_instance_id").toString();
    const auto currentDaemonHash = static_cast<std::int64_t> (daemonId.hashCode64());
    const auto previousDaemonHash = daemonInstanceHash.exchange (
        currentDaemonHash, std::memory_order_relaxed);
    if (previousDaemonHash != 0 && previousDaemonHash != currentDaemonHash)
    {
        daemonRestartsObserved.fetch_add (1, std::memory_order_relaxed);
        highestAcceptedSequence.store (0, std::memory_order_relaxed);
        highestContiguousSequence.store (0, std::memory_order_relaxed);
    }

    const auto highestAccepted = static_cast<std::uint64_t> (
        static_cast<juce::int64> (object->getProperty ("highest_accepted_sequence")));
    const auto highestContiguous = static_cast<std::uint64_t> (
        static_cast<juce::int64> (object->getProperty ("highest_contiguous_sequence")));
    highestAcceptedSequence.store (
        std::max (highestAcceptedSequence.load (std::memory_order_relaxed), highestAccepted),
        std::memory_order_relaxed);
    highestContiguousSequence.store (
        std::max (highestContiguousSequence.load (std::memory_order_relaxed), highestContiguous),
        std::memory_order_relaxed);
    streamGaps.store (static_cast<std::uint64_t> (
        static_cast<juce::int64> (object->getProperty ("stream_gaps"))),
        std::memory_order_relaxed);
    streamRejections.store (static_cast<std::uint64_t> (
        static_cast<juce::int64> (object->getProperty ("stream_rejections"))),
        std::memory_order_relaxed);
    streamChainBreaks.store (static_cast<std::uint64_t> (
        static_cast<juce::int64> (object->getProperty ("stream_chain_breaks"))),
        std::memory_order_relaxed);

    const auto accepted = static_cast<bool> (object->getProperty ("accepted"));
    lastReceiptAccepted.store (accepted, std::memory_order_relaxed);
    lastReceiptRejected.store (! accepted, std::memory_order_relaxed);
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
