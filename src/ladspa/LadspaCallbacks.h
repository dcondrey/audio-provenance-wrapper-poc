#pragma once

#include "CaptureInstance.h"
#include "ladspa.h"

// LADSPA entry points shared by the LADSPA and DSSI shims. The two formats
// differ only in the wrapper_format reported to the daemon.
namespace apw::shim
{
inline const LADSPA_PortDescriptor portDescriptors[portCount] = {
    LADSPA_PORT_INPUT | LADSPA_PORT_AUDIO,
    LADSPA_PORT_INPUT | LADSPA_PORT_AUDIO,
    LADSPA_PORT_OUTPUT | LADSPA_PORT_AUDIO,
    LADSPA_PORT_OUTPUT | LADSPA_PORT_AUDIO
};

inline const char* const portNames[portCount] = {
    "Input L", "Input R", "Output L", "Output R"
};

inline const LADSPA_PortRangeHint portRangeHints[portCount] = {
    { 0, 0.0f, 0.0f }, { 0, 0.0f, 0.0f }, { 0, 0.0f, 0.0f }, { 0, 0.0f, 0.0f }
};

template <Format format>
LADSPA_Handle instantiate (const LADSPA_Descriptor*, unsigned long sampleRate)
{
    return create (format, sampleRate);
}

inline CaptureInstance& asInstance (LADSPA_Handle handle) noexcept
{
    return *static_cast<CaptureInstance*> (handle);
}

inline void connectPort (LADSPA_Handle handle, unsigned long port, LADSPA_Data* data)
{
    apw::shim::connectPort (asInstance (handle), port, data);
}

inline void activate (LADSPA_Handle handle) { apw::shim::activate (asInstance (handle)); }
inline void deactivate (LADSPA_Handle handle) { apw::shim::deactivate (asInstance (handle)); }
inline void cleanup (LADSPA_Handle handle) { destroy (&asInstance (handle)); }

inline void run (LADSPA_Handle handle, unsigned long sampleCount)
{
    apw::shim::run (asInstance (handle), sampleCount, nullptr, 0);
}

// UniqueID sits in the range LADSPA reserves for development and testing:
// this repository has no registered ID.
inline constexpr unsigned long developmentUniqueId = 0x415043; // "APC"

template <Format format>
LADSPA_Descriptor makeDescriptor (const char* label, const char* name)
{
    LADSPA_Descriptor descriptor {};
    descriptor.UniqueID = developmentUniqueId + (format == Format::dssi ? 1 : 0);
    descriptor.Label = label;
    descriptor.Properties = 0; // not claimed HARD_RT_CAPABLE: see docs/LADSPA_DSSI.md
    descriptor.Name = name;
    descriptor.Maker = "Audio Provenance POC";
    descriptor.Copyright = "All rights reserved";
    descriptor.PortCount = portCount;
    descriptor.PortDescriptors = portDescriptors;
    descriptor.PortNames = portNames;
    descriptor.PortRangeHints = portRangeHints;
    descriptor.ImplementationData = nullptr;
    descriptor.instantiate = &instantiate<format>;
    descriptor.connect_port = &connectPort;
    descriptor.activate = &activate;
    descriptor.run = &run;
    descriptor.run_adding = nullptr;
    descriptor.set_run_adding_gain = nullptr;
    descriptor.deactivate = &deactivate;
    descriptor.cleanup = &cleanup;
    return descriptor;
}
}
