#include "LadspaCallbacks.h"

#if defined (_WIN32)
 #define APW_EXPORT __declspec (dllexport)
#else
 #define APW_EXPORT __attribute__ ((visibility ("default")))
#endif

extern "C" APW_EXPORT const LADSPA_Descriptor* ladspa_descriptor (unsigned long index)
{
    static const LADSPA_Descriptor descriptor =
        apw::shim::makeDescriptor<apw::shim::Format::ladspa> ("apw_capture",
                                                              "Audio Provenance Capture");
    return index == 0 ? &descriptor : nullptr;
}
