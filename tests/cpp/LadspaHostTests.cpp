// Minimal LADSPA host: loads the module (dlopen) or links the entry point
// in-process (APW_STATIC_ENTRY, which also counts run-thread allocations),
// walks the descriptors, and checks pass-through transparency and daemon events.
#include "LadspaTestSupport.h"

#include "ladspa.h"

#include <dlfcn.h>

#include <algorithm>
#include <set>

using namespace support;

#ifdef APW_STATIC_ENTRY
extern "C" const LADSPA_Descriptor* ladspa_descriptor (unsigned long index);
#endif

namespace
{
constexpr unsigned long sampleRate = 48000;

using DescriptorFn = const LADSPA_Descriptor* (*) (unsigned long);

struct Ports
{
    std::vector<float> in[2];
    std::vector<float> out[2];
};

void connect (const LADSPA_Descriptor* d, LADSPA_Handle h,
              float* inL, float* inR, float* outL, float* outR)
{
    d->connect_port (h, 0, inL);
    d->connect_port (h, 1, inR);
    d->connect_port (h, 2, outL);
    d->connect_port (h, 3, outR);
}

// Block sizes include zero, one, a prime, and sizes straddling the shim's
// internal 8192-frame chunk limit.
const unsigned long blockSizes[] = { 64, 1, 0, 333, 512, 8192, 8193, 17, 0, 4096, 20000, 129 };

void passThroughOutOfPlace (const LADSPA_Descriptor* d, LADSPA_Handle h, bool countAllocs)
{
    std::size_t total = 0;
    for (auto s : blockSizes)
        total += s;
    Ports p;
    for (int c = 0; c < 2; ++c)
    {
        p.in[c].resize (total);
        fillSignal (p.in[c], c);
        p.out[c].assign (total, 0.123f);
    }
    const auto reference0 = p.in[0];
    const auto reference1 = p.in[1];

    std::size_t offset = 0;
    std::uint64_t allocs = 0;
    for (auto s : blockSizes)
    {
        // Ports are re-connected each block to the moving window, as hosts do.
        connect (d, h, p.in[0].data() + offset, p.in[1].data() + offset,
                 p.out[0].data() + offset, p.out[1].data() + offset);
        if (countAllocs)
        {
            AllocationProbe probe;
            d->run (h, s);
            allocs += probe.count();
        }
        else
            d->run (h, s);
        offset += s;
        std::this_thread::sleep_for (std::chrono::milliseconds (2));
    }
    check (allocs == 0, "run allocated " + std::to_string (allocs) + " time(s)");
    check (bitEqual (p.out[0].data(), reference0.data(), total), "out-of-place L not bit exact");
    check (bitEqual (p.out[1].data(), reference1.data(), total), "out-of-place R not bit exact");
    check (bitEqual (p.in[0].data(), reference0.data(), total), "out-of-place input L modified");
    check (bitEqual (p.in[1].data(), reference1.data(), total), "out-of-place input R modified");
}

void passThroughInPlace (const LADSPA_Descriptor* d, LADSPA_Handle h)
{
    std::size_t total = 0;
    for (auto s : blockSizes)
        total += s;
    std::vector<float> l (total), r (total);
    fillSignal (l, 0);
    fillSignal (r, 1);
    const auto refL = l, refR = r;
    connect (d, h, l.data(), r.data(), l.data(), r.data());
    std::size_t offset = 0;
    for (auto s : blockSizes)
    {
        d->connect_port (h, 0, l.data() + offset);
        d->connect_port (h, 1, r.data() + offset);
        d->connect_port (h, 2, l.data() + offset);
        d->connect_port (h, 3, r.data() + offset);
        d->run (h, s);
        offset += s;
    }
    check (bitEqual (l.data(), refL.data(), offset), "in-place L not bit exact");
    check (bitEqual (r.data(), refR.data(), offset), "in-place R not bit exact");
}

// Output ports that swap the input buffers must still deliver each input to its
// own channel, not the overwritten neighbour.
void passThroughCrossAliased (const LADSPA_Descriptor* d, LADSPA_Handle h)
{
    std::vector<float> a (5000), b (5000);
    fillSignal (a, 0);
    fillSignal (b, 1);
    const auto refA = a, refB = b;
    connect (d, h, a.data(), b.data(), b.data(), a.data());
    d->run (h, static_cast<unsigned long> (a.size()));
    check (bitEqual (b.data(), refA.data(), a.size()), "cross-aliased L lost");
    check (bitEqual (a.data(), refB.data(), a.size()), "cross-aliased R lost");
}

void zeroLengthRunLeavesBuffersAlone (const LADSPA_Descriptor* d, LADSPA_Handle h)
{
    std::vector<float> in (16, 0.5f), out (16, -0.25f);
    connect (d, h, in.data(), in.data(), out.data(), out.data());
    d->run (h, 0);
    check (std::all_of (out.begin(), out.end(), [] (float v) { return v == -0.25f; }),
           "zero-length run wrote output");
}
}

int main (int argc, char** argv)
{
    DaemonListener listener;
    if (! listener.isBound())
    {
        std::cerr << "cannot bind UDP 127.0.0.1:9876 (is a daemon running?)\n";
        return 2;
    }

#ifdef APW_STATIC_ENTRY
    const DescriptorFn descriptorFn = &ladspa_descriptor;
    const bool countAllocs = true;
    (void) argc; (void) argv;
#else
    if (argc < 2)
    {
        std::cerr << "usage: " << argv[0] << " <module.so>\n";
        return 2;
    }
    void* module = ::dlopen (argv[1], RTLD_NOW | RTLD_LOCAL);
    if (module == nullptr)
    {
        std::cerr << "dlopen failed: " << ::dlerror() << '\n';
        return 2;
    }
    const auto descriptorFn = reinterpret_cast<DescriptorFn> (::dlsym (module, "ladspa_descriptor"));
    const bool countAllocs = false;
    if (descriptorFn == nullptr)
    {
        std::cerr << "no ladspa_descriptor symbol\n";
        return 2;
    }
#endif

    // Descriptor walk, as a host scanning the file does.
    unsigned long count = 0;
    while (descriptorFn (count) != nullptr && count < 16)
        ++count;
    check (count == 1, "expected one descriptor, saw " + std::to_string (count));
    const auto* d = descriptorFn (0);
    if (d == nullptr)
        return 1;

    check (d->Label != nullptr && std::strlen (d->Label) > 0
               && std::strchr (d->Label, ' ') == nullptr, "label must be non-empty without spaces");
    check (d->PortCount == 4, "expected 4 ports");
    check (d->instantiate && d->connect_port && d->run && d->cleanup, "mandatory callbacks missing");
    if (d->PortCount == 4)
    {
        check (LADSPA_IS_PORT_INPUT (d->PortDescriptors[0]) && LADSPA_IS_PORT_AUDIO (d->PortDescriptors[0]),
               "port 0 must be audio input");
        check (LADSPA_IS_PORT_INPUT (d->PortDescriptors[1]) && LADSPA_IS_PORT_AUDIO (d->PortDescriptors[1]),
               "port 1 must be audio input");
        check (LADSPA_IS_PORT_OUTPUT (d->PortDescriptors[2]) && LADSPA_IS_PORT_AUDIO (d->PortDescriptors[2]),
               "port 2 must be audio output");
        check (LADSPA_IS_PORT_OUTPUT (d->PortDescriptors[3]) && LADSPA_IS_PORT_AUDIO (d->PortDescriptors[3]),
               "port 3 must be audio output");
    }
    check (d->instantiate (d, 0) == nullptr, "zero sample rate must be rejected");

    listener.clear();
    auto* first = d->instantiate (d, sampleRate);
    auto* second = d->instantiate (d, sampleRate);
    check (first != nullptr && second != nullptr, "instantiate failed");
    if (first == nullptr || second == nullptr)
        return 1;

    // First instance runs without activate (optional in LADSPA); second follows
    // the full activate/run/deactivate/activate cycle.
    passThroughOutOfPlace (d, first, countAllocs);
    passThroughInPlace (d, first);
    passThroughCrossAliased (d, first);
    zeroLengthRunLeavesBuffersAlone (d, first);

    d->activate (second);
    passThroughOutOfPlace (d, second, countAllocs);
    d->deactivate (second);
    d->activate (second);
    passThroughInPlace (d, second);
    d->deactivate (second);

    // Events from both instances must reach the daemon port.
    const auto haveHashes = [&]
    {
        return ofType (listener.snapshot(), "buffer_hash").size() >= 2;
    };
    check (waitFor (haveHashes, 5000), "no buffer_hash events reached 127.0.0.1:9876");

    d->cleanup (first);
    d->cleanup (second);
    waitFor ([] { return false; }, 300);

    const auto all = listener.snapshot();
    const auto hostEvents = ofType (all, "host_environment");
    check (! hostEvents.empty(), "no host_environment event");
    for (const auto& e : hostEvents)
        check (stringField (e, "wrapper_format") == "LADSPA",
               "wrapper_format was '" + stringField (e, "wrapper_format") + "', expected LADSPA");

    std::set<std::string> instances, sessions;
    for (const auto& e : all)
    {
        instances.insert (stringField (e, "plugin_instance_id"));
        sessions.insert (stringField (e, "plugin_capture_session_id"));
    }
    check (instances.size() == 2 && sessions.size() == 2,
           "expected two distinct instance/session ids, saw "
               + std::to_string (instances.size()) + "/" + std::to_string (sessions.size()));
    check (instances.count ("") == 0, "event without plugin_instance_id");

    std::cout << "datagrams: " << all.size() << " (host_environment " << hostEvents.size()
              << ", buffer_hash " << ofType (all, "buffer_hash").size() << ")\n";
    if (failures == 0)
        std::cout << "LADSPA host tests passed\n";
    return failures == 0 ? 0 : 1;
}
