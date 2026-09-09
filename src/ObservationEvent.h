#pragma once

#include <juce_core/juce_core.h>
#include <cstdint>
#include <utility>

namespace apw
{

namespace EventTypes
{
    inline constexpr const char* bufferHash      = "buffer_hash";
    inline constexpr const char* audioTransition = "audio_transition";
    inline constexpr const char* spectralShift   = "spectral_shift";
    inline constexpr const char* transportChange = "transport_change";
    inline constexpr const char* midiEvent       = "midi_event";
    inline constexpr const char* sessionConfig   = "session_config_change";
    inline constexpr const char* hostEnvironment = "host_environment";
}

namespace ProofLevels
{
    inline constexpr const char* directlyObserved = "directly_observed";
    inline constexpr const char* inferred         = "inferred";
    inline constexpr const char* userDeclared     = "user_declared";
    inline constexpr const char* unknown          = "unknown_unobserved";
}

inline juce::String buildJsonEvent (const char* eventType,
                                     std::uint64_t timestampMs,
                                     juce::int64 samplePosition,
                                     std::initializer_list<std::pair<juce::String, juce::var>> fields)
{
    auto* obj = new juce::DynamicObject();
    obj->setProperty ("event_type", juce::String (eventType));
    obj->setProperty ("proof_level", juce::String (ProofLevels::directlyObserved));
    obj->setProperty ("timestamp_ms", juce::var (static_cast<juce::int64> (timestampMs)));
    obj->setProperty ("sample_position", juce::var (samplePosition));

    for (const auto& field : fields)
        obj->setProperty (field.first, field.second);

    return juce::JSON::toString (juce::var (obj), true);
}

} // namespace apw
