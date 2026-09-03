#pragma once

#include <cstdint>
#include <string>

namespace apw::ack
{

constexpr std::uint64_t maxCounter = (1ull << 53);

struct Fields
{
    bool envelopeValid = false;
    std::string pluginInstanceId;
    std::string pluginCaptureSessionId;
    std::string daemonInstanceId;
    bool acceptedFieldValid = false;
    bool accepted = false;
    bool countersValid = false;
    std::uint64_t highestAcceptedSequence = 0;
    std::uint64_t highestContiguousSequence = 0;
    std::uint64_t streamGaps = 0;
    std::uint64_t streamRejections = 0;
    std::uint64_t streamChainBreaks = 0;
};

struct State
{
    std::string daemonInstanceId;
    std::uint64_t highestAcceptedSequence = 0;
    std::uint64_t highestContiguousSequence = 0;
    std::uint64_t streamGaps = 0;
    std::uint64_t streamRejections = 0;
    std::uint64_t streamChainBreaks = 0;
    bool lastReceiptAccepted = false;
};

enum class Decision
{
    accepted,
    scopeMismatch,
    malformed
};

struct Update
{
    Decision decision = Decision::malformed;
    State state;
    bool daemonRestarted = false;
};

/// Pure acknowledgement transition. It performs no I/O, allocation, clock read, logging or shared
/// state mutation; callers may apply an accepted result to atomics after this returns.
Update apply (const Fields& fields,
              const std::string& expectedPluginInstanceId,
              const std::string& expectedPluginCaptureSessionId,
              const State& previous);

} // namespace apw::ack
