#include "AcknowledgementLogic.h"

#include <algorithm>

namespace apw::ack
{

Update apply (const Fields& fields,
              const std::string& expectedPluginInstanceId,
              const std::string& expectedPluginCaptureSessionId,
              const State& previous)
{
    Update update;
    update.state = previous;
    if (! fields.envelopeValid)
        return update;

    if (fields.pluginInstanceId != expectedPluginInstanceId
        || fields.pluginCaptureSessionId != expectedPluginCaptureSessionId)
    {
        update.decision = Decision::scopeMismatch;
        return update;
    }

    const auto countersWithinBounds = fields.highestAcceptedSequence <= maxCounter
        && fields.highestContiguousSequence <= maxCounter
        && fields.streamGaps <= maxCounter
        && fields.streamRejections <= maxCounter
        && fields.streamChainBreaks <= maxCounter;
    if (! fields.acceptedFieldValid || ! fields.countersValid
        || fields.daemonInstanceId.empty() || fields.daemonInstanceId.size() > 128
        || ! countersWithinBounds
        || fields.highestContiguousSequence > fields.highestAcceptedSequence)
    {
        return update;
    }

    update.daemonRestarted = ! previous.daemonInstanceId.empty()
        && previous.daemonInstanceId != fields.daemonInstanceId;
    if (update.daemonRestarted)
    {
        update.state.highestAcceptedSequence = 0;
        update.state.highestContiguousSequence = 0;
    }
    update.state.daemonInstanceId = fields.daemonInstanceId;
    update.state.highestAcceptedSequence = std::max (
        update.state.highestAcceptedSequence, fields.highestAcceptedSequence);
    update.state.highestContiguousSequence = std::max (
        update.state.highestContiguousSequence, fields.highestContiguousSequence);
    update.state.streamGaps = fields.streamGaps;
    update.state.streamRejections = fields.streamRejections;
    update.state.streamChainBreaks = fields.streamChainBreaks;
    update.state.lastReceiptAccepted = fields.accepted;
    update.decision = Decision::accepted;
    return update;
}

} // namespace apw::ack
