#pragma once

#include <juce_core/juce_core.h>

#include <cstddef>

namespace apw::safejson
{

// IMPORTANT: juce::JSON::parse is unbounded recursive descent and the threads
// that call it get the 512 KB macOS default stack, so a nested payload of a few
// thousand frames faults the host process. Every parse of bytes this plug-in did
// not produce itself MUST go through parseBounded/parseBoundedFile.
inline constexpr int kMaxNestingDepth = 32;

inline bool hasBoundedNesting (const char* data, size_t numBytes, int maxDepth) noexcept
{
    int depth = 0;
    bool inString = false;
    bool escaped = false;

    for (size_t index = 0; index < numBytes; ++index)
    {
        const auto character = data[index];

        if (inString)
        {
            if (escaped)                escaped = false;
            else if (character == '\\') escaped = true;
            else if (character == '"')  inString = false;
            continue;
        }

        if (character == '"')
        {
            inString = true;
        }
        else if (character == '[' || character == '{')
        {
            if (++depth > maxDepth)
                return false;
        }
        else if (character == ']' || character == '}')
        {
            if (--depth < 0)
                return false;
        }
    }

    return depth == 0 && ! inString;
}

inline juce::var parseBounded (const juce::String& text, int maxDepth = kMaxNestingDepth)
{
    const auto numBytes = static_cast<size_t> (text.getNumBytesAsUTF8());
    if (numBytes == 0 || ! hasBoundedNesting (text.toRawUTF8(), numBytes, maxDepth))
        return {};

    juce::var parsed;
    if (juce::JSON::parse (text, parsed).failed())
        return {};
    return parsed;
}

inline juce::var parseBoundedFile (const juce::File& file, juce::int64 maxBytes,
                                   int maxDepth = kMaxNestingDepth)
{
    const auto size = file.getSize();
    if (size <= 0 || size > maxBytes)
        return {};

    juce::MemoryBlock bytes;
    if (! file.loadFileAsData (bytes) || bytes.getSize() == 0)
        return {};

    return parseBounded (juce::String::fromUTF8 (static_cast<const char*> (bytes.getData()),
                                                 static_cast<int> (bytes.getSize())),
                         maxDepth);
}

} // namespace apw::safejson
