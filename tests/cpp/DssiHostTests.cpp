// Minimal DSSI host: loads the module (dlopen) or links the entry point
// in-process (APW_STATIC_ENTRY, which also counts run-thread allocations) and
// checks pass-through transparency, MIDI delivery through run_synth and
// run_multiple_synths, and the daemon events.
#include "LadspaTestSupport.h"

#include "dssi.h"

#include <dlfcn.h>

#include <algorithm>
#include <set>

using namespace support;

#ifdef APW_STATIC_ENTRY
extern "C" const DSSI_Descriptor* dssi_descriptor (unsigned long index);
#endif

namespace
{
constexpr unsigned long sampleRate = 48000;
using DescriptorFn = const DSSI_Descriptor* (*) (unsigned long);

snd_seq_event_t makeEvent (unsigned char type, unsigned int tick, int a, int b, int c = 0)
{
    snd_seq_event_t e {};
    e.type = type;
    e.time.tick = tick;
    if (type == SND_SEQ_EVENT_NOTEON || type == SND_SEQ_EVENT_NOTEOFF)
    {
        e.data.note.channel = 0;
        e.data.note.note = static_cast<unsigned char> (a);
        e.data.note.velocity = static_cast<unsigned char> (b);
    }
    else
    {
        e.data.control.channel = 0;
        e.data.control.param = static_cast<unsigned int> (a);
        e.data.control.value = b + c;
    }
    return e;
}

const unsigned long blockSizes[] = { 64, 1, 0, 333, 512, 8192, 8193, 17, 0, 4096, 20000, 129 };
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
    const DescriptorFn descriptorFn = &dssi_descriptor;
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
    const auto descriptorFn = reinterpret_cast<DescriptorFn> (::dlsym (module, "dssi_descriptor"));
    const bool countAllocs = false;
    if (descriptorFn == nullptr)
    {
        std::cerr << "no dssi_descriptor symbol\n";
        return 2;
    }
#endif

    unsigned long count = 0;
    while (descriptorFn (count) != nullptr && count < 16)
        ++count;
    check (count == 1, "expected one descriptor, saw " + std::to_string (count));
    const auto* dssi = descriptorFn (0);
    if (dssi == nullptr)
        return 1;
    const auto* d = dssi->LADSPA_Plugin;
    check (dssi->DSSI_API_Version == 1, "DSSI_API_Version must be 1");
    check (d != nullptr, "missing LADSPA_Plugin");
    check (dssi->run_synth != nullptr && dssi->run_multiple_synths != nullptr, "run entry points missing");
    if (d == nullptr)
        return 1;
    check (d->PortCount == 4 && d->instantiate && d->connect_port && d->run && d->cleanup,
           "LADSPA half incomplete");

    listener.clear();
    auto* a = d->instantiate (d, sampleRate);
    auto* b = d->instantiate (d, sampleRate);
    check (a != nullptr && b != nullptr, "instantiate failed");
    if (a == nullptr || b == nullptr)
        return 1;

    std::size_t total = 0;
    for (auto s : blockSizes)
        total += s;
    std::vector<float> in[2], outA[2], outB[2];
    for (int c = 0; c < 2; ++c)
    {
        in[c].resize (total);
        fillSignal (in[c], c);
        outA[c].assign (total, 0.5f);
        outB[c].assign (total, 0.5f);
    }
    const auto ref0 = in[0], ref1 = in[1];

    d->activate (a);
    d->activate (b);

    std::size_t offset = 0;
    std::uint64_t allocs = 0;
    bool multi = false;
    for (auto s : blockSizes)
    {
        for (auto* h : { a, b })
        {
            auto& out = (h == a) ? outA : outB;
            d->connect_port (h, 0, in[0].data() + offset);
            d->connect_port (h, 1, in[1].data() + offset);
            d->connect_port (h, 2, out[0].data() + offset);
            d->connect_port (h, 3, out[1].data() + offset);
        }
        // Sorted events; the last one is deliberately past the block end.
        snd_seq_event_t events[] = {
            makeEvent (SND_SEQ_EVENT_NOTEON, 0, 60, 100),
            makeEvent (SND_SEQ_EVENT_CONTROLLER, static_cast<unsigned int> (s / 2), 7, 90),
            makeEvent (SND_SEQ_EVENT_PITCHBEND, static_cast<unsigned int> (s > 0 ? s - 1 : 0), 0, 0, 0),
            makeEvent (SND_SEQ_EVENT_NOTEOFF, static_cast<unsigned int> (s + 50), 60, 0)
        };
        unsigned long eventCount = sizeof events / sizeof events[0];
        const auto measure = [&] (auto&& call)
        {
            if (countAllocs)
            {
                AllocationProbe probe;
                call();
                allocs += probe.count();
            }
            else
                call();
        };
        if (multi)
        {
            LADSPA_Handle handles[2] = { a, b };
            snd_seq_event_t* lists[2] = { events, events };
            unsigned long counts[2] = { eventCount, eventCount };
            measure ([&] { dssi->run_multiple_synths (2, handles, s, lists, counts); });
        }
        else
        {
            measure ([&] { dssi->run_synth (a, s, events, eventCount);
                           dssi->run_synth (b, s, events, eventCount); });
        }
        multi = ! multi;
        offset += s;
        std::this_thread::sleep_for (std::chrono::milliseconds (2));
    }
    check (allocs == 0, "run allocated " + std::to_string (allocs) + " time(s)");
    check (bitEqual (outA[0].data(), ref0.data(), total) && bitEqual (outA[1].data(), ref1.data(), total),
           "instance A output not bit exact");
    check (bitEqual (outB[0].data(), ref0.data(), total) && bitEqual (outB[1].data(), ref1.data(), total),
           "instance B output not bit exact");

    // In place, events omitted (LADSPA run) and a null event list.
    {
        std::vector<float> l = ref0, r = ref1;
        d->connect_port (a, 0, l.data());
        d->connect_port (a, 1, r.data());
        d->connect_port (a, 2, l.data());
        d->connect_port (a, 3, r.data());
        dssi->run_synth (a, 1000, nullptr, 0);
        d->run (a, 1000);
        check (bitEqual (l.data(), ref0.data(), 1000) && bitEqual (r.data(), ref1.data(), 1000),
               "in-place run not bit exact");
        dssi->run_synth (a, 0, nullptr, 0);
    }
    // More events than the shim forwards per block: excess is dropped, and the
    // run path must still not allocate.
    {
        std::vector<snd_seq_event_t> flood;
        for (unsigned int i = 0; i < 3000; ++i)
            flood.push_back (makeEvent (i % 2 == 0 ? SND_SEQ_EVENT_NOTEON : SND_SEQ_EVENT_NOTEOFF,
                                        i % 4096, 40 + static_cast<int> (i % 40), 90));
        std::vector<float> l = ref0, r = ref1;
        d->connect_port (a, 0, l.data());
        d->connect_port (a, 1, r.data());
        d->connect_port (a, 2, l.data());
        d->connect_port (a, 3, r.data());
        std::stable_sort (flood.begin(), flood.end(),
                          [] (const snd_seq_event_t& x, const snd_seq_event_t& y) { return x.time.tick < y.time.tick; });
        std::uint64_t floodAllocs = 0;
        if (countAllocs)
        {
            AllocationProbe probe;
            dssi->run_synth (a, 4096, flood.data(), flood.size());
            floodAllocs = probe.count();
        }
        else
            dssi->run_synth (a, 4096, flood.data(), flood.size());
        check (floodAllocs == 0, "event flood allocated " + std::to_string (floodAllocs) + " time(s)");
        check (bitEqual (l.data(), ref0.data(), 4096), "event flood altered audio");
    }
    d->deactivate (a);
    d->deactivate (b);

    check (waitFor ([&] { return ofType (listener.snapshot(), "buffer_hash").size() >= 2; }, 5000),
           "no buffer_hash events reached 127.0.0.1:9876");
    check (waitFor ([&] { return ! ofType (listener.snapshot(), "midi_event").empty(); }, 5000),
           "no midi_event reached 127.0.0.1:9876");

    d->cleanup (a);
    d->cleanup (b);
    waitFor ([] { return false; }, 300);

    const auto all = listener.snapshot();
    const auto hostEvents = ofType (all, "host_environment");
    check (! hostEvents.empty(), "no host_environment event");
    for (const auto& e : hostEvents)
        check (stringField (e, "wrapper_format") == "DSSI",
               "wrapper_format was '" + stringField (e, "wrapper_format") + "', expected DSSI");
    std::set<std::string> sessions;
    for (const auto& e : all)
        sessions.insert (stringField (e, "plugin_capture_session_id"));
    check (sessions.size() == 2, "expected two distinct sessions, saw " + std::to_string (sessions.size()));

    std::cout << "datagrams: " << all.size() << " (host_environment " << hostEvents.size()
              << ", buffer_hash " << ofType (all, "buffer_hash").size()
              << ", midi_event " << ofType (all, "midi_event").size() << ")\n";
    if (failures == 0)
        std::cout << "DSSI host tests passed\n";
    return failures == 0 ? 0 : 1;
}
