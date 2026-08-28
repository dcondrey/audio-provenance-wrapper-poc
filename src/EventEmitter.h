#pragma once

#include <juce_core/juce_core.h>
#include <atomic>
#include <cstdint>

namespace apw
{

class EventEmitter final
{
public:
    explicit EventEmitter (const juce::String& host = "127.0.0.1", int port = 9876);
    ~EventEmitter() = default;

    bool sendEvent (const juce::String& jsonEvent);
    std::uint64_t getSendAttempts() const noexcept { return sendAttempts.load (std::memory_order_relaxed); }
    std::uint64_t getSendFailures() const noexcept { return sendFailures.load (std::memory_order_relaxed); }
    std::uint64_t getSendAccepted() const noexcept { return sendAccepted.load (std::memory_order_relaxed); }

private:
    juce::DatagramSocket socket;
    juce::String targetHost;
    int targetPort;
    std::atomic<std::uint64_t> sendAttempts { 0 };
    std::atomic<std::uint64_t> sendFailures { 0 };
    std::atomic<std::uint64_t> sendAccepted { 0 };

    JUCE_DECLARE_NON_COPYABLE_WITH_LEAK_DETECTOR (EventEmitter)
};

} // namespace apw
