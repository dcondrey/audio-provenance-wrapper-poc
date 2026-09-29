// Shared by LadspaHostTests.cpp and DssiHostTests.cpp. Include from exactly one
// translation unit per executable: it replaces global operator new when the
// entry points are linked in-process (APW_STATIC_ENTRY).
#pragma once

#include <arpa/inet.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <limits>
#include <mutex>
#include <new>
#include <string>
#include <thread>
#include <vector>

namespace support
{
inline thread_local bool countAllocations = false;
inline thread_local std::uint64_t allocations = 0;

struct AllocationProbe
{
    AllocationProbe() { allocations = 0; countAllocations = true; }
    ~AllocationProbe() { countAllocations = false; }
    std::uint64_t count() const { return allocations; }
};

inline int failures = 0;

inline void check (bool condition, const std::string& message)
{
    if (condition)
        return;
    ++failures;
    std::cerr << "FAIL: " << message << '\n';
}

// Deterministic signal with the awkward values a transparent pass-through
// must preserve bit for bit: negative zero, denormals, NaN, infinities.
inline void fillSignal (std::vector<float>& data, int channel)
{
    for (std::size_t i = 0; i < data.size(); ++i)
    {
        const auto phase = static_cast<double> ((i * 37 + static_cast<std::size_t> (channel) * 11) % 257) / 257.0;
        data[i] = static_cast<float> (phase * 1.6 - 0.8);
    }
    if (data.size() >= 6)
    {
        data[0] = -0.0f;
        data[1] = std::numeric_limits<float>::denorm_min();
        data[2] = -std::numeric_limits<float>::denorm_min();
        data[3] = std::numeric_limits<float>::quiet_NaN();
        data[4] = std::numeric_limits<float>::infinity();
        data[5] = -std::numeric_limits<float>::infinity();
    }
}

inline bool bitEqual (const float* a, const float* b, std::size_t n)
{
    return n == 0 || std::memcmp (a, b, n * sizeof (float)) == 0;
}

// Collects datagrams sent to the daemon port (127.0.0.1:9876) on a thread.
class DaemonListener
{
public:
    static constexpr int port = 9876;

    DaemonListener()
    {
        fd = ::socket (AF_INET, SOCK_DGRAM, 0);
        int yes = 1;
        ::setsockopt (fd, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof yes);
        sockaddr_in address {};
        address.sin_family = AF_INET;
        address.sin_port = htons (port);
        address.sin_addr.s_addr = htonl (INADDR_LOOPBACK);
        bound = fd >= 0 && ::bind (fd, reinterpret_cast<sockaddr*> (&address), sizeof address) == 0;
        if (bound)
            reader = std::thread ([this] { loop(); });
    }

    ~DaemonListener()
    {
        stop = true;
        if (reader.joinable())
            reader.join();
        if (fd >= 0)
            ::close (fd);
    }

    bool isBound() const { return bound; }

    std::vector<std::string> snapshot()
    {
        std::lock_guard<std::mutex> guard (mutex);
        return datagrams;
    }

    void clear()
    {
        std::lock_guard<std::mutex> guard (mutex);
        datagrams.clear();
    }

private:
    void loop()
    {
        char buffer[65536];
        while (! stop)
        {
            pollfd p { fd, POLLIN, 0 };
            if (::poll (&p, 1, 50) <= 0)
                continue;
            const auto n = ::recv (fd, buffer, sizeof buffer, 0);
            if (n > 0)
            {
                std::lock_guard<std::mutex> guard (mutex);
                datagrams.emplace_back (buffer, static_cast<std::size_t> (n));
            }
        }
    }

    int fd = -1;
    bool bound = false;
    std::atomic<bool> stop { false };
    std::mutex mutex;
    std::vector<std::string> datagrams;
    std::thread reader;
};

// Reads the string value of "key" from a JSON object without a parser.
inline std::string stringField (const std::string& json, const std::string& key)
{
    const auto k = json.find ("\"" + key + "\"");
    if (k == std::string::npos)
        return {};
    const auto colon = json.find (':', k);
    const auto open = json.find ('"', colon);
    const auto close = json.find ('"', open + 1);
    if (colon == std::string::npos || open == std::string::npos || close == std::string::npos)
        return {};
    return json.substr (open + 1, close - open - 1);
}

inline std::vector<std::string> ofType (const std::vector<std::string>& all, const std::string& type)
{
    std::vector<std::string> out;
    for (const auto& d : all)
        if (stringField (d, "event_type") == type)
            out.push_back (d);
    return out;
}

template <typename Predicate>
bool waitFor (Predicate&& predicate, int milliseconds)
{
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds (milliseconds);
    while (std::chrono::steady_clock::now() < deadline)
    {
        if (predicate())
            return true;
        std::this_thread::sleep_for (std::chrono::milliseconds (20));
    }
    return predicate();
}
}

#ifdef APW_STATIC_ENTRY
void* operator new (std::size_t size)
{
    if (support::countAllocations)
        ++support::allocations;
    if (auto* memory = std::malloc (size))
        return memory;
    throw std::bad_alloc();
}
void* operator new[] (std::size_t size)
{
    if (support::countAllocations)
        ++support::allocations;
    if (auto* memory = std::malloc (size))
        return memory;
    throw std::bad_alloc();
}
void operator delete (void* memory) noexcept { std::free (memory); }
void operator delete[] (void* memory) noexcept { std::free (memory); }
void operator delete (void* memory, std::size_t) noexcept { std::free (memory); }
void operator delete[] (void* memory, std::size_t) noexcept { std::free (memory); }
#endif
