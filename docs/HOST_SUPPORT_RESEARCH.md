# Host application research (plug-in formats, status, project format)

Retrieved 2026-09-29. Evidence tags on each claim:

- **P** = page fetched directly from the primary source in this session.
- **S** = stated by a search-result summary of the cited primary URL; the page itself was not read. Re-verify before relying on it.
- **U** = unconfirmed. No primary source found, or the primary page was unreachable (403, 404, DNS, refused connection).

Item (4), default exported-audio behavior, is skipped for every application. No primary quote was retrieved.

"Open format" means the vendor or project publishes the spec or the parsing source. Community reverse-engineering does not count.

---

## DAWs

### Apple Logic Pro
1. Third-party formats (macOS only): Audio Units (AUv2) and Audio Unit Extensions (AUv3). No VST/VST3/CLAP/LV2. **P**: https://support.apple.com/guide/logicpro/work-with-audio-units-in-logic-pro-for-mac-lgcp22a0dab0/mac ("you can use Audio Units plug-ins and Audio Unit Extensions in your projects"). The absence of VST is inferred from the page not listing it; the Apple pages read did not state a negative.
2. Status: current. Version number U (the Apple guide index does not state it in what was read).
3. Project: `.logicx` package. Proprietary, no public spec. U (no primary source found).

### Avid Pro Tools
1. Third-party format: AAX only (Native and DSP). VST/AU are not loaded. AAX is the format named in Avid's developer program; the direct Avid pages returned 403 (https://www.avid.com/pro-tools). Only blog and forum sources were found for the "no VST/AU" statement, so U for the negative. Wrapper products exist (S: https://www.production-expert.com/production-expert-1/7-ways-to-be-able-to-use-vst-or-au-plug-ins-in-pro-tools, non-primary).
2. Status: current. Version U.
3. Project: `.ptx` (older `.ptf`, `.pts`). Proprietary binary, no public spec. U. Interchange is via AAF/OMF (not fetched).

### Ableton Live
1. Windows and macOS: VST2, VST3. macOS: AU (v2), and AUv3 from 11.2 per the manual text. No CLAP: the manual does not mention it and the Live 12 release notes contain no CLAP entry. **P**: https://www.ableton.com/en/manual/using-plug-ins/ and https://www.ableton.com/en/release-notes/live-12/. No LV2/LADSPA/DSSI.
2. Status: current. Live 12.4.6, 2026-09-15. **P**: https://www.ableton.com/en/release-notes/live-12/
3. Project: `.als`. Community-observed gzip-compressed XML. No vendor spec, so this is not a documented open format. Non-primary sources: https://github.com/Qpai/ableton-als-file-format, https://news.ycombinator.com/item?id=30485195. U for vendor documentation.

### Steinberg Cubase
1. VST2 and VST3 (64-bit). No AU. Cubase 15 is the current version. **S**: https://www.steinberg.help/r/cubase-pro/15.0/en/cubase_nuendo/topics/installing_and_managing_plugins/installing_and_managing_plugins_plugin_manager_installing_vst_plugins_c.html (the page fetch returned a TOC only). Product page: https://www.steinberg.net/cubase/. A summary of that page listed "AU", which contradicts other sources and is treated as unreliable. AU is U-negative (not documented as supported).
2. Status: current, Cubase 15 (Pro/Artist/Elements). **P**: https://www.steinberg.net/cubase/
3. Project: `.cpr`. Proprietary binary, no public spec. U. Cubase 14+ can export/import the open `.dawproject` (see DAWproject below).

### Image-Line FL Studio
1. Windows: VST2, VST3, CLAP. macOS: VST2, VST3, AU, CLAP. CLAP since FL Studio 2024.1. **P**: https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/basics_externalplugins.htm and https://www.image-line.com/fl-studio
2. Status: current, "FL Studio 2026" per the product page. **P**: https://www.image-line.com/fl-studio
3. Project: `.flp`. Proprietary binary, no vendor spec. U.

### PreSonus Studio One, now Fender Studio Pro
1. Third-party formats: VST2, VST3, AU (macOS) per a retailer/support summary (**S**: https://www.waves.com/support/how-to-find-your-plugins-in-fender-studio-pro, non-vendor). CLAP: U. The vendor plug-ins page (https://www.fender.com/pages/fender-studio-pro-plug-ins) returned no format list.
2. Status: renamed. The presonus.com Studio One Pro URL 301-redirects to https://www.fender.com/pages/fender-studio-pro (**P**, redirect observed). Rename date 2026-01-13 (**S**: https://www.soundonsound.com/news/studio-one-becomes-fender-studio-pro, press). Current product is Fender Studio Pro 8 (S: https://www.soundonsound.com/reviews/fender-studio-pro-8).
3. Project: `.song`. Community-observed zip of XML; no vendor spec, so not an open format by the definition above. U. Studio One 6.5 supports the open `.dawproject` (**P**: https://github.com/bitwig/dawproject).

### REAPER
1. VST, VST3, LV2, AU, CLAP, DX, JS (JSFX). **P**: https://www.reaper.fm/ ("including VST, VST3, LV2, AU, CLAP, DX, and JS"). OS split per format: U (AU is macOS-only and DX Windows-only by nature; not stated on the page). No LADSPA/DSSI listed.
2. Status: current. v7.81, 2026-09-28. **P**: https://www.reaper.fm/whatsnew.txt
3. Project: `.rpp`. Line-based plain text; Cockos publishes the parser in WDL. **P**: https://raw.githubusercontent.com/justinfrankel/WDL/main/WDL/projectcontext.h (`ProjectStateContext` line reader/writer). No standalone spec page found; the source is the reference.

### Audacity
1. Version-dependent, and the sources conflict:
   - Audacity 4.0.0 release notes: "Supported plugin formats are VST3, Nyquist, LV2 on Linux and Audio Units on macOS"; VAMP and LADSPA not available in 4.0; VST2 dropped. **P** (summarized fetch of the release page): https://github.com/audacity/audacity/releases
   - Audacity 3.x per the support repo: Windows VST2/VST3/LV2/LADSPA/Vamp/Nyquist; macOS AU/VST2/VST3/LV2/Vamp/Nyquist; Linux LV2/VST2/VST3/LADSPA/Vamp/Nyquist. **P**: https://raw.githubusercontent.com/audacity/audacity-support/main/basics/customizing-audacity/installing-plugins.md. This page likely describes 3.x.
   - Use the 4.0 row for 4.x and the 3.x row for 3.x. Do not merge them.
2. Status: current. 4.0.0 is the latest release on GitHub (release year not confirmed by my fetch). Distributed via MuseHub per https://www.audacityteam.org/. Project is GPL. **P**.
3. Project: `.aup3`, an SQLite3 database. **P**: https://github.com/audacity/audacity-project-tools ("Internally, the AUP3 file is an SQLite3 database"). The vendor manual documents the extension (https://manual.audacityteam.org/man/audacity_projects.html) but not the schema. Legacy 2.x `.aup` plus `_data` folder.

### Ardour
1. LADSPA, LV2, AU (macOS), VST2, VST3, plus Lua. No CLAP found. **P**: https://ardour.org/features.html ("native VST2, VST3, AU, LADSPA, and LV2 plugins") and https://manual.ardour.org/working-with-plugins/. OS split beyond AU=macOS: U.
2. Status: current, Ardour 9 (version string not confirmed). **P**: https://ardour.org/features.html
3. Project: `.ardour`. XML, written through XMLTree. **P**: https://github.com/Ardour/ardour/blob/master/libs/ardour/session_state.cc (`Session::save_state()`, `statefile_suffix`). Open source is the spec.

### LMMS
1. README: SoundFont2, VST2 (instruments and effects), LADSPA, LV2. **P**: https://github.com/LMMS/lmms. VST3: not in README (U). CLAP: a PR is open, not merged (S: https://github.com/LMMS/lmms/pull/7199). LinuxVST natively in 1.3.0-alpha.2 (P: https://github.com/LMMS/lmms/releases).
2. Status: current, in development. 1.3.0-alpha.2 is the latest prerelease. The release page fetch showed a September 6 date; the year read as 2024 in that fetch and 2026 in search results, so the year is U. **P** for existence: https://github.com/LMMS/lmms/releases
3. Project: `.mmp` (plain XML) and `.mmpz` (compressed `.mmp`). The README lists both extensions; the XML/compression detail is not stated on the fetched page. Format is defined by source (`DataFile`, not fetched). Format claim is U at primary level; from memory only.

### Zrythm
1. Homepage: "every major plugin format including LV2, VST2, VST3, AU, CLAP and JSFX ... sandboxing thanks to Carla" (the 1.x-era page). **P**: https://www.zrythm.org/en/index.html. Zrythm 2.0 is a Qt/QML + JUCE rewrite with native CLAP (S: https://www.phoronix.com/news/Zrythm-2.0-Alpha, https://bedroomproducersblog.com/2026/08/06/zrythm-2-alpha/).
2. Status: current. 2.0 alpha; the alpha date is contradicted (May 2026 in one source, August 2026 in another), so the date is U.
3. Project: U. The homepage does not name a format, and 2.0 changed the architecture. The GitLab source is the place to check: https://gitlab.zrythm.org/zrythm/zrythm.

### Tracktion Waveform (Free)
1. VST, VST3, AU. **P**: https://www.tracktion.com/products/waveform-free ("support for VST, VST3 and AU"). No CLAP/LV2 stated (U).
2. Status: current, Waveform 14; macOS 13+, Windows 10/11, Ubuntu 24.04. **P**: same URL.
3. Project: `.tracktionedit` per the engine (JUCE ValueTree serialized as XML). U at primary level. The engine repo (https://github.com/Tracktion/tracktion_engine) is open (GPL/commercial), but the README fetched did not state the edit-file format.

### Bitwig Studio
1. VST2, VST3, CLAP. No AU or LV2. **S**: https://www.bitwig.com/learnings/plug-in-hosting-crash-protection-in-bitwig-studio-20/ and https://www.bitwig.com/userguide/latest/vst_plug-ins/. The user guide page fetched only names "VST or CLAP plug-ins". VST2 vs VST3 versions and AU absence: U at primary level.
2. Status: current. Bitwig Studio 6.1 per the homepage. **P**: https://www.bitwig.com/
3. Project: `.bwproject`. Proprietary. Bitwig authored the open DAWproject exchange format (`.dawproject`, zip of `project.xml` + `metadata.xml`). **P**: https://github.com/bitwig/dawproject (v1.0 supported by Bitwig 5.0.9, Studio One 6.5, Cubase 14, Cubasis 3.7.1, VST Live 2.2, n-Track 10.2.2).

### Reason (Studios)
1. VST3 and VST2 (2.4), 64-bit, Windows and macOS, since Reason 12.5. Audio Units not mentioned. Rack Extensions is a Reason-specific format (other). **P**: https://docs.reasonstudios.com/reason13/working-with-vst-plugins
2. Status: current, Reason 13. **S** (search results); the product page fetch did not state a version.
3. Project: `.reason`. Proprietary, no spec. U.

### Renoise
1. Windows: VST (incl. VST3 in separate category). macOS: VST, AU. Linux: VST, LADSPA, DSSI. The manual's plugin page confirms "VST, AU, DSSI" (**P**: https://tutorials.renoise.com/wiki/Plugin). The full per-OS split comes from a search summary of https://www.renoise.com/renoise (**S**). LADSPA appears in the Linux notes (S: https://tutorials.renoise.com/wiki/Linux_FAQ).
2. Status: current. Version U.
3. Project: `.xrns`. Zip archive containing `Song.xml` plus FLAC samples. The XML schema `RenoiseSong.xsd` ships with the demo. Format is documented by the vendor, but the vendor wiki page "Song_Format" was empty. Archive description: http://justsolve.archiveteam.org/wiki/Renoise_song (non-primary). Treat "open" as U at primary level.

### Adobe Audition
1. Windows and macOS: VST 2.4 and VST3 (64-bit). macOS: Audio Units. VST instruments not supported. **S**: https://helpx.adobe.com/audition/using/adding-third-party-plug-ins.html (direct fetch 403; jina 422).
2. Status: current. Version U.
3. Project (session): `.sesx`, XML. Adobe does not publish a schema (U).

### Adobe Premiere Pro
1. Audio effect plug-ins: VST, VST3, and AU on macOS only. **S**: https://helpx.adobe.com/premiere-pro/desktop/use-premiere-pro-with-other-apps/plug-ins.html (direct fetch 403, jina 404).
2. Status: current. Version U.
3. Project: `.prproj`, gzip-compressed XML in practice. Not documented by Adobe. U.

### MakeMusic Finale
1. Plug-in hosting: U (no primary source retrieved).
2. Status: discontinued. Development and sales ended 2024-08-26; technical support ended after August 2025; successor offered is Steinberg Dorico. **S**: https://www.makemusic.com/press-room/press-releases-2024/makemusic-sunsets-finale/ and https://makemusic.zendesk.com/hc/en-us/articles/25843888130839-Finale-Sunset-FAQ (search results only; pages not fetched).
3. Project: `.musx` (zip of XML, Finale 2014+) and older binary `.mus`. Not a published spec. The MusicXML 4.0 export is the interchange route (S: https://makemusic.zendesk.com/hc/en-us/articles/25868952458519-Exporting-MusicXML-Files-from-Finale). `.musx` structure is from non-primary pages; U.

### Sony/MAGIX ACID Pro
1. Windows only: VST, VST3, DirectX audio (ACID Pro 11: 64-bit). Forum reports of unstable VST3. **S**: https://www.magix.info/us/forum/vst-versus-vst3-in-acid-pro-11--1305516/ (a user forum, not a vendor spec). Vendor spec page: U.
2. Status: owned and sold by MAGIX (originally Sony Creative Software, sold to MAGIX in 2016 per S: https://en.wikipedia.org/wiki/Acid_Pro, non-primary). Current version: U.
3. Project: `.acd` (and `.acd-zip`). Proprietary. U.

### Sony/MAGIX Vegas Pro (VEGAS Pro)
1. Windows only: VST2, VST3, and DirectX. VEGAS Pro 20+ loads VST3 (**S**: https://creativecow.net/forums/thread/which-vegas-pro-versions-can-utilize-vst3-audio-plugins/, forum; vendor page: U).
2. Status: current, VEGAS Pro 23 (S). Ownership: MAGIX (Sony sold Creative Software in 2016; non-primary).
3. Project: `.veg`. Proprietary. U.

### GarageBand
1. macOS: Audio Units (third-party AU installed in the system plug-in folders). iOS/iPadOS: Audio Unit Extensions. No VST. **S**: https://support.apple.com/en-sa/102239 (Apple support; page not fetched).
2. Status: current (Apple, free). Version U.
3. Project: `.band` package. Proprietary, no public spec. U.

### DaVinci Resolve (Fairlight)
1. Windows and macOS: VST3 (since 17.4). macOS: Audio Units. Not on Linux. **S**: https://forum.blackmagicdesign.com/viewtopic.php?f=33&t=148213 (vendor forum thread), https://www.steakunderwater.com/VFXPedia/__man/Resolve18-6/DaVinciResolve18_Manual_files/part3757.htm (mirror of the manual; returned 404 on direct fetch). Vendor manual: U.
2. Status: current; the vendor blog (S) referred to "DaVinci Resolve 21". Version U.
3. Project: `.drp` export/import; database-based projects otherwise. Proprietary. U.

### Sibelius (Avid)
1. Hosts VST, Audio Units, AAX. VST3 added in Sibelius 2026.5 alongside VST2. **S**: https://www.production-expert.com/production-expert-1/sibelius-20265-adds-vst3-support-and-cloud-part-sharing (press). Avid pages returned 403. Vendor confirmation: U.
2. Status: current. Sibelius 2026.5.
3. Project: `.sib`. Proprietary. U.

### Steinberg Dorico
1. VST3 (all). VST2 only via a vendor whitelist. AU: U (not found in what was retrieved). **S**: https://helpcenter.steinberg.de/hc/en-us/articles/206899770-How-to-use-VST2-plug-ins-in-Dorico (403 on direct fetch). Manual: https://www.steinberg.help/v/u/dorico_pro_6_1_operation_manual_en.pdf (not read).
2. Status: current, Dorico Pro 6.x. Named as the successor to Finale by MakeMusic (see Finale).
3. Project: `.dorico`, zip container. No public spec for the internal XML (community discussion: https://forums.steinberg.net/t/dorico-file-format-commitment-to-an-unencrypted-file-format/736582; non-primary). U for openness.

---

## Programming and patching environments

### Cycling '74 Max (Max/MSP/Jitter)
1. Hosts Audio Unit, VST and VST3 plug-ins via `vst~` / `mc.vst~`. Also loads Max for Live devices (`.amxd`) via `amxd~`. **P**: https://docs.cycling74.com/userguide/plugins/. OS split: not stated; AU is macOS-only by nature.
2. Status: current, Max 9. Version U in retrieved pages.
3. Project (patcher): `.maxpat`, JSON text. Cycling '74 has not published a formal spec (S: https://cycling74.com/forums/specification-for-maxpat-json-format). Treat as documented-in-practice text but unspecified.

### Pure Data (vanilla)
1. Vanilla Pd hosts no plug-ins. Hosting comes from externals: `vstplugin~` (VST2/VST3, Windows/macOS/Linux), `plugin~` (LADSPA), `dssi~`. **S**: https://puredata.info/docs/faq/plugins (403 on direct fetch), https://github.com/Spacechild1/vstplugin. The "yes/no" answer for vanilla is no.
2. Status: current, open source. Version U.
3. Project: `.pd`, plain text ("#N canvas ... ; #X obj ..."). Open, but I retrieved no spec page. From memory only; U at primary level.

### Native Instruments Reaktor
1. Reaktor is a plug-in (VST, VST3, AU, AAX, and standalone), not a host of third-party audio plug-ins. Third-party content is Reaktor Ensembles, not plug-in formats. **S**: https://support.native-instruments.com/hc/en-us/articles/115004430405-Setting-Up-a-Third-Party-Reaktor-Product and https://www.kvraudio.com/product/reaktor-by-native-instruments (KVR is non-primary).
2. Status: current, Reaktor 6.5 with VST3 (2023); no Reaktor 7 (S: https://www.gearnews.com/reaktor-7-native-instruments-synths/, press).
3. Project: `.ens` (ensemble), `.rkplr`. Proprietary. U.

### VCV Rack
1. Rack itself hosts no third-party plug-ins. Hosting is by the paid VCV Host module: 64-bit VST2 and VST3 (**P**: https://vcvrack.com/Host, "Buy VCV Host $30"). Rack Pro (also paid) is itself a VST2/VST3/AU/CLAP plug-in with CLAP FX/Generator adapters (**S**: https://vcvrack.com/manual/RackPro, https://github.com/VCVRack/Rack/blob/v2/CHANGELOG.md). Host's OS list: U.
2. Status: current, Rack 2.
3. Project: `.vcv`. Rack 2: POSIX tar compressed with Zstandard, containing `patch.json` (plain JSON) and module assets. Rack 1 patches were plain JSON. **P/S**: https://github.com/VCVRack/Rack/blob/v2/CHANGELOG.md; parser at https://github.com/VCVRack/Rack/blob/v2/src/patch.cpp. Open source, so open.

### AudioMulch
1. VST2 (documented in the vendor's MulchNote https://www.audiomulch.com/mulchnotes/mulchnote_2.htm, S). Audio Units on macOS since 2.1. VST3: not mentioned in the release notes. **P** (via jina): http://www.audiomulch.com/info/release-notes. Direct HTTPS fetch of audiomulch.com is refused.
2. Status: last releases per the release notes page: 2.2.5 (macOS, OS X 10.10+), 2.2.4 (Windows). Release dates not confirmed.
3. Project: `.amh`, XML. The developer states it is undocumented and may change without notice (S: http://www.audiomulch.com/forums/support-help-and-ideas/amh-format-documented-anywhere). XML but not a published spec.

### Sonic Pi
1. No third-party plug-in hosting. Sound comes from SuperCollider synthdefs. The project FAQ has no mention of VST/AU/LADSPA (**P**: https://github.com/sonic-pi-net/sonic-pi/blob/stable/FAQ.md, answer NO). An experimental LADSPA branch existed (S: https://gist.github.com/xavriley/5a98e30ea8fc5aefacdac02e78accd77, not merged as far as I confirmed: U).
2. Status: current, open source. Version U.
3. Project: no project file. Buffers are plain-text Ruby code (`.rb` files can be saved/loaded). U at primary level (README/help not fetched).

### MilkyTracker
1. No plug-in hosting. It is a Fasttracker II compatible tracker that reads and writes `.xm` and `.mod` and exports `.wav`. **S**: https://github.com/milkytracker/MilkyTracker and https://milkytracker.org/docs/manual/MilkyTracker.html. Absence of plug-in support is inferred, not stated.
2. Status: current, open source. Version U.
3. Project: `.xm` / `.mod`. Open, documented. XM spec in the repo: https://github.com/milkytracker/MilkyTracker/blob/master/resources/reference/xm-form.txt (listed in search results, not fetched).

---

## Summary table

"Loads" means the application itself hosts third-party plug-ins in that format. Formats outside the VST3/AU/CLAP/LV2/LADSPA/DSSI list (VST2, AAX, DX, JSFX) are noted in parentheses.

| application | loads VST3/AU/CLAP/LV2/LADSPA/DSSI | current status | open project format (Y/N + extension) |
|---|---|---|---|
| Apple Logic Pro | yes: AU, AUv3 | current | N (`.logicx`) |
| Avid Pro Tools | no (AAX only; U for negative, blog sources) | current | N (`.ptx`) |
| Ableton Live | yes: VST3, AU (also VST2); no CLAP | current (12.4.6, 2026-09-15) | N (`.als` gzip XML, undocumented by vendor) |
| Steinberg Cubase | yes: VST3 (also VST2); AU not supported | current (15) | N (`.cpr`); exports `.dawproject` |
| Image-Line FL Studio | yes: VST3, CLAP; AU on macOS | current (2026) | N (`.flp`) |
| PreSonus Studio One / Fender Studio Pro | yes: VST3, AU (S); CLAP U | renamed to Fender Studio Pro (2026-01-13 per press; redirect observed) | N (`.song` zip of XML, unspecified); `.dawproject` supported |
| REAPER | yes: VST3, AU, CLAP, LV2 (also VST2, DX, JSFX) | current (7.81, 2026-09-28) | Y (`.rpp`, text; parser in WDL) |
| Audacity | 4.x: VST3, LV2 (Linux), AU (macOS); 3.x also LADSPA | current (4.0.0) | Y (`.aup3`, SQLite; schema not published) |
| Ardour | yes: VST3, AU, LV2, LADSPA (also VST2); no CLAP found | current (9) | Y (`.ardour`, XML) |
| LMMS | LV2, LADSPA, VST2; VST3/CLAP no (alpha, in dev) | current (1.3.0-alpha.2) | Y (`.mmp`/`.mmpz`, XML per source; U at primary level) |
| Zrythm | yes: LV2, VST3, AU, CLAP (also VST2, JSFX) | current (2.0 alpha) | U |
| Tracktion Waveform (Free) | yes: VST3, AU (also VST2); no CLAP/LV2 stated | current (14) | U (`.tracktionedit`, JUCE ValueTree) |
| Bitwig Studio | yes: VST3, CLAP (also VST2); no AU | current (6.1) | N (`.bwproject`); authored open `.dawproject` |
| Reason (Studios) | yes: VST3 (also VST2) | current (13) | N (`.reason`) |
| Renoise | yes: VST3/AU/LADSPA/DSSI (per OS; S) | current | N/U (`.xrns` zip of XML + XSD; wiki spec page empty) |
| Adobe Audition | yes: VST3, AU (also VST2) | current | N (`.sesx` XML, no schema) |
| Adobe Premiere Pro | yes: VST3, AU (also VST2) | current | N (`.prproj`) |
| MakeMusic Finale | U | discontinued 2024-08-26; successor Dorico | N (`.musx`/`.mus`); MusicXML export |
| Sony/MAGIX ACID Pro | yes: VST3 (also VST2, DX; Windows only) | current (MAGIX) | N (`.acd`) |
| Sony/MAGIX Vegas Pro | yes: VST3 (also VST2, DX; Windows only) | current (VEGAS Pro 23) | N (`.veg`) |
| GarageBand | yes: AU (AUv3 on iOS) | current | N (`.band`) |
| MilkyTracker | no | current | Y (`.xm`, `.mod`) |
| Steinberg Dorico | yes: VST3 (VST2 whitelisted); AU U | current (6.x) | N (`.dorico` zip, unspecified) |
| DaVinci Resolve | yes: VST3 (Win/mac), AU (mac); not Linux | current | N (`.drp`) |
| Avid Sibelius | yes: VST3 (2026.5), VST2, AU (also AAX) | current (2026.5) | N (`.sib`) |
| Cycling '74 Max | yes: VST3, AU (also VST2) | current (9) | Y-ish (`.maxpat` JSON; no formal spec) |
| Pure Data | no in vanilla; externals add VST/LADSPA/DSSI | current | Y (`.pd`, plain text) |
| NI Reaktor | no (is a plug-in, not a host) | current (6.5) | N (`.ens`) |
| VCV Rack | Rack: no; paid Host module: VST3 (also VST2) | current (2) | Y (`.vcv`, tar+zstd of JSON) |
| AudioMulch | VST2 + AU on macOS; VST3 not found | old (last 2.2.5/2.2.4) | N (`.amh` XML, developer says undocumented) |
| Sonic Pi | no | current | Y (plain-text Ruby `.rb`) |

## Open items to re-verify before relying on this file

- Every **S** or **U** row above.
- Pro Tools, Cubase, Dorico, and Resolve plug-in claims: the primary vendor pages were blocked or empty.
- Audacity 4.0.0 and LMMS 1.3.0-alpha.2 release years.
- Zrythm 2.0 alpha date and project format.
- Logic Pro's current version and status.
