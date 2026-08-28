#include "EventEmitter.h"

namespace apw
{

EventEmitter::EventEmitter (const juce::String& host, int port)
    : targetHost (host),
      targetPort (port)
{
    socket.bindToPort (0);
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

} // namespace apw
