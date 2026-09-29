#include "LadspaCallbacks.h"

#include "dssi.h"

#if defined (_WIN32)
 #define APW_EXPORT __declspec (dllexport)
#else
 #define APW_EXPORT __attribute__ ((visibility ("default")))
#endif

namespace
{
void runSynth (LADSPA_Handle handle, unsigned long sampleCount,
               snd_seq_event_t* events, unsigned long eventCount)
{
    apw::shim::run (apw::shim::asInstance (handle), sampleCount, events, eventCount);
}

// The host guarantees every active instance is listed; instances are
// independent here, so each is run with its own events in turn.
void runMultipleSynths (unsigned long instanceCount, LADSPA_Handle* instances,
                        unsigned long sampleCount, snd_seq_event_t** events,
                        unsigned long* eventCounts)
{
    for (unsigned long i = 0; i < instanceCount; ++i)
        apw::shim::run (apw::shim::asInstance (instances[i]), sampleCount,
                        events != nullptr ? events[i] : nullptr,
                        eventCounts != nullptr ? eventCounts[i] : 0);
}
}

extern "C" APW_EXPORT const DSSI_Descriptor* dssi_descriptor (unsigned long index)
{
    static const LADSPA_Descriptor ladspa =
        apw::shim::makeDescriptor<apw::shim::Format::dssi> ("apw_capture_dssi",
                                                            "Audio Provenance Capture");
    static const DSSI_Descriptor descriptor = [&]
    {
        DSSI_Descriptor d {};
        d.DSSI_API_Version = 1;
        d.LADSPA_Plugin = &ladspa;
        // No GUI, no programs, no MIDI controller map, no configure keys.
        d.configure = nullptr;
        d.get_program = nullptr;
        d.get_midi_controller_for_port = nullptr;
        d.select_program = nullptr;
        d.run_synth = &runSynth;
        d.run_synth_adding = nullptr;
        d.run_multiple_synths = &runMultipleSynths;
        d.run_multiple_synths_adding = nullptr;
        return d;
    }();
    return index == 0 ? &descriptor : nullptr;
}
