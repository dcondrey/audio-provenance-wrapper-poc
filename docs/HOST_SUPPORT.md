# Host application support

"Supported" is four separate claims. Each column below is stated on its own and
none of them means the plug-in has been run in that application: no listed host
is installed on the maintainer's machine, so **no row has a real-host run**.

| Column | Meaning | Where it comes from |
| --- | --- | --- |
| Capture | The host loads a plug-in format this repository builds | `docs/HOST_SUPPORT_RESEARCH.md` (vendor pages; **S** = search summary, not a page read) |
| Identified | `host_environment` names the host | JUCE `PluginHostType` (**J**, directly observed), or `data/host_executables.json` (**T**, exact executable name, `inferred`), or none |
| Project | A parser feeds `session_facts` | `docs/PROJECT_FORMATS.md` |
| Export-only | No usable capture path: only the sample watcher on exported audio, at weaker proof | this file |

Evidence for Capture, strongest available here: `auval -v aumf ApCa ApPr` passes
for the AU. pluginval and clap-validator have not been run. Everything else is
"the vendor documents that it loads this format".

| Application | Capture formats we build that it loads | Identified | Project | Notes |
| --- | --- | --- | --- | --- |
| Apple Logic Pro | AU (`auval` passed) | J | no (`.logicx` proprietary) | |
| GarageBand | AU (`auval` passed) | J | no (`.band`) | |
| Avid Pro Tools | none: loads AAX only | J | no (`.ptx`) | **Blocked**: an AAX build needs Avid's SDK and PACE signing (`-DAPW_AAX_SDK_PATH`, unsigned build only). Export-only until then |
| Ableton Live | VST3, AU | J | `.als` | |
| Steinberg Cubase | VST3 | J | via `.dawproject` export; `.cpr` no | |
| FL Studio | VST3, CLAP, AU (macOS) | J | no (`.flp`) | |
| PreSonus Studio One / Fender Studio Pro | VST3, AU (S) | J | via `.dawproject` export; `.song` no | renamed to Fender Studio Pro |
| REAPER | VST3, AU, CLAP, LV2 | J | `.rpp` | |
| Audacity | 4.x: VST3, LV2, AU; 3.x adds LADSPA | T | no (`.aup3` schema unpublished) | |
| Ardour | VST3, AU, LV2, LADSPA | J | `.ardour` | |
| LMMS | LV2, LADSPA | T | `.mmp`, `.mmpz` | VST2 needs the legacy SDK |
| Zrythm | VST3, AU, CLAP, LV2 | T | no (format unconfirmed) | |
| Tracktion Waveform (Free) | VST3, AU | J | no | |
| Bitwig Studio | VST3, CLAP | J | `.dawproject` | `.bwproject` proprietary |
| Reason | VST3 | J | no (`.reason`) | |
| Renoise | VST3, AU, LADSPA, DSSI (per OS, S) | J | no (`.xrns` unconfirmed) | |
| Adobe Audition | VST3, AU (S) | J | no (`.sesx`) | |
| Adobe Premiere Pro | VST3, AU (S) | J | no (`.prproj`) | |
| DaVinci Resolve | VST3, AU macOS (S) | J | no (`.drp`) | |
| Sony/MAGIX ACID Pro | VST3, Windows only (S) | none | no (`.acd`) | |
| Sony/MAGIX Vegas Pro | VST3, Windows only (S) | none | no (`.veg`) | |
| Steinberg Dorico | VST3 (S) | none | no (`.dorico`) | |
| Avid Sibelius | VST3, AU (S) | none | no (`.sib`) | |
| Cycling '74 Max | VST3, AU inside patchers (S) | none | `.maxpat` (constructed fixtures only) | |
| AudioMulch | AU (macOS); VST3 not found | none | no (`.amh` undocumented) | old, last release 2.2.5 |
| MakeMusic Finale | unconfirmed | none | no | discontinued 2024-08-26; successor Dorico |
| MilkyTracker | none (no plug-in support) | T | no | Export-only; `.xm`/`.mod` not yet parsed |
| Pure Data | none in vanilla | T | `.pd` | Export-only for audio |
| VCV Rack | none (paid Host module loads VST3) | T | no (`.vcv` needs zstd) | Export-only |
| Sonic Pi | none | none (not a plug-in host) | no | Export-only |
| Native Instruments Reaktor | not a host: it is a plug-in | n/a | no (`.ens`) | Nothing to capture inside it |

Unidentified hosts (`none`) are still recorded with the wrapper format and the
host executable name; they just carry no host name. Their executable names could
not be confirmed from a primary source and are deliberately not guessed.

Formats that are built but opt-in, or not buildable, are in `docs/PLATFORMS.md`.
