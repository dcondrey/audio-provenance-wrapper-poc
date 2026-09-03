#pragma once

#include <juce_core/juce_core.h>

#include <atomic>

namespace apw
{

// Consent-gated local capture of session actions. NON_GOALS forbids cloud
// synchronisation, so this writes one local JSONL file and never egresses.
class SessionActionLog final
{
public:
    explicit SessionActionLog (juce::String sessionIdentifier)
        : sessionId (std::move (sessionIdentifier))
    {
    }

    bool hasConsent() const noexcept { return consentGranted.load (std::memory_order_relaxed); }

    void setConsent (bool granted)
    {
        const auto previous = consentGranted.exchange (granted, std::memory_order_relaxed);
        if (previous == granted || granted)
            return;
        const juce::ScopedLock lock (stateLock);
        recentActions.clear();
    }

    void record (const juce::String& action, const juce::String& detail)
    {
        if (! consentGranted.load (std::memory_order_relaxed))
            return;

        auto* entry = new juce::DynamicObject();
        entry->setProperty ("event_type", "session_action");
        entry->setProperty ("apw:proof_level", "user_declared");
        entry->setProperty ("plugin_capture_session_id", sessionId);
        entry->setProperty ("recorded_at", juce::Time::getCurrentTime().toISO8601 (true));
        entry->setProperty ("action", action);
        entry->setProperty ("detail", detail);
        const auto line = juce::JSON::toString (juce::var (entry), true);

        {
            const juce::ScopedLock lock (stateLock);
            recentActions.add (juce::Time::getCurrentTime().formatted ("%H:%M:%S") + "  " + action);
            while (recentActions.size() > kRecentActionsShown)
                recentActions.remove (0);
        }
        actionsRecorded.fetch_add (1, std::memory_order_relaxed);

        // PERF: the file write stays outside stateLock; the editor timer reads
        // the recent-action summary under that lock four times a second.
        const auto file = getLogFile();
        if (! file.getParentDirectory().createDirectory().wasOk()
            || ! file.appendText (line + juce::newLine, false, false, "\n"))
            writeFailures.fetch_add (1, std::memory_order_relaxed);
    }

    juce::File getLogFile() const
    {
        return juce::File::getSpecialLocation (juce::File::userApplicationDataDirectory)
            .getChildFile ("AudioProvenanceCapture")
            .getChildFile ("session-actions-" + sessionId + ".jsonl");
    }

    std::uint64_t getActionsRecorded() const noexcept { return actionsRecorded.load (std::memory_order_relaxed); }
    std::uint64_t getWriteFailures() const noexcept { return writeFailures.load (std::memory_order_relaxed); }

    juce::String getRecentActionSummary() const
    {
        const juce::ScopedLock lock (stateLock);
        if (recentActions.isEmpty())
            return "no actions captured";
        return recentActions[recentActions.size() - 1];
    }

private:
    static constexpr int kRecentActionsShown = 8;

    const juce::String sessionId;
    std::atomic<bool> consentGranted { false };
    std::atomic<std::uint64_t> actionsRecorded { 0 };
    std::atomic<std::uint64_t> writeFailures { 0 };
    mutable juce::CriticalSection stateLock;
    juce::StringArray recentActions;

    JUCE_DECLARE_NON_COPYABLE_WITH_LEAK_DETECTOR (SessionActionLog)
};

} // namespace apw
