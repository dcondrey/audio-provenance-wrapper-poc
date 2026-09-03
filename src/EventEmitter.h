#pragma once

#include "AcknowledgementLogic.h"

#include <juce_core/juce_core.h>
#include <atomic>
#include <cstdint>

namespace apw
{

class EventEmitter final : private juce::Thread
{
public:
    struct AcknowledgementSnapshot
    {
        std::uint64_t acknowledgementsProcessed = 0;
        std::uint64_t highestAcceptedSequence = 0;
        std::uint64_t highestContiguousSequence = 0;
        std::uint64_t streamGaps = 0;
        std::uint64_t streamRejections = 0;
        std::uint64_t streamChainBreaks = 0;
        std::uint64_t sessionMismatchesIgnored = 0;
        std::uint64_t daemonRestartsObserved = 0;
        std::uint64_t lastAcknowledgementMilliseconds = 0;
        bool lastReceiptAccepted = false;
        bool lastReceiptRejected = false;
        bool stale = false;
    };

    explicit EventEmitter (const juce::String& pluginInstanceId,
                           const juce::String& pluginCaptureSessionId,
                           const juce::String& host = "127.0.0.1",
                           int port = 9876);
    ~EventEmitter() override;

    bool sendEvent (const juce::String& jsonEvent);
    AcknowledgementSnapshot getAcknowledgementSnapshot() const noexcept;
    std::uint64_t getSendAttempts() const noexcept { return sendAttempts.load (std::memory_order_relaxed); }
    std::uint64_t getSendFailures() const noexcept { return sendFailures.load (std::memory_order_relaxed); }
    std::uint64_t getSendAccepted() const noexcept { return sendAccepted.load (std::memory_order_relaxed); }
    std::uint64_t getMalformedAcknowledgements() const noexcept
    {
        return malformedAcknowledgements.load (std::memory_order_relaxed);
    }

private:
    void run() override;
    void processAcknowledgement (const juce::String& jsonAcknowledgement);

    juce::DatagramSocket socket;
    const juce::String expectedPluginInstanceId;
    const juce::String expectedPluginCaptureSessionId;
    juce::String targetHost;
    int targetPort;
    std::atomic<std::uint64_t> sendAttempts { 0 };
    std::atomic<std::uint64_t> sendFailures { 0 };
    std::atomic<std::uint64_t> sendAccepted { 0 };
    std::atomic<std::uint64_t> acknowledgementsProcessed { 0 };
    std::atomic<std::uint64_t> highestAcceptedSequence { 0 };
    std::atomic<std::uint64_t> highestContiguousSequence { 0 };
    std::atomic<std::uint64_t> streamGaps { 0 };
    std::atomic<std::uint64_t> streamRejections { 0 };
    std::atomic<std::uint64_t> streamChainBreaks { 0 };
    std::atomic<std::uint64_t> sessionMismatchesIgnored { 0 };
    std::atomic<std::uint64_t> malformedAcknowledgements { 0 };
    std::atomic<std::uint64_t> daemonRestartsObserved { 0 };
    std::atomic<std::uint64_t> lastAcknowledgementMilliseconds { 0 };
    ack::State acknowledgementState;
    std::atomic<bool> lastReceiptAccepted { false };
    std::atomic<bool> lastReceiptRejected { false };

    static constexpr std::uint64_t acknowledgementStaleMilliseconds = 3000;
    static constexpr int maxEventBytes = 8192;
    JUCE_DECLARE_NON_COPYABLE_WITH_LEAK_DETECTOR (EventEmitter)
};

} // namespace apw
